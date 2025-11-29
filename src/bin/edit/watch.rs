// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-platform file watcher for detecting external file modifications.
//!
//! Uses native OS APIs when available:
//! - Windows: ReadDirectoryChangesW with events
//! - Linux: inotify
//! - macOS/BSD: kqueue
//!
//! Falls back to timestamp polling if native watching fails.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

/// Events that can be received from the file watcher
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// File was modified externally
    Modified(PathBuf),
    /// File was deleted
    Deleted(PathBuf),
}

/// Cross-platform file watcher
pub struct FileWatcher {
    event_rx: Receiver<WatchEvent>,
    command_tx: Sender<WatchCommand>,
    thread: Option<JoinHandle<()>>,
}

enum WatchCommand {
    Watch(PathBuf),
    Unwatch(PathBuf),
    Shutdown,
}

impl FileWatcher {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();

        let thread = thread::spawn(move || {
            watcher_thread(command_rx, event_tx);
        });

        Self {
            event_rx,
            command_tx,
            thread: Some(thread),
        }
    }

    pub fn watch(&self, path: &Path) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let _ = self.command_tx.send(WatchCommand::Watch(canonical));
    }

    pub fn unwatch(&self, path: &Path) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let _ = self.command_tx.send(WatchCommand::Unwatch(canonical));
    }

    pub fn poll_events(&self) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        events
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        let _ = self.command_tx.send(WatchCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Polling fallback state
struct WatchedFile {
    path: PathBuf,
    last_modified: Option<SystemTime>,
    last_check: Instant,
    initialized: bool,
}

impl WatchedFile {
    fn new(path: PathBuf) -> Self {
        let last_modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        Self {
            path,
            last_modified,
            last_check: Instant::now(),
            initialized: false,
        }
    }

    fn check_changed(&mut self) -> Option<WatchEvent> {
        self.last_check = Instant::now();

        match std::fs::metadata(&self.path) {
            Ok(metadata) => {
                if let Ok(current_modified) = metadata.modified() {
                    let changed = self.last_modified != Some(current_modified);
                    let was_initialized = self.initialized;
                    self.last_modified = Some(current_modified);
                    self.initialized = true;

                    if changed && was_initialized {
                        return Some(WatchEvent::Modified(self.path.clone()));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if self.last_modified.is_some() && self.initialized {
                    self.last_modified = None;
                    return Some(WatchEvent::Deleted(self.path.clone()));
                }
                self.initialized = true;
            }
            Err(_) => {}
        }
        None
    }
}

// ============================================================================
// Windows implementation
// ============================================================================

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects};
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

    const BUFFER_SIZE: usize = 8192;

    pub struct NativeWatcher {
        directories: HashMap<PathBuf, DirectoryWatch>,
        wake_event: HANDLE,
    }

    struct DirectoryWatch {
        handle: HANDLE,
        event: HANDLE,
        overlapped: Box<OVERLAPPED>,
        buffer: Box<[u8; BUFFER_SIZE]>,
        files: HashMap<PathBuf, (PathBuf, SystemTime)>,
        pending: bool,
    }

    impl NativeWatcher {
        pub fn new() -> Option<Self> {
            let wake_event = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
            if wake_event.is_null() {
                return None;
            }
            Some(Self {
                directories: HashMap::new(),
                wake_event,
            })
        }

        pub fn watch(&mut self, path: &Path) -> bool {
            let dir = match path.parent() {
                Some(d) if d.as_os_str().is_empty() => Path::new("."),
                Some(d) => d,
                None => return false,
            };
            let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

            let file_name = match path.file_name() {
                Some(n) => PathBuf::from(n),
                None => return false,
            };

            if !self.directories.contains_key(&dir) {
                match Self::create_directory_watch(&dir) {
                    Some(watch) => { self.directories.insert(dir.clone(), watch); }
                    None => return false,
                }
            }

            if let Some(watch) = self.directories.get_mut(&dir) {
                let mtime = std::fs::metadata(path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                watch.files.insert(file_name, (path.to_path_buf(), mtime));

                if !watch.pending {
                    Self::start_read(watch);
                }
            }
            true
        }

        fn create_directory_watch(dir: &Path) -> Option<DirectoryWatch> {
            let dir_wide: Vec<u16> = OsStr::new(dir)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let handle = unsafe {
                CreateFileW(
                    dir_wide.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    ptr::null_mut(),
                )
            };

            if handle == INVALID_HANDLE_VALUE {
                return None;
            }

            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if event.is_null() {
                unsafe { CloseHandle(handle) };
                return None;
            }

            let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
            overlapped.hEvent = event;

            Some(DirectoryWatch {
                handle,
                event,
                overlapped,
                buffer: Box::new([0u8; BUFFER_SIZE]),
                files: HashMap::new(),
                pending: false,
            })
        }

        fn start_read(watch: &mut DirectoryWatch) -> bool {
            let result = unsafe {
                ReadDirectoryChangesW(
                    watch.handle,
                    watch.buffer.as_mut_ptr() as *mut _,
                    BUFFER_SIZE as u32,
                    0,
                    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
                    ptr::null_mut(),
                    watch.overlapped.as_mut() as *mut _,
                    None,
                )
            };
            watch.pending = result != 0;
            watch.pending
        }

        pub fn unwatch(&mut self, path: &Path) {
            let dir = match path.parent() {
                Some(d) if d.as_os_str().is_empty() => Path::new("."),
                Some(d) => d,
                None => return,
            };
            let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

            if let Some(file_name) = path.file_name() {
                if let Some(watch) = self.directories.get_mut(&dir) {
                    watch.files.remove(&PathBuf::from(file_name));
                }
            }
        }

        pub fn poll(&mut self, timeout_ms: u32) -> Vec<WatchEvent> {
            let mut events = Vec::new();

            if self.directories.is_empty() {
                unsafe { WaitForMultipleObjects(1, &self.wake_event, 0, timeout_ms) };
                return events;
            }

            let mut handles: Vec<HANDLE> = Vec::with_capacity(self.directories.len() + 1);
            handles.push(self.wake_event);
            let dir_keys: Vec<PathBuf> = self.directories.keys().cloned().collect();
            for key in &dir_keys {
                if let Some(watch) = self.directories.get(key) {
                    handles.push(watch.event);
                }
            }

            let result = unsafe {
                WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout_ms)
            };

            if result == WAIT_TIMEOUT {
                return events;
            }

            let signaled_idx = result.wrapping_sub(WAIT_OBJECT_0) as usize;
            if signaled_idx == 0 {
                return events;
            }

            if signaled_idx > 0 && signaled_idx <= dir_keys.len() {
                let dir_key = &dir_keys[signaled_idx - 1];
                if let Some(watch) = self.directories.get_mut(dir_key) {
                    events.extend(Self::process_event(watch));
                }
            }

            events
        }

        fn process_event(watch: &mut DirectoryWatch) -> Vec<WatchEvent> {
            let mut events = Vec::new();

            if !watch.pending {
                return events;
            }

            let mut bytes_transferred: u32 = 0;
            let success = unsafe {
                GetOverlappedResult(watch.handle, watch.overlapped.as_mut() as *mut _, &mut bytes_transferred, 0)
            };

            watch.pending = false;

            if success == 0 || bytes_transferred == 0 {
                Self::start_read(watch);
                return events;
            }

            let mut offset = 0usize;
            while offset < bytes_transferred as usize {
                let info = unsafe { &*(watch.buffer.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION) };

                let name_len = info.FileNameLength as usize / 2;
                let name_slice = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_len) };
                let changed_name = String::from_utf16_lossy(name_slice);
                let changed_path = PathBuf::from(&changed_name);

                if let Some((full_path, last_mtime)) = watch.files.get_mut(&changed_path) {
                    if let Ok(metadata) = std::fs::metadata(full_path.as_path()) {
                        if let Ok(current_mtime) = metadata.modified() {
                            if current_mtime != *last_mtime {
                                *last_mtime = current_mtime;
                                events.push(WatchEvent::Modified(full_path.clone()));
                            }
                        }
                    } else {
                        events.push(WatchEvent::Deleted(full_path.clone()));
                    }
                }

                if info.NextEntryOffset == 0 { break; }
                offset += info.NextEntryOffset as usize;
            }

            Self::start_read(watch);
            events
        }
    }

    impl Drop for NativeWatcher {
        fn drop(&mut self) {
            for (_, watch) in &self.directories {
                unsafe {
                    CloseHandle(watch.event);
                    CloseHandle(watch.handle);
                }
            }
            unsafe { CloseHandle(self.wake_event) };
        }
    }

    pub fn create_native_watcher() -> Option<NativeWatcher> {
        NativeWatcher::new()
    }
}

// ============================================================================
// Linux implementation
// ============================================================================

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::os::unix::io::RawFd;

    pub struct NativeWatcher {
        fd: RawFd,
        watch_descriptors: HashMap<i32, PathBuf>,
        path_to_wd: HashMap<PathBuf, i32>,
        last_event: HashMap<PathBuf, Instant>,
    }

    impl NativeWatcher {
        pub fn new() -> Option<Self> {
            let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
            if fd < 0 { return None; }
            Some(Self {
                fd,
                watch_descriptors: HashMap::new(),
                path_to_wd: HashMap::new(),
                last_event: HashMap::new(),
            })
        }

        pub fn watch(&mut self, path: &Path) -> bool {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;

            if self.path_to_wd.contains_key(path) { return true; }

            let c_path = match CString::new(path.as_os_str().as_bytes()) {
                Ok(p) => p, Err(_) => return false,
            };

            let wd = unsafe {
                libc::inotify_add_watch(self.fd, c_path.as_ptr(),
                    (libc::IN_MODIFY | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) as u32)
            };
            if wd < 0 { return false; }

            self.watch_descriptors.insert(wd, path.to_path_buf());
            self.path_to_wd.insert(path.to_path_buf(), wd);
            true
        }

        pub fn unwatch(&mut self, path: &Path) {
            if let Some(wd) = self.path_to_wd.remove(path) {
                unsafe { libc::inotify_rm_watch(self.fd, wd) };
                self.watch_descriptors.remove(&wd);
                self.last_event.remove(path);
            }
        }

        pub fn poll(&mut self, _timeout_ms: u32) -> Vec<WatchEvent> {
            let mut events = Vec::new();
            let mut buffer = [0u8; 4096];
            let now = Instant::now();

            loop {
                let len = unsafe { libc::read(self.fd, buffer.as_mut_ptr() as *mut _, buffer.len()) };
                if len <= 0 { break; }

                let mut offset = 0;
                while offset < len as usize {
                    let event = unsafe { &*(buffer.as_ptr().add(offset) as *const libc::inotify_event) };

                    if let Some(path) = self.watch_descriptors.get(&event.wd) {
                        let dominated = self.last_event.get(path).map(|t| t.elapsed().as_millis() < 100).unwrap_or(false);
                        if !dominated {
                            if event.mask & libc::IN_MODIFY as u32 != 0 {
                                events.push(WatchEvent::Modified(path.clone()));
                                self.last_event.insert(path.clone(), now);
                            }
                            if event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) as u32 != 0 {
                                events.push(WatchEvent::Deleted(path.clone()));
                            }
                        }
                    }
                    offset += std::mem::size_of::<libc::inotify_event>() + event.len as usize;
                }
            }
            events
        }
    }

    impl Drop for NativeWatcher {
        fn drop(&mut self) { unsafe { libc::close(self.fd) }; }
    }

    pub fn create_native_watcher() -> Option<NativeWatcher> { NativeWatcher::new() }
}

// ============================================================================
// macOS implementation
// ============================================================================

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::os::unix::io::RawFd;

    pub struct NativeWatcher {
        kq: RawFd,
        watched_fds: HashMap<RawFd, PathBuf>,
        path_to_fd: HashMap<PathBuf, RawFd>,
    }

    impl NativeWatcher {
        pub fn new() -> Option<Self> {
            let kq = unsafe { libc::kqueue() };
            if kq < 0 { return None; }
            Some(Self { kq, watched_fds: HashMap::new(), path_to_fd: HashMap::new() })
        }

        pub fn watch(&mut self, path: &Path) -> bool {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;

            if self.path_to_fd.contains_key(path) { return true; }

            let c_path = match CString::new(path.as_os_str().as_bytes()) {
                Ok(p) => p, Err(_) => return false,
            };

            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | 0x8000) };
            if fd < 0 { return false; }

            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            event.ident = fd as usize;
            event.filter = libc::EVFILT_VNODE;
            event.flags = libc::EV_ADD | libc::EV_CLEAR;
            event.fflags = libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME;

            if unsafe { libc::kevent(self.kq, &event, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
                unsafe { libc::close(fd) };
                return false;
            }

            self.watched_fds.insert(fd, path.to_path_buf());
            self.path_to_fd.insert(path.to_path_buf(), fd);
            true
        }

        pub fn unwatch(&mut self, path: &Path) {
            if let Some(fd) = self.path_to_fd.remove(path) {
                self.watched_fds.remove(&fd);
                unsafe { libc::close(fd) };
            }
        }

        pub fn poll(&mut self, timeout_ms: u32) -> Vec<WatchEvent> {
            let mut events = Vec::new();
            let mut event_list: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
            let timeout = libc::timespec {
                tv_sec: (timeout_ms / 1000) as i64,
                tv_nsec: ((timeout_ms % 1000) * 1_000_000) as i64,
            };

            let count = unsafe { libc::kevent(self.kq, std::ptr::null(), 0, event_list.as_mut_ptr(), 16, &timeout) };

            for i in 0..count.max(0) as usize {
                let event = &event_list[i];
                if let Some(path) = self.watched_fds.get(&(event.ident as RawFd)) {
                    if event.fflags & libc::NOTE_WRITE != 0 {
                        events.push(WatchEvent::Modified(path.clone()));
                    }
                    if event.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME) != 0 {
                        events.push(WatchEvent::Deleted(path.clone()));
                    }
                }
            }
            events
        }
    }

    impl Drop for NativeWatcher {
        fn drop(&mut self) {
            for (fd, _) in &self.watched_fds { unsafe { libc::close(*fd) }; }
            unsafe { libc::close(self.kq) };
        }
    }

    pub fn create_native_watcher() -> Option<NativeWatcher> { NativeWatcher::new() }
}

// ============================================================================
// Fallback for other platforms
// ============================================================================

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;

    pub struct NativeWatcher;
    impl NativeWatcher {
        pub fn new() -> Option<Self> { None }
        pub fn watch(&mut self, _: &Path) -> bool { false }
        pub fn unwatch(&mut self, _: &Path) {}
        pub fn poll(&mut self, _: u32) -> Vec<WatchEvent> { Vec::new() }
    }
    pub fn create_native_watcher() -> Option<NativeWatcher> { None }
}

// ============================================================================
// Watcher thread
// ============================================================================

fn watcher_thread(command_rx: Receiver<WatchCommand>, event_tx: Sender<WatchEvent>) {
    let mut native_watcher = platform::create_native_watcher();
    let mut polling_files: HashMap<PathBuf, WatchedFile> = HashMap::new();
    let use_native = native_watcher.is_some();

    crate::logging::log_action(&format!("FILE_WATCHER: Started (native={})", use_native));

    let poll_interval = Duration::from_millis(500);
    let mut last_poll = Instant::now();

    loop {
        loop {
            match command_rx.try_recv() {
                Ok(WatchCommand::Watch(path)) => {
                    let use_polling = if let Some(ref mut w) = native_watcher {
                        if w.watch(&path) {
                            crate::logging::log_action(&format!("FILE_WATCHER: native {}", path.display()));
                            false
                        } else { true }
                    } else { true };

                    if use_polling {
                        polling_files.insert(path.clone(), WatchedFile::new(path.clone()));
                        crate::logging::log_action(&format!("FILE_WATCHER: polling {}", path.display()));
                    }
                }
                Ok(WatchCommand::Unwatch(path)) => {
                    if let Some(ref mut w) = native_watcher { w.unwatch(&path); }
                    polling_files.remove(&path);
                }
                Ok(WatchCommand::Shutdown) => {
                    crate::logging::log_action("FILE_WATCHER: shutdown");
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        let timeout = if polling_files.is_empty() { 200 } else { 50 };
        if let Some(ref mut w) = native_watcher {
            for event in w.poll(timeout) { let _ = event_tx.send(event); }
        } else {
            thread::sleep(Duration::from_millis(timeout as u64));
        }

        if last_poll.elapsed() >= poll_interval {
            last_poll = Instant::now();
            for watched in polling_files.values_mut() {
                if let Some(event) = watched.check_changed() {
                    let _ = event_tx.send(event);
                }
            }
        }
    }
}
