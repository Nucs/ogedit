// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unix-specific platform code.
//!
//! Read the `windows` module for reference.
//! TODO: This reminds me that the sys API should probably be a trait.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs::File;
use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::os::fd::FromRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::ptr::{self, NonNull, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time;

use crate::apperr;
use crate::arena::{Arena, ArenaString};
use crate::helpers::*;

#[cfg(target_os = "netbsd")]
const fn desired_mprotect(flags: c_int) -> c_int {
    // NetBSD allows an mmap(2) caller to specify what protection flags they
    // will use later via mprotect. It does not allow a caller to move from
    // PROT_NONE to PROT_READ | PROT_WRITE.
    //
    // see PROT_MPROTECT in man 2 mmap
    flags << 3
}

#[cfg(not(target_os = "netbsd"))]
const fn desired_mprotect(_: c_int) -> c_int {
    libc::PROT_NONE
}

struct State {
    stdin: libc::c_int,
    stdin_flags: libc::c_int,
    stdin_current_flags: libc::c_int,
    stdout_initial_termios: Option<libc::termios>,
    // Buffer for incomplete UTF-8 sequences (max 4 bytes needed)
    utf8_buf: [u8; 4],
    utf8_len: usize,
}

static mut STATE: State = State {
    stdin: libc::STDIN_FILENO,
    stdin_flags: -1,
    stdin_current_flags: -1,
    stdout_initial_termios: None,
    utf8_buf: [0; 4],
    utf8_len: 0,
};

static INJECT_RESIZE: AtomicBool = AtomicBool::new(false);

extern "C" fn sigwinch_handler(_: libc::c_int) {
    INJECT_RESIZE.store(true, Ordering::Relaxed);
}

pub fn init() -> Deinit {
    Deinit
}

pub fn switch_modes() -> apperr::Result<()> {
    unsafe {
        // Store the stdin flags so we can more easily toggle `O_NONBLOCK` later on.
        STATE.stdin_flags = check_int_return(libc::fcntl(STATE.stdin, libc::F_GETFL))?;
        STATE.stdin_current_flags = STATE.stdin_flags;

        // Set INJECT_RESIZE to true whenever we get a SIGWINCH.
        let mut sigwinch_action: libc::sigaction = mem::zeroed();
        sigwinch_action.sa_sigaction = sigwinch_handler as *const () as libc::sighandler_t;
        check_int_return(libc::sigaction(libc::SIGWINCH, &sigwinch_action, null_mut()))?;

        // Get the original terminal modes so we can disable raw mode on exit.
        let mut termios = MaybeUninit::<libc::termios>::uninit();
        check_int_return(libc::tcgetattr(libc::STDOUT_FILENO, termios.as_mut_ptr()))?;
        let mut termios = termios.assume_init();
        STATE.stdout_initial_termios = Some(termios);

        termios.c_iflag &= !(
            // When neither IGNBRK...
            libc::IGNBRK
            // ...nor BRKINT are set, a BREAK reads as a null byte ('\0'), ...
            | libc::BRKINT
            // ...except when PARMRK is set, in which case it reads as the sequence \377 \0 \0.
            | libc::PARMRK
            // Disable input parity checking.
            | libc::INPCK
            // Disable stripping of eighth bit.
            | libc::ISTRIP
            // Disable mapping of NL to CR on input.
            | libc::INLCR
            // Disable ignoring CR on input.
            | libc::IGNCR
            // Disable mapping of CR to NL on input.
            | libc::ICRNL
            // Disable software flow control.
            | libc::IXON
            | libc::IXOFF
        );
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            termios.c_iflag &= !libc::IUCLC;
        }
        // Disable output processing.
        termios.c_oflag &= !libc::OPOST;
        termios.c_cflag &= !(
            // Reset character size mask.
            libc::CSIZE
            // Disable parity generation.
            | libc::PARENB
        );
        // Set character size back to 8 bits.
        termios.c_cflag |= libc::CS8 | libc::CREAD;
        termios.c_lflag &= !(
            // Disable signal generation (SIGINT, SIGTSTP, SIGQUIT).
            libc::ISIG
            // Disable canonical mode (line buffering).
            | libc::ICANON
            // Disable echoing of input characters.
            | libc::ECHO
            // Disable echoing of NL.
            | libc::ECHONL
            // Disable extended input processing (e.g. Ctrl-V).
            | libc::IEXTEN
        );

        termios.c_cc[libc::VMIN] = 1;
        termios.c_cc[libc::VTIME] = 0;
        termios_setattr(libc::STDOUT_FILENO, &termios)
    }
}

fn termios_setattr(fd: c_int, termios: &libc::termios) -> apperr::Result<()> {
    loop {
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) } == 0 {
            return Ok(());
        }
        if errno() != libc::EINTR {
            return Err(errno_to_apperr(errno()));
        }
    }
}

pub struct Deinit;

impl Drop for Deinit {
    fn drop(&mut self) {
        unsafe {
            #[allow(static_mut_refs)]
            if let Some(termios) = STATE.stdout_initial_termios.take() {
                // Restore the original terminal modes.
                _ = termios_setattr(libc::STDOUT_FILENO, &termios);
            }
            if STATE.stdin_flags != -1 {
                libc::fcntl(STATE.stdin, libc::F_SETFL, STATE.stdin_flags);
                STATE.stdin_flags = -1;
                STATE.stdin_current_flags = -1;
            }
            if STATE.stdin != libc::STDIN_FILENO {
                libc::close(STATE.stdin);
                STATE.stdin = libc::STDIN_FILENO;
            }
        }
    }
}

/// Read the native terminal size without injecting bytes into the input stream.
pub fn get_window_size() -> apperr::Result<Size> {
    let mut winsz: libc::winsize = unsafe { mem::zeroed() };
    loop {
        let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut winsz) };
        if ret == 0 {
            break;
        }
        let error = errno();
        if error != libc::EINTR {
            return Err(errno_to_apperr(error));
        }
    }
    if winsz.ws_col == 0 || winsz.ws_row == 0 {
        Err(errno_to_apperr(libc::EINVAL))
    } else {
        Ok(Size { width: winsz.ws_col as CoordType, height: winsz.ws_row as CoordType })
    }
}

/// Reads from stdin.
///
/// Returns `None` if there was an error reading from stdin.
/// Returns `Some((resize, ""))` if the given timeout was reached.
/// Otherwise, it returns the pending native resize and decoded input text.
pub fn read_stdin(
    arena: &Arena,
    mut timeout: time::Duration,
) -> Option<(Option<Size>, ArenaString<'_>)> {
    unsafe {
        if INJECT_RESIZE.load(Ordering::Relaxed) {
            timeout = time::Duration::ZERO;
        }

        let read_poll = timeout != time::Duration::MAX;
        let mut buf = Vec::new_in(arena);

        // We don't know if the input is valid UTF8, so we first use a Vec and then
        // later turn it into UTF8 using `from_utf8_lossy_owned`.
        // It is important that we allocate the buffer with an explicit capacity,
        // because we later use `spare_capacity_mut` to access it.
        buf.reserve(4 * KIBI);

        // We got some leftover broken UTF8 from a previous read? Prepend it.
        if STATE.utf8_len != 0 {
            buf.extend_from_slice(&STATE.utf8_buf[..STATE.utf8_len]);
            STATE.utf8_len = 0;
        }

        loop {
            if timeout != time::Duration::MAX {
                let beg = time::Instant::now();

                let mut pollfd = libc::pollfd { fd: STATE.stdin, events: libc::POLLIN, revents: 0 };
                let ret;
                #[cfg(target_os = "linux")]
                {
                    let ts = libc::timespec {
                        tv_sec: timeout.as_secs() as libc::time_t,
                        tv_nsec: timeout.subsec_nanos() as libc::c_long,
                    };
                    ret = libc::ppoll(&mut pollfd, 1, &ts, ptr::null());
                }
                #[cfg(not(target_os = "linux"))]
                {
                    ret = libc::poll(
                        &mut pollfd,
                        1,
                        timeout.as_millis().min(libc::c_int::MAX as u128) as libc::c_int,
                    );
                }
                if ret < 0 {
                    if errno() == libc::EINTR {
                        timeout = timeout.saturating_sub(beg.elapsed());
                        if INJECT_RESIZE.load(Ordering::Relaxed) {
                            break;
                        }
                        continue;
                    }
                    return None;
                }
                if ret == 0 {
                    break; // Timeout? We can stop reading.
                }

                timeout = timeout.saturating_sub(beg.elapsed());
            };

            // If we're asked for a non-blocking read we need
            // to manipulate `O_NONBLOCK` and vice versa.
            set_tty_nonblocking(read_poll);

            // Read from stdin.
            let spare = buf.spare_capacity_mut();
            let ret = libc::read(STATE.stdin, spare.as_mut_ptr() as *mut _, spare.len());
            if ret > 0 {
                buf.set_len(buf.len() + ret as usize);
                break;
            }
            if ret == 0 {
                return None; // EOF
            }
            if ret < 0 {
                match errno() {
                    libc::EINTR if INJECT_RESIZE.load(Ordering::Relaxed) => break,
                    libc::EAGAIN if timeout == time::Duration::ZERO => break,
                    libc::EINTR | libc::EAGAIN => {}
                    _ => return None,
                }
            }
        }

        if !buf.is_empty() {
            // We only need to check the last 3 bytes for UTF-8 continuation bytes,
            // because we should be able to assume that any 4 byte sequence is complete.
            let lim = buf.len().saturating_sub(3);
            let mut off = buf.len() - 1;

            // Find the start of the last potentially incomplete UTF-8 sequence.
            while off > lim && buf[off] & 0b1100_0000 == 0b1000_0000 {
                off -= 1;
            }

            let seq_len = match buf[off] {
                b if b & 0b1000_0000 == 0 => 1,
                b if b & 0b1110_0000 == 0b1100_0000 => 2,
                b if b & 0b1111_0000 == 0b1110_0000 => 3,
                b if b & 0b1111_1000 == 0b1111_0000 => 4,
                // If the lead byte we found isn't actually one, we don't cache it.
                // `from_utf8_lossy_owned` will replace it with U+FFFD.
                _ => 0,
            };

            // Cache incomplete sequence if any.
            if off + seq_len > buf.len() {
                STATE.utf8_len = buf.len() - off;
                STATE.utf8_buf[..STATE.utf8_len].copy_from_slice(&buf[off..]);
                buf.truncate(off);
            }
        }

        let mut result = ArenaString::from_utf8_lossy_owned(buf);

        let resize = if INJECT_RESIZE.swap(false, Ordering::Relaxed) {
            get_window_size().ok()
        } else {
            None
        };

        result.shrink_to_fit();
        Some((resize, result))
    }
}

pub fn write_stdout(text: &str) {
    let mut buf = text.as_bytes();

    while !buf.is_empty() {
        let chunk = &buf[..buf.len().min(GIBI)];
        let n = unsafe { libc::write(libc::STDOUT_FILENO, chunk.as_ptr().cast(), chunk.len()) };

        if n > 0 {
            buf = &buf[n as usize..];
            continue;
        }

        if n == 0 {
            return; // broken pipe
        }

        #[allow(unreachable_patterns, reason = "On Linux EAGAIN and EWOULDBLOCK are the same")]
        match errno() {
            libc::EINTR => continue,
            libc::EAGAIN | libc::EWOULDBLOCK => {
                // Block until it becomes writable
                let mut pollfd =
                    libc::pollfd { fd: libc::STDOUT_FILENO, events: libc::POLLOUT, revents: 0 };
                loop {
                    let ret = unsafe { libc::poll(&mut pollfd, 1, -1) };
                    if ret >= 0 {
                        if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                            return; // broken pipe
                        }
                        break;
                    }
                    if errno() != libc::EINTR {
                        return; // broken pipe
                    }
                }
            }
            _ => return, // broken pipe
        }
    }
}

/// Sets/Resets `O_NONBLOCK` on the TTY handle.
///
/// Note that setting this flag applies to both stdin and stdout, because the
/// TTY is a bidirectional device and both handles refer to the same thing.
fn set_tty_nonblocking(nonblock: bool) {
    unsafe {
        let is_nonblock = (STATE.stdin_current_flags & libc::O_NONBLOCK) != 0;
        if is_nonblock != nonblock {
            let flags = STATE.stdin_current_flags ^ libc::O_NONBLOCK;
            if libc::fcntl(STATE.stdin, libc::F_SETFL, flags) != -1 {
                STATE.stdin_current_flags = flags;
            }
        }
    }
}

/// Reopen the terminal input before consuming redirected stdin.
pub fn reopen_stdin_if_redirected() -> apperr::Result<Option<File>> {
    unsafe {
        if libc::isatty(STATE.stdin) != 0 {
            return Ok(None);
        }
        // Keep the old state intact if opening the controlling terminal fails.
        let terminal = check_int_return(libc::open(c"/dev/tty".as_ptr(), libc::O_RDONLY))?;
        STATE.stdin = terminal;
        Ok(Some(File::from_raw_fd(libc::STDIN_FILENO)))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct FileId {
    dev: u64,
    ino: u64,
}

/// Returns a unique identifier for the given file by handle or path.
pub fn file_id(file: Option<&File>, path: &Path) -> apperr::Result<FileId> {
    let metadata = match file {
        Some(file) => file.metadata()?,
        None => std::fs::metadata(path)?,
    };
    Ok(FileId { dev: metadata.dev(), ino: metadata.ino() })
}

/// Reserves a virtual memory region of the given size.
/// To commit the memory, use `virtual_commit`.
/// To release the memory, use `virtual_release`.
///
/// # Safety
///
/// This function is unsafe because it uses raw pointers.
/// Don't forget to release the memory when you're done with it or you'll leak it.
pub unsafe fn virtual_reserve(size: usize) -> apperr::Result<NonNull<u8>> {
    unsafe {
        let ptr = libc::mmap(
            null_mut(),
            size,
            desired_mprotect(libc::PROT_READ | libc::PROT_WRITE),
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if ptr.is_null() || ptr::eq(ptr, libc::MAP_FAILED) {
            Err(errno_to_apperr(libc::ENOMEM))
        } else {
            Ok(NonNull::new_unchecked(ptr as *mut u8))
        }
    }
}

/// Releases a virtual memory region of the given size.
///
/// # Safety
///
/// This function is unsafe because it uses raw pointers.
/// Make sure to only pass pointers acquired from `virtual_reserve`.
pub unsafe fn virtual_release(base: NonNull<u8>, size: usize) {
    unsafe {
        libc::munmap(base.cast().as_ptr(), size);
    }
}

/// Commits a virtual memory region of the given size.
///
/// # Safety
///
/// This function is unsafe because it uses raw pointers.
/// Make sure to only pass pointers acquired from `virtual_reserve`
/// and to pass a size less than or equal to the size passed to `virtual_reserve`.
pub unsafe fn virtual_commit(base: NonNull<u8>, size: usize) -> apperr::Result<()> {
    unsafe {
        let status = libc::mprotect(base.cast().as_ptr(), size, libc::PROT_READ | libc::PROT_WRITE);
        if status != 0 { Err(errno_to_apperr(libc::ENOMEM)) } else { Ok(()) }
    }
}

unsafe fn load_library(name: *const c_char) -> apperr::Result<NonNull<c_void>> {
    unsafe {
        NonNull::new(libc::dlopen(name, libc::RTLD_LAZY))
            .ok_or_else(|| errno_to_apperr(libc::ENOENT))
    }
}

/// Loads a function from a dynamic library.
///
/// # Safety
///
/// This function is highly unsafe as it requires you to know the exact type
/// of the function you're loading. No type checks whatsoever are performed.
//
// It'd be nice to constrain T to std::marker::FnPtr, but that's unstable.
pub unsafe fn get_proc_address<T>(
    handle: NonNull<c_void>,
    name: *const c_char,
) -> apperr::Result<T> {
    unsafe {
        let sym = libc::dlsym(handle.as_ptr(), name);
        if sym.is_null() {
            Err(errno_to_apperr(libc::ENOENT))
        } else {
            Ok(mem::transmute_copy(&sym))
        }
    }
}

pub struct LibIcu {
    pub libicuuc: NonNull<c_void>,
    pub libicui18n: NonNull<c_void>,
}

pub fn load_icu() -> apperr::Result<LibIcu> {
    const LIBICUUC: &str = concat!(env!("EDIT_CFG_ICUUC_SONAME"), "\0");
    const LIBICUI18N: &str = concat!(env!("EDIT_CFG_ICUI18N_SONAME"), "\0");
    let uc = CStr::from_bytes_with_nul(LIBICUUC.as_bytes())
        .map_err(|_| errno_to_apperr(libc::EINVAL))?;
    let i18n = CStr::from_bytes_with_nul(LIBICUI18N.as_bytes())
        .map_err(|_| errno_to_apperr(libc::EINVAL))?;
    select_icu_pair(uc, i18n, env!("EDIT_CFG_ICU_VERSIONED_FALLBACK") == "true", load_icu_pair)
}

fn load_icu_pair(uc: &CStr, i18n: &CStr) -> apperr::Result<LibIcu> {
    let libicuuc = unsafe { load_library(uc.as_ptr())? };
    if uc == i18n {
        return Ok(LibIcu { libicuuc, libicui18n: libicuuc });
    }
    match unsafe { load_library(i18n.as_ptr()) } {
        Ok(libicui18n) => Ok(LibIcu { libicuuc, libicui18n }),
        Err(error) => {
            // A newer core library without its matching i18n library must not
            // leak a loader reference before we try another installed pair.
            unsafe { libc::dlclose(libicuuc.as_ptr()) };
            Err(error)
        }
    }
}

fn select_icu_pair<T, E>(
    uc: &CStr,
    i18n: &CStr,
    versioned_fallback: bool,
    mut load: impl FnMut(&CStr, &CStr) -> Result<T, E>,
) -> Result<T, E> {
    let original_error = match load(uc, i18n) {
        Ok(libraries) => return Ok(libraries),
        Err(error) => error,
    };
    if versioned_fallback {
        // ICU uses integer major versions in ELF SONAMEs. Probe through the
        // platform loader, so ld.so.cache, standard paths and LD_LIBRARY_PATH
        // retain their normal semantics. Bound the work and never mix versions.
        for major in (1..=255).rev() {
            let uc = format!("libicuuc.so.{major}\0");
            let i18n = format!("libicui18n.so.{major}\0");
            let uc = CStr::from_bytes_with_nul(uc.as_bytes()).unwrap();
            let i18n = CStr::from_bytes_with_nul(i18n.as_bytes()).unwrap();
            if let Ok(libraries) = load(uc, i18n) {
                return Ok(libraries);
            }
        }
    }
    Err(original_error)
}
/// ICU, by default, adds the major version as a suffix to each exported symbol.
/// They also recommend to disable this for system-level installations (`runConfigureICU Linux --disable-renaming`),
/// but I found that many (most?) Linux distributions don't do this for some reason.
/// This function returns the suffix, if any.
#[cfg(edit_icu_renaming_auto_detect)]
pub fn icu_detect_renaming_suffix(arena: &Arena, handle: NonNull<c_void>) -> ArenaString<'_> {
    let mut result = ArenaString::new_in(arena);
    // Probe a public C export instead of relying on an incidental C++ destructor
    // export or on filesystem symlink names; runtime-only and stripped ICU builds
    // still provide u_errorName.
    if unsafe { get_proc_address::<*const c_void>(handle, c"u_errorName".as_ptr()) }.is_ok() {
        return result;
    }
    for major in (1..=255).rev() {
        let name = format!("u_errorName_{major}\0");
        if unsafe { get_proc_address::<*const c_void>(handle, name.as_ptr().cast()) }.is_ok() {
            result.push('_');
            result.push_str(&major.to_string());
            break;
        }
    }
    result
}

#[cfg(edit_icu_renaming_auto_detect)]
/// Appends the configured ICU symbol suffix into `arena`.
///
/// # Safety
/// `name` must point to a live, null-terminated, valid UTF-8 symbol name.
/// A newly allocated result remains valid only while `arena` is alive and not reset.
pub unsafe fn icu_add_renaming_suffix<'a, 'b, 'r>(
    arena: &'a Arena,
    name: *const c_char,
    suffix: &str,
) -> *const c_char
where
    'a: 'r,
    'b: 'r,
{
    if suffix.is_empty() {
        name
    } else {
        // SAFETY: The caller guarantees a live, valid UTF-8 C string.
        let name = unsafe { CStr::from_ptr(name) };
        let name = unsafe { name.to_str().unwrap_unchecked() };

        let mut res = ManuallyDrop::new(ArenaString::new_in(arena));
        res.reserve(name.len() + suffix.len() + 1);
        res.push_str(name);
        res.push_str(suffix);
        res.push('\0');
        res.as_ptr() as *const c_char
    }
}

pub fn preferred_languages(arena: &Arena) -> Vec<ArenaString<'_>, &Arena> {
    preferred_languages_from(arena, |key| std::env::var(key).ok())
}

fn preferred_languages_from<'a>(
    arena: &'a Arena,
    mut get: impl FnMut(&str) -> Option<String>,
) -> Vec<ArenaString<'a>, &'a Arena> {
    let mut locales = Vec::new_in(arena);

    for key in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(val) = get(key)
            && !val.is_empty()
        {
            locales.extend(val.split(':').filter(|s| !s.is_empty()).map(|s| {
                // Replace all underscores with dashes,
                // because the localization code expects pt-br, not pt_BR.
                let mut res = Vec::new_in(arena);
                res.extend(s.as_bytes().iter().map(|&b| if b == b'_' { b'-' } else { b }));
                unsafe { ArenaString::from_utf8_unchecked(res) }
            }));
            if !locales.is_empty() {
                break;
            }
        }
    }

    locales
}

#[inline]
fn errno() -> i32 {
    // Under `-O -Copt-level=s` the 1.87 compiler fails to fully inline and
    // remove the raw_os_error() call. This leaves us with the drop() call.
    // ManuallyDrop fixes that and results in a direct `std::sys::os::errno` call.
    ManuallyDrop::new(std::io::Error::last_os_error()).raw_os_error().unwrap_or(0)
}

#[inline]
pub(crate) fn io_error_to_apperr(err: std::io::Error) -> apperr::Error {
    errno_to_apperr(err.raw_os_error().unwrap_or(0))
}

pub fn apperr_format(f: &mut std::fmt::Formatter<'_>, code: u32) -> std::fmt::Result {
    write!(f, "Error {code}")?;

    unsafe {
        let ptr = libc::strerror(code as i32);
        if !ptr.is_null() {
            let msg = CStr::from_ptr(ptr).to_string_lossy();
            write!(f, ": {msg}")?;
        }
    }

    Ok(())
}

pub fn apperr_is_not_found(err: apperr::Error) -> bool {
    err == errno_to_apperr(libc::ENOENT)
}

const fn errno_to_apperr(no: c_int) -> apperr::Error {
    apperr::Error::new_sys(if no < 0 { 0 } else { no as u32 })
}

fn check_int_return(ret: libc::c_int) -> apperr::Result<libc::c_int> {
    if ret < 0 { Err(errno_to_apperr(errno())) } else { Ok(ret) }
}

#[cfg(test)]
pub fn memfd() -> std::io::Result<File> {
    fn checked_fd(ret: libc::c_int) -> std::io::Result<libc::c_int> {
        if ret < 0 { Err(std::io::Error::last_os_error()) } else { Ok(ret) }
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let fd = checked_fd(libc::memfd_create(c"edit".as_ptr(), libc::MFD_CLOEXEC))?;
        Ok(File::from_raw_fd(fd))
    }
    #[cfg(not(target_os = "linux"))]
    unsafe {
        let stream = libc::tmpfile();
        if stream.is_null() {
            return Err(std::io::Error::last_os_error());
        }

        // Duplicate the descriptor so that closing the C stream doesn't close our file.
        let file = checked_fd(libc::fcntl(libc::fileno(stream), libc::F_DUPFD_CLOEXEC, 0))
            .map(|fd| File::from_raw_fd(fd));
        let close = checked_fd(libc::fclose(stream));
        let file = file?;
        close?;

        Ok(file)
    }
}

#[cfg(test)]
mod upstream_tests {
    use super::*;

    #[test]
    fn icu_runtime_selection_uses_complete_matching_versioned_pairs() {
        let mut attempts = Vec::new();
        let selected = select_icu_pair(c"libicuuc.so", c"libicui18n.so", true, |uc, i18n| {
            let uc = uc.to_str().unwrap();
            let i18n = i18n.to_str().unwrap();
            attempts.push((uc.to_owned(), i18n.to_owned()));
            // Newer core-only installations are skipped; both libraries must load.
            if uc == "libicuuc.so.74" && i18n == "libicui18n.so.74" { Ok(74) } else { Err(()) }
        });
        assert_eq!(selected, Ok(74));
        assert_eq!(attempts[0], ("libicuuc.so".to_owned(), "libicui18n.so".to_owned()));
        for (uc, i18n) in &attempts[1..] {
            assert_eq!(uc.strip_prefix("libicuuc.so."), i18n.strip_prefix("libicui18n.so."));
        }
        assert_eq!(attempts.last().unwrap().0, "libicuuc.so.74");
    }

    #[test]
    fn icu_exact_overrides_never_fall_back_and_default_aliases_win() {
        let mut attempts = 0;
        let result: Result<(), _> =
            select_icu_pair(c"/custom/uc.so", c"/custom/i18n.so", false, |uc, i18n| {
                attempts += 1;
                assert_eq!(uc, c"/custom/uc.so");
                assert_eq!(i18n, c"/custom/i18n.so");
                Err("custom load failure")
            });
        assert_eq!(result, Err("custom load failure"));
        assert_eq!(attempts, 1);
        let result: Result<_, ()> =
            select_icu_pair(c"libicuuc.so", c"libicui18n.so", true, |_, _| {
                attempts += 1;
                Ok(())
            });
        assert_eq!(result, Ok(()));
        assert_eq!(attempts, 2);
    }

    #[test]
    fn icu_version_probe_is_bounded_and_preserves_the_initial_error() {
        let mut attempts = 0;
        let result: Result<(), _> =
            select_icu_pair(c"libicuuc.so", c"libicui18n.so", true, |_, _| {
                attempts += 1;
                Err(attempts)
            });
        assert_eq!(attempts, 256);
        assert_eq!(result, Err(1));
    }

    #[test]
    #[cfg(edit_icu_renaming_auto_detect)]
    fn native_runtime_icu_resolves_public_exports_without_development_aliases() {
        let libraries = load_icu().expect("install the ICU runtime package for this native test");
        let arena = Arena::new(MEBI).unwrap();
        let suffix = icu_detect_renaming_suffix(&arena, libraries.libicuuc);
        for (handle, name) in
            [(libraries.libicuuc, "u_errorName"), (libraries.libicui18n, "ucol_open")]
        {
            let symbol = std::ffi::CString::new(format!("{name}{suffix}")).unwrap();
            assert!(unsafe { get_proc_address::<*const c_void>(handle, symbol.as_ptr()) }.is_ok());
        }
        unsafe {
            if libraries.libicui18n != libraries.libicuuc {
                libc::dlclose(libraries.libicui18n.as_ptr());
            }
            libc::dlclose(libraries.libicuuc.as_ptr());
        }
    }

    #[test]
    fn language_fallback_skips_empty_and_delimiter_only_overrides() {
        let arena = Arena::new(MEBI).unwrap();
        let cases = [
            (["", "", "pt_BR.UTF-8", "en_US"], vec!["pt-BR.UTF-8"]),
            (["::", "de_DE", "fr_FR", "en_US"], vec!["de-DE"]),
            (["fr_FR::en_US:", "de_DE", "", ""], vec!["fr-FR", "en-US"]),
            (["", "", "", "en_US"], vec!["en-US"]),
        ];
        for (values, expected) in cases {
            let keys = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"];
            let actual = preferred_languages_from(&arena, |key| {
                Some(values[keys.iter().position(|&k| k == key).unwrap()].to_owned())
            });
            assert_eq!(actual.iter().map(|s| s.as_str()).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn file_identity_uses_metadata_for_directories_and_open_files() {
        let directory = std::env::temp_dir();
        assert!(file_id(None, &directory).is_ok());
        let file = memfd().unwrap();
        assert!(file_id(Some(&file), Path::new("missing-not-used")).is_ok());
    }
}
