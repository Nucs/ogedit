// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Path related helpers.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::{Component, MAIN_SEPARATOR_STR, Path, PathBuf};

/// Normalizes a given path by removing redundant components.
/// The given path must be absolute (e.g. by joining it with the current working directory).
pub fn normalize(path: &Path) -> PathBuf {
    // Verbatim Windows paths treat '/' as a literal byte in components. Convert
    // separators before walking components, including after joining a relative
    // user path onto a canonical \\?\ base directory.
    let path = native_separators(path);
    let mut res = PathBuf::with_capacity(path.as_os_str().as_encoded_bytes().len());
    let mut root_len = 0;

    for component in path.components() {
        match component {
            Component::Prefix(p) => res.push(p.as_os_str()),
            Component::RootDir => {
                res.push(OsStr::new(MAIN_SEPARATOR_STR));
                root_len = res.as_os_str().as_encoded_bytes().len();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                // Get the length up to the parent directory
                if let Some(len) = res
                    .parent()
                    .map(|p| p.as_os_str().as_encoded_bytes().len())
                    // Ensure we don't pop the root directory
                    && len >= root_len
                {
                    // Pop the last component from `res`.
                    //
                    // This can be replaced with a plain `res.as_mut_os_string().truncate(len)`
                    // once `os_string_truncate` is stabilized (#133262).
                    let mut bytes = res.into_os_string().into_encoded_bytes();
                    bytes.truncate(len);
                    res = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(bytes) });
                }
            }
            Component::Normal(p) => res.push(p),
        }
    }

    res
}

/// Interpret a path pasted or typed into a path field. A matching double-quote
/// pair may enclose the whole input or just the path before :line[:column].
/// Unmatched quotes remain untouched. This does not perform shell expansion.
/// On Unix, ordinary filesystem paths should bypass this explicit-input API.
pub fn from_user_input(input: &Path) -> PathBuf {
    let bytes = input.as_os_str().as_encoded_bytes();
    let unquoted = if bytes.first() == Some(&b'"') {
        bytes[1..].iter().position(|&byte| byte == b'"').and_then(|relative| {
            let close = relative + 1;
            let suffix = &bytes[close + 1..];
            if !suffix.is_empty() && !is_location_suffix(suffix) {
                return None;
            }
            let mut value = bytes[1..close].to_vec();
            value.extend_from_slice(suffix);
            // Removing ASCII quote bytes at their own character boundaries and
            // concatenating existing encoded slices preserves the OS encoding.
            Some(PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(value) }))
        })
    } else {
        None
    };
    native_separators(unquoted.as_deref().unwrap_or(input)).into_owned()
}

/// Resolve an entered path against a directory after interpreting its quotes
/// and separators. Sanitizing before join is essential for quoted absolutes.
pub fn join_user_input(directory: &Path, input: &Path) -> PathBuf {
    // Quotes and backslashes are legal Unix filename characters. Existing
    // literal filenames selected in the picker always retain their identity.
    #[cfg(unix)]
    if directory.join(input).exists() {
        return normalize(&directory.join(input));
    }
    let input = from_user_input(input);
    normalize(&directory.join(input))
}

fn is_location_suffix(suffix: &[u8]) -> bool {
    if suffix.first() != Some(&b':') {
        return false;
    }
    let mut fields = suffix[1..].split(|&byte| byte == b':');
    let number = |value: &[u8]| !value.is_empty() && value.iter().all(u8::is_ascii_digit);
    fields.next().is_some_and(number) && fields.next().is_none_or(number) && fields.next().is_none()
}

fn native_separators(path: &Path) -> Cow<'_, Path> {
    #[cfg(windows)]
    if path.as_os_str().as_encoded_bytes().contains(&b'/') {
        let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
        for byte in &mut bytes {
            if *byte == b'/' {
                *byte = b'\\';
            }
        }
        // ASCII slash and backslash are complete encoded characters on Windows.
        return Cow::Owned(PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(bytes) }));
    }
    Cow::Borrowed(path)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::*;

    fn norm(s: &str) -> OsString {
        normalize(Path::new(s)).into_os_string()
    }

    #[cfg(unix)]
    #[test]
    fn test_unix() {
        assert_eq!(norm("/a/b/c"), "/a/b/c");
        assert_eq!(norm("/a/b/c/"), "/a/b/c");
        assert_eq!(norm("/a/./b"), "/a/b");
        assert_eq!(norm("/a/b/../c"), "/a/c");
        assert_eq!(norm("/../../a"), "/a");
        assert_eq!(norm("/../"), "/");
        assert_eq!(norm("/a//b/c"), "/a/b/c");
        assert_eq!(norm("/a/b/c/../../../../d"), "/d");
        assert_eq!(norm("//"), "/");
    }

    #[cfg(windows)]
    #[test]
    fn test_windows() {
        assert_eq!(norm(r"C:\a\b\c"), r"C:\a\b\c");
        assert_eq!(norm(r"C:\a\b\c\"), r"C:\a\b\c");
        assert_eq!(norm(r"C:\a\.\b"), r"C:\a\b");
        assert_eq!(norm(r"C:\a\b\..\c"), r"C:\a\c");
        assert_eq!(norm(r"C:\..\..\a"), r"C:\a");
        assert_eq!(norm(r"C:\..\"), r"C:\");
        assert_eq!(norm(r"C:\a\\b\c"), r"C:\a\b\c");
        assert_eq!(norm(r"C:/a\b/c"), r"C:\a\b\c");
        assert_eq!(norm(r"C:\a\b\c\..\..\..\..\d"), r"C:\d");
        assert_eq!(norm(r"\\server\share\path"), r"\\server\share\path");
        assert_eq!(norm(r"\\?\C:\base\one/two/../three"), r"\\?\C:\base\one\three");
        assert_eq!(norm(r"//server/share\one/../two"), r"\\server\share\two");
        assert_eq!(norm(r"\\?\UNC/server/share/one\../two"), r"\\?\UNC\server\share\two");
    }

    #[test]
    fn entered_quotes_and_location_suffixes() {
        for (input, expected) in [
            ("\"file name.txt\"", "file name.txt"),
            ("\"file name.txt:12:3\"", "file name.txt:12:3"),
            ("\"file name.txt\":12:3", "file name.txt:12:3"),
            ("\"file name.txt\":12", "file name.txt:12"),
            ("\"file name.txt", "\"file name.txt"),
            ("file name.txt\"", "file name.txt\""),
            ("\"file\":12:3:4", "\"file\":12:3:4"),
            ("\"file\":x", "\"file\":x"),
            ("\"file\":12:", "\"file\":12:"),
            ("\"file\"tail", "\"file\"tail"),
            ("\" file \"", " file "),
            ("\"\"", ""),
        ] {
            assert_eq!(from_user_input(Path::new(input)), Path::new(expected), "{input:?}");
        }
    }

    #[test]
    #[cfg(windows)]
    fn windows_entered_paths_are_normalized_before_join() {
        let base = Path::new(r"\\?\C:\base\folder");
        assert_eq!(
            join_user_input(base, Path::new(r#""K:\projects\sample\README.md""#)),
            Path::new(r"K:\projects\sample\README.md")
        );
        assert_eq!(
            join_user_input(base, Path::new(r#""K:/projects\sample folder/README.md":12:3"#)),
            Path::new(r"K:\projects\sample folder\README.md:12:3")
        );
        assert_eq!(
            join_user_input(base, Path::new(r"..\one/two.txt")),
            Path::new(r"\\?\C:\base\one\two.txt")
        );
        assert_eq!(
            join_user_input(base, Path::new(r"/rooted\file.txt")),
            Path::new(r"\\?\C:\rooted\file.txt")
        );
        assert_eq!(
            join_user_input(base, Path::new(r#""//server/share\folder/file.txt""#)),
            Path::new(r"\\server\share\folder\file.txt")
        );
        assert_eq!(from_user_input(Path::new(r"C:123")), Path::new(r"C:123"));
    }

    #[test]
    #[cfg(unix)]
    fn unix_literal_backslashes_are_not_separators() {
        assert_eq!(from_user_input(Path::new(r"dir\file")), Path::new(r"dir\file"));
        assert_eq!(
            join_user_input(Path::new("/base"), Path::new(r#""dir\file""#)),
            Path::new(r"/base/dir\file")
        );
    }
}
