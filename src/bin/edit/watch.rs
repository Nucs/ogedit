// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-platform file watcher for detecting external file modifications.
//!
//! Uses native OS APIs when available:
//! - Windows: ReadDirectoryChangesW
//! - Linux: inotify
//! - macOS/BSD: kqueue
//!
//! Falls back to timestamp polling if native watching fails.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

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
    /// Channel to receive events from the watcher thread
    event_rx: Receiver<WatchEvent>,
    /// Channel to send commands to the watcher thread
    command_tx: Sender<WatchCommand>,
    /// Handle to the watcher thread
    _thread: JoinHandle<()>,
}

enum WatchCommand {
    Watch(PathBuf),
    Unwatch(PathBuf),
    Shutdown,
}

impl FileWatcher {
    /// Create a new file watcher
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();

        let thread = thread::spawn(move || {
            watcher_thread(command_rx, event_tx);
        });

        Self {
            event_rx,
            command_tx,
            _thread: thread,
        }
    }

    /// Start watching a file for changes
    pub fn watch(&self, path: &Path) {
        let _ = self.command_tx.send(WatchCommand::Watch(path.to_path_buf()));
    }

    /// Stop watching a file
    pub fn unwatch(&self, path: &Path) {
        let _ = self.command_tx.send(WatchCommand::Unwatch(path.to_path_buf()));
    }

    /// Check for any pending file change events (non-blocking)
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
    }
}

/// Internal state for tracking a watched file
struct WatchedFile {
    path: PathBuf,
    last_modified: Option<SystemTime>,
    last_check: std::time::Instant,
}

impl WatchedFile {
    fn new(path: PathBuf) -> Self {
        let last_modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        Self {
            path,
            last_modified,
            last_check: std::time::Instant::now(),
        }
    }

    /// Check if the file has changed since last check
    fn check_changed(&mut self) -> Option<WatchEvent> {
        self.last_check = std::time::Instant::now();

        match std::fs::metadata(&self.path) {
            Ok(metadata) => {
                if let Ok(current_modified) = metadata.modified() {
                    if self.last_modified != Some(current_modified) {
                        self.last_modified = Some(current_modified);
                        return Some(WatchEvent::Modified(self.path.clone()));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if self.last_modified.is_some() {
                    self.last_modified = None;
                    return Some(WatchEvent::Deleted(self.path.clone()));
                }
            }
            Err(_) => {}
        }
        None
    }

}

// ============================================================================
// Platform-specific implementations
// ============================================================================

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus};

    /// Windows-specific directory watcher using ReadDirectoryChangesW
    pub struct NativeWatcher {
        /// Map of directory handle to watched files in that directory
        directories: HashMap<PathBuf, DirectoryWatch>,
        /// Completion port for async I/O
        completion_port: HANDLE,
    }

    struct DirectoryWatch {
        handle: HANDLE,
        buffer: Vec<u8>,
        files: HashMap<PathBuf, SystemTime>,
    }

    impl NativeWatcher {
        pub fn new() -> Option<Self> {
            let completion_port = unsafe {
                CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1)
            };
            if completion_port == ptr::null_mut() {
                return None;
            }
            Some(Self {
                directories: HashMap::new(),
                completion_port,
            })
        }

        pub fn watch(&mut self, path: &Path) -> bool {
            let dir = match path.parent() {
                Some(d) => d.to_path_buf(),
                None => return false,
            };

            if !self.directories.contains_key(&dir) {
                let dir_wide: Vec<u16> = OsStr::new(&dir)
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
                    return false;
                }

                // Associate with completion port
                let result = unsafe {
                    CreateIoCompletionPort(handle, self.completion_port, dir.as_os_str().len(), 0)
                };
                if result == ptr::null_mut() {
                    unsafe { CloseHandle(handle) };
                    return false;
                }

                let mut watch = DirectoryWatch {
                    handle,
                    buffer: vec![0u8; 4096],
                    files: HashMap::new(),
                };

                // Start watching
                if !Self::start_watch(&mut watch) {
                    unsafe { CloseHandle(handle) };
                    return false;
                }

                self.directories.insert(dir.clone(), watch);
            }

            // Track the specific file
            if let Some(watch) = self.directories.get_mut(&dir) {
                let mtime = std::fs::metadata(path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                watch.files.insert(path.to_path_buf(), mtime);
            }

            true
        }

        fn start_watch(watch: &mut DirectoryWatch) -> bool {
            let mut bytes_returned: u32 = 0;
            let result = unsafe {
                ReadDirectoryChangesW(
                    watch.handle,
                    watch.buffer.as_mut_ptr() as *mut _,
                    watch.buffer.len() as u32,
                    0, // Don't watch subtree
                    FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
                    &mut bytes_returned,
                    ptr::null_mut(), // Using completion port instead
                    None,
                )
            };
            result != 0
        }

        pub fn unwatch(&mut self, path: &Path) {
            if let Some(dir) = path.parent() {
                if let Some(watch) = self.directories.get_mut(dir) {
                    watch.files.remove(path);
                    // Keep watching the directory if other files are still tracked
                }
            }
        }

        pub fn poll(&mut self, timeout_ms: u32) -> Vec<WatchEvent> {
            let mut events = Vec::new();

            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped: *mut std::ffi::c_void = ptr::null_mut();

            let result = unsafe {
                GetQueuedCompletionStatus(
                    self.completion_port,
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped as *mut _ as *mut _,
                    timeout_ms,
                )
            };

            if result != 0 && bytes_transferred > 0 {
                // Process the notification and check tracked files
                for (_, watch) in &mut self.directories {
                    for (file_path, last_mtime) in &mut watch.files {
                        if let Ok(metadata) = std::fs::metadata(file_path) {
                            if let Ok(current_mtime) = metadata.modified() {
                                if current_mtime != *last_mtime {
                                    *last_mtime = current_mtime;
                                    events.push(WatchEvent::Modified(file_path.clone()));
                                }
                            }
                        }
                    }
                    // Re-arm the watch
                    Self::start_watch(watch);
                }
            }

            events
        }
    }

    impl Drop for NativeWatcher {
        fn drop(&mut self) {
            for (_, watch) in &self.directories {
                unsafe { CloseHandle(watch.handle) };
            }
            unsafe { CloseHandle(self.completion_port) };
        }
    }

    pub fn create_native_watcher() -> Option<NativeWatcher> {
        NativeWatcher::new()
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::io::RawFd;

    /// Linux-specific watcher using inotify
    pub struct NativeWatcher {
        fd: RawFd,
        watch_descriptors: HashMap<i32, PathBuf>,
        path_to_wd: HashMap<PathBuf, i32>,
    }

    impl NativeWatcher {
        pub fn new() -> Option<Self> {
            let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
            if fd < 0 {
                return None;
            }
            Some(Self {
                fd,
                watch_descriptors: HashMap::new(),
                path_to_wd: HashMap::new(),
            })
        }

        pub fn watch(&mut self, path: &Path) -> bool {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;

            let c_path = match CString::new(path.as_os_str().as_bytes()) {
                Ok(p) => p,
                Err(_) => return false,
            };

            let wd = unsafe {
                libc::inotify_add_watch(
                    self.fd,
                    c_path.as_ptr(),
                    (libc::IN_MODIFY | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) as u32,
                )
            };

            if wd < 0 {
                return false;
            }

            self.watch_descriptors.insert(wd, path.to_path_buf());
            self.path_to_wd.insert(path.to_path_buf(), wd);
            true
        }

        pub fn unwatch(&mut self, path: &Path) {
            if let Some(wd) = self.path_to_wd.remove(path) {
                unsafe { libc::inotify_rm_watch(self.fd, wd) };
                self.watch_descriptors.remove(&wd);
            }
        }

        pub fn poll(&mut self, _timeout_ms: u32) -> Vec<WatchEvent> {
            let mut events = Vec::new();
            let mut buffer = [0u8; 4096];

            loop {
                let len = unsafe {
                    libc::read(self.fd, buffer.as_mut_ptr() as *mut _, buffer.len())
                };

                if len <= 0 {
                    break;
                }

                let mut offset = 0;
                while offset < len as usize {
                    let event = unsafe {
                        &*(buffer.as_ptr().add(offset) as *const libc::inotify_event)
                    };

                    if let Some(path) = self.watch_descriptors.get(&event.wd) {
                        if event.mask & libc::IN_MODIFY as u32 != 0 {
                            events.push(WatchEvent::Modified(path.clone()));
                        }
                        if event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) as u32 != 0 {
                            events.push(WatchEvent::Deleted(path.clone()));
                        }
                    }

                    offset += std::mem::size_of::<libc::inotify_event>() + event.len as usize;
                }
            }

            events
        }
    }

    impl Drop for NativeWatcher {
        fn drop(&mut self) {
            unsafe { libc::close(self.fd) };
        }
    }

    pub fn create_native_watcher() -> Option<NativeWatcher> {
        NativeWatcher::new()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::io::RawFd;

    /// macOS-specific watcher using kqueue
    pub struct NativeWatcher {
        kq: RawFd,
        watched_fds: HashMap<RawFd, PathBuf>,
        path_to_fd: HashMap<PathBuf, RawFd>,
    }

    impl NativeWatcher {
        pub fn new() -> Option<Self> {
            let kq = unsafe { libc::kqueue() };
            if kq < 0 {
                return None;
            }
            Some(Self {
                kq,
                watched_fds: HashMap::new(),
                path_to_fd: HashMap::new(),
            })
        }

        pub fn watch(&mut self, path: &Path) -> bool {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;

            let c_path = match CString::new(path.as_os_str().as_bytes()) {
                Ok(p) => p,
                Err(_) => return false,
            };

            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_EVTONLY) };
            if fd < 0 {
                return false;
            }

            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            event.ident = fd as usize;
            event.filter = libc::EVFILT_VNODE;
            event.flags = libc::EV_ADD | libc::EV_CLEAR;
            event.fflags = libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME;

            let result = unsafe {
                libc::kevent(self.kq, &event, 1, std::ptr::null_mut(), 0, std::ptr::null())
            };

            if result < 0 {
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

            let count = unsafe {
                libc::kevent(
                    self.kq,
                    std::ptr::null(),
                    0,
                    event_list.as_mut_ptr(),
                    event_list.len() as i32,
                    &timeout,
                )
            };

            for i in 0..count.max(0) as usize {
                let event = &event_list[i];
                let fd = event.ident as RawFd;

                if let Some(path) = self.watched_fds.get(&fd) {
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
            for (fd, _) in &self.watched_fds {
                unsafe { libc::close(*fd) };
            }
            unsafe { libc::close(self.kq) };
        }
    }

    pub fn create_native_watcher() -> Option<NativeWatcher> {
        NativeWatcher::new()
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;

    /// Fallback polling-based watcher for unsupported platforms
    pub struct NativeWatcher;

    impl NativeWatcher {
        pub fn new() -> Option<Self> {
            None // Force fallback to polling
        }

        pub fn watch(&mut self, _path: &Path) -> bool {
            false
        }

        pub fn unwatch(&mut self, _path: &Path) {}

        pub fn poll(&mut self, _timeout_ms: u32) -> Vec<WatchEvent> {
            Vec::new()
        }
    }

    pub fn create_native_watcher() -> Option<NativeWatcher> {
        None
    }
}

// ============================================================================
// Watcher thread implementation
// ============================================================================

fn watcher_thread(command_rx: Receiver<WatchCommand>, event_tx: Sender<WatchEvent>) {
    // Try native watcher first, fall back to polling
    let mut native_watcher = platform::create_native_watcher();
    let mut polling_files: HashMap<PathBuf, WatchedFile> = HashMap::new();
    let use_native = native_watcher.is_some();

    crate::logging::log_action(&format!(
        "FILE_WATCHER: Started (native={})",
        use_native
    ));

    loop {
        // Check for commands (non-blocking)
        match command_rx.try_recv() {
            Ok(WatchCommand::Watch(path)) => {
                if let Some(ref mut watcher) = native_watcher {
                    if watcher.watch(&path) {
                        crate::logging::log_action(&format!(
                            "FILE_WATCHER: Watching (native) {}",
                            path.display()
                        ));
                    } else {
                        // Fall back to polling for this file
                        polling_files.insert(path.clone(), WatchedFile::new(path.clone()));
                        crate::logging::log_action(&format!(
                            "FILE_WATCHER: Watching (polling) {}",
                            path.display()
                        ));
                    }
                } else {
                    polling_files.insert(path.clone(), WatchedFile::new(path.clone()));
                    crate::logging::log_action(&format!(
                        "FILE_WATCHER: Watching (polling) {}",
                        path.display()
                    ));
                }
            }
            Ok(WatchCommand::Unwatch(path)) => {
                if let Some(ref mut watcher) = native_watcher {
                    watcher.unwatch(&path);
                }
                polling_files.remove(&path);
                crate::logging::log_action(&format!(
                    "FILE_WATCHER: Unwatched {}",
                    path.display()
                ));
            }
            Ok(WatchCommand::Shutdown) => {
                crate::logging::log_action("FILE_WATCHER: Shutting down");
                break;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break,
        }

        // Poll native watcher
        if let Some(ref mut watcher) = native_watcher {
            for event in watcher.poll(100) {
                let _ = event_tx.send(event);
            }
        }

        // Poll files using timestamp comparison (for fallback or when native fails)
        for (_, watched) in &mut polling_files {
            if watched.last_check.elapsed() >= Duration::from_millis(500) {
                if let Some(event) = watched.check_changed() {
                    let _ = event_tx.send(event);
                }
            }
        }

        // Small sleep to prevent busy-waiting
        thread::sleep(Duration::from_millis(100));
    }
}
