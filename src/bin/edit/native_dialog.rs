// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The Windows Common Item Dialog lives on its own STA thread. Its modal message
//! loop never blocks terminal rendering, file watching, or recovery checkpoints.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

type PickerResult = Result<Option<PathBuf>, String>;

pub struct NativeFilePicker {
    result: Receiver<PickerResult>,
    finished: bool,
}

impl NativeFilePicker {
    pub const fn supported() -> bool {
        cfg!(windows)
    }

    pub fn start(initial: PathBuf) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let cwd = std::env::current_dir()
                .map_err(|err| format!("Cannot determine the starting folder: {err}"))?;
            // HWND is not Send. Capture only the console's identity here and
            // validate its visibility again on the worker before using it.
            let owner = platform::visible_console_owner();
            let (sender, result) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("ogedit-file-picker".into())
                .spawn(move || {
                    let result = platform::pick(initial, cwd, owner);
                    let _ = sender.send(result);
                })
                .map_err(|err| format!("Cannot start the Windows file browser: {err}"))?;
            Ok(Self { result, finished: false })
        }
        #[cfg(not(windows))]
        {
            let _ = initial;
            Err("The native file browser is available on Windows.".into())
        }
    }

    /// Completion is delivered exactly once. Cancellation is Ok(None), while
    /// worker or Windows failures remain distinct from a user cancellation.
    pub fn try_finish(&mut self) -> Option<PickerResult> {
        if self.finished {
            return None;
        }
        let result = match self.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Err("The Windows file browser closed unexpectedly.".into())
            }
        };
        self.finished = true;
        Some(result)
    }
}

#[cfg(any(windows, test))]
struct InitialSelection {
    directory: PathBuf,
    filename: Option<std::ffi::OsString>,
}

#[cfg(any(windows, test))]
fn initial_selection(initial: &std::path::Path, cwd: &std::path::Path) -> InitialSelection {
    #[cfg(windows)]
    let shell_cwd = shell_path(cwd);
    #[cfg(windows)]
    let cwd = shell_cwd.as_path();
    let absolute = if initial.as_os_str().is_empty() {
        cwd.to_owned()
    } else if initial.is_absolute() {
        initial.to_owned()
    } else {
        cwd.join(initial)
    };
    #[cfg(windows)]
    let absolute = shell_path(&absolute);
    if absolute.is_dir() {
        return InitialSelection { directory: absolute, filename: None };
    }
    let filename = absolute.file_name().filter(|name| valid_filename(name)).map(ToOwned::to_owned);
    let directory =
        absolute.ancestors().skip(1).find(|ancestor| ancestor.is_dir()).unwrap_or(cwd).to_owned();
    InitialSelection { directory, filename }
}

/// Win32 file I/O uses canonical verbatim paths, but the Shell parsing APIs
/// reject that namespace. Convert only the picker's private copy, preserving
/// UTF-16 code units and leaving document/save/watch paths unchanged.
#[cfg(windows)]
fn shell_path(path: &std::path::Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide: Vec<_> = path.as_os_str().encode_wide().collect();
    if !wide.starts_with(&[92, 92, 63, 92]) {
        return path.to_owned();
    }
    if wide.len() >= 8
        && wide[4..7]
            .iter()
            .zip(b"UNC")
            .all(|(&a, &b)| a == u16::from(b) || a == u16::from(b.to_ascii_lowercase()))
        && wide[7] == 92
    {
        let mut normalized = vec![92, 92];
        normalized.extend_from_slice(&wide[8..]);
        PathBuf::from(OsString::from_wide(&normalized))
    } else if wide.len() >= 7
        && ((65..=90).contains(&wide[4]) || (97..=122).contains(&wide[4]))
        && wide[5] == 58
        && wide[6] == 92
    {
        PathBuf::from(OsString::from_wide(&wide[4..]))
    } else {
        path.to_owned()
    }
}

#[cfg(any(windows, test))]
fn valid_filename(name: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    let characters: Vec<u16> = {
        use std::os::windows::ffi::OsStrExt;
        name.encode_wide().collect()
    };
    #[cfg(not(windows))]
    let characters: Vec<u16> = name.to_string_lossy().encode_utf16().collect();
    // In particular, filename:line:column locations are not valid initial file
    // names. Clearing them lets the browser open in their containing directory.
    if characters.is_empty()
        || characters.iter().any(|&c| c < 32 || b"<>:\"/\\|?*".iter().any(|&b| u16::from(b) == c))
        || characters.last().is_some_and(|&c| c == u16::from(b'.') || c == u16::from(b' '))
    {
        return false;
    }
    let stem = name.to_string_lossy().split('.').next().unwrap_or_default().to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum Completion {
    Accepted,
    Cancelled,
    Failed,
}

#[cfg(any(windows, test))]
fn completion(status: i32) -> Completion {
    const CANCELLED: i32 = 0x8007_04c7u32 as i32; // HRESULT_FROM_WIN32(ERROR_CANCELLED)
    if status >= 0 {
        Completion::Accepted
    } else if status == CANCELLED {
        Completion::Cancelled
    } else {
        Completion::Failed
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
        CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::UI::Shell::{
        FOS_ALLNONSTORAGEITEMS, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
        FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
        IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    use windows::core::{PCWSTR, PWSTR, w};

    use super::{Completion, PathBuf, PickerResult, completion, initial_selection};

    pub fn visible_console_owner() -> usize {
        // A pseudoconsole has a hidden message-only window; using it as owner
        // can strand the picker behind unrelated apps. Never borrow the current
        // foreground window: in browser-based terminals it belongs to the browser.
        let window = unsafe { GetConsoleWindow() };
        if unsafe { IsWindowVisible(window) }.as_bool() { window.0 as usize } else { 0 }
    }

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    struct AllocatedPath(PWSTR);
    impl Drop for AllocatedPath {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(Some(self.0.0.cast())) };
        }
    }

    pub fn pick(initial: PathBuf, cwd: PathBuf, owner: usize) -> PickerResult {
        let initial = initial_selection(&initial, &cwd);
        let directory: Vec<u16> =
            initial.directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let filename = initial
            .filename
            .as_ref()
            .map(|name| name.encode_wide().chain(Some(0)).collect::<Vec<_>>());
        // All COM objects are created and released inside the same STA worker.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
                .ok()
                .map_err(|err| format!("Cannot initialize the Windows file browser: {err}"))?;
        }
        let _apartment = Apartment;
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
                .map_err(|err| format!("Cannot create the Windows file browser: {err}"))?;
        unsafe {
            let options = dialog
                .GetOptions()
                .map_err(|err| format!("Cannot configure the Windows file browser: {err}"))?;
            dialog
                .SetOptions(
                    (options
                        | FOS_FILEMUSTEXIST
                        | FOS_PATHMUSTEXIST
                        | FOS_FORCEFILESYSTEM
                        | FOS_NOCHANGEDIR)
                        & !(FOS_ALLOWMULTISELECT | FOS_PICKFOLDERS | FOS_ALLNONSTORAGEITEMS),
                )
                .map_err(|err| format!("Cannot configure the Windows file browser: {err}"))?;
            let folder: IShellItem = SHCreateItemFromParsingName(PCWSTR(directory.as_ptr()), None)
                .map_err(|err| {
                    format!("Cannot open the starting folder in the file browser: {err}")
                })?;
            // Browse starts from the editor's currently chosen directory rather
            // than a process-global Shell MRU directory from an earlier dialog.
            dialog
                .SetFolder(&folder)
                .map_err(|err| format!("Cannot select the starting folder: {err}"))?;
            if let Some(filename) = &filename {
                dialog
                    .SetFileName(PCWSTR(filename.as_ptr()))
                    .map_err(|err| format!("Cannot set the initial filename: {err}"))?;
            }
            dialog
                .SetTitle(w!("Open file — OGEdit"))
                .map_err(|err| format!("Cannot set the file browser title: {err}"))?;
            let owner = HWND(owner as *mut std::ffi::c_void);
            let owner =
                (owner == GetConsoleWindow() && IsWindowVisible(owner).as_bool()).then_some(owner);
            if let Err(error) = dialog.Show(owner) {
                return match completion(error.code().0) {
                    Completion::Cancelled => Ok(None),
                    _ => Err(format!("The Windows file browser could not open: {error}")),
                };
            }
            let selected = dialog
                .GetResult()
                .map_err(|err| format!("Cannot read the selected file: {err}"))?;
            let selected = AllocatedPath(
                selected
                    .GetDisplayName(SIGDN_FILESYSPATH)
                    .map_err(|err| format!("Cannot read the selected file path: {err}"))?,
            );
            if selected.0.is_null() {
                return Err("The Windows file browser returned an empty path.".into());
            }
            // COM allocates the complete UTF-16 path, avoiding fixed MAX_PATH
            // truncation and preserving non-ASCII and unpaired-surrogate names.
            let path = PathBuf::from(OsString::from_wide(selected.0.as_wide()));
            let metadata = std::fs::metadata(&path)
                .map_err(|err| format!("Cannot open the selected file: {err}"))?;
            if !path.is_absolute() || !metadata.is_file() {
                return Err("Choose an existing file.".into());
            }
            Ok(Some(path))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn initial_folder_file_unicode_and_location_suffix() {
        let root = std::env::temp_dir().join(format!(
            "ogedit-native-dialog-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let folder = root.join("תיקייה 😀");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("notes 日本語.txt");
        std::fs::write(&file, b"fixture").unwrap();
        let prepared = initial_selection(&folder, &root);
        assert_eq!(prepared.directory, folder);
        assert_eq!(prepared.filename, None);
        let prepared = initial_selection(&file, &root);
        assert_eq!(prepared.directory, folder);
        assert_eq!(prepared.filename.as_deref(), file.file_name());
        #[cfg(windows)]
        {
            let canonical = std::fs::canonicalize(&file).unwrap();
            let prepared = initial_selection(&canonical, &std::fs::canonicalize(&root).unwrap());
            // TEMP may use an 8.3 alias (RUNNER~1 on hosted Windows runners),
            // while canonicalization expands it to the same long-name folder.
            assert_eq!(
                std::fs::canonicalize(&prepared.directory).unwrap(),
                std::fs::canonicalize(&folder).unwrap()
            );
            assert!(!prepared.directory.as_os_str().to_string_lossy().starts_with(r"\\?\"));
            assert_eq!(prepared.filename.as_deref(), file.file_name());
            assert!(canonical.as_os_str().to_string_lossy().starts_with(r"\\?\"));
            // Exercise Shell path parsing without displaying a dialog: this is
            // the API that rejected active documents' canonical paths in QA.
            std::thread::spawn(move || {
                use std::os::windows::ffi::OsStrExt;

                use windows::Win32::System::Com::{
                    COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
                };
                use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName};
                use windows::core::PCWSTR;
                let wide: Vec<_> =
                    prepared.directory.as_os_str().encode_wide().chain(Some(0)).collect();
                unsafe {
                    CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
                }
                let parsed = unsafe {
                    SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None)
                }
                .map(drop);
                unsafe {
                    CoUninitialize();
                }
                parsed.expect("Shell accepts the prepared canonical-document folder");
            })
            .join()
            .unwrap();
        }
        let prepared = initial_selection(Path::new("new.txt"), &folder);
        assert_eq!(prepared.directory, folder);
        assert_eq!(prepared.filename.as_deref(), Some(OsStr::new("new.txt")));
        let prepared = initial_selection(&folder.join("notes.txt:12:5"), &root);
        assert_eq!(prepared.directory, folder);
        assert!(prepared.filename.is_none());
        assert!(!valid_filename(OsStr::new("CON.txt")));
        assert!(!valid_filename(OsStr::new("invalid\0.txt")));
        assert!(valid_filename(OsStr::new("Japanese 日本語.txt")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn shell_paths_remove_only_disk_and_unc_verbatim_prefixes() {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        assert_eq!(
            shell_path(Path::new(r"\\?\C:\תיקייה\notes 日本語.txt")),
            Path::new(r"C:\תיקייה\notes 日本語.txt")
        );
        assert_eq!(
            shell_path(Path::new(r"\\?\UNC\server\share\folder\notes.txt")),
            Path::new(r"\\server\share\folder\notes.txt")
        );
        assert_eq!(
            shell_path(Path::new(r"\\server\share\notes.txt")),
            Path::new(r"\\server\share\notes.txt")
        );
        assert_eq!(shell_path(Path::new(r"\\.\device")), Path::new(r"\\.\device"));
        let mut unusual: Vec<_> = r"\\?\C:\folder\".encode_utf16().collect();
        unusual.push(0xd800);
        let result = shell_path(Path::new(&OsString::from_wide(&unusual)));
        assert_eq!(result.as_os_str().encode_wide().collect::<Vec<_>>(), unusual[4..]);
    }

    #[test]
    fn cancellation_is_not_an_error_and_completion_is_once() {
        assert_eq!(completion(0), Completion::Accepted);
        assert_eq!(completion(0x8007_04c7u32 as i32), Completion::Cancelled);
        assert_eq!(completion(0x8007_0005u32 as i32), Completion::Failed);
        let (sender, result) = mpsc::channel();
        let mut picker = NativeFilePicker { result, finished: false };
        assert!(picker.try_finish().is_none());
        sender.send(Ok(None)).unwrap();
        assert_eq!(picker.try_finish(), Some(Ok(None)));
        assert!(picker.try_finish().is_none());
        let (sender, result) = mpsc::channel();
        let mut picker = NativeFilePicker { result, finished: false };
        drop(sender);
        assert!(matches!(picker.try_finish(), Some(Err(_))));
        assert!(picker.try_finish().is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_does_not_launch_a_dialog() {
        assert!(!NativeFilePicker::supported());
        assert!(NativeFilePicker::start(PathBuf::new()).is_err());
    }
}
