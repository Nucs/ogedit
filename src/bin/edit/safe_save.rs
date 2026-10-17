// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Staged saves: never truncate a destination, and never mark a buffer clean here.

use std::fs::{self, File, Metadata, OpenOptions};
use std::hash::Hasher;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use ogedit::{apperr, sys};

pub const APP_SAVE_CONFLICT: apperr::Error = apperr::Error::new_app(1001);
pub const APP_SAVE_LINKED: apperr::Error = apperr::Error::new_app(1002);
pub const APP_SAVE_UNSUPPORTED: apperr::Error = apperr::Error::new_app(1003);
pub const APP_RECOVERY_INVALID: apperr::Error = apperr::Error::new_app(1004);

/// Content and metadata are both checked: timestamps alone miss fast edits and replacements.
pub struct DiskVersion {
    pub modified: Option<SystemTime>,
    pub len: u64,
    id: sys::FileId,
    hash: u64,
}

impl PartialEq for DiskVersion {
    fn eq(&self, other: &Self) -> bool {
        self.modified == other.modified
            && self.len == other.len
            && self.id == other.id
            && self.hash == other.hash
    }
}

impl DiskVersion {
    pub fn read(path: &Path) -> apperr::Result<Option<Self>> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let before = file.metadata()?;
        if !before.is_file() {
            return Err(APP_SAVE_UNSUPPORTED);
        }
        let id = sys::file_id(Some(&file), path)?;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        let mut chunk = [0; 65536];
        let mut len = 0;
        loop {
            let n = file.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            hash.write(&chunk[..n]);
            len += n as u64;
        }
        let after = file.metadata()?;
        if before.modified().ok() != after.modified().ok()
            || before.len() != after.len()
            || len != after.len()
            || sys::file_id(None, path)? != id
        {
            return Err(APP_SAVE_CONFLICT);
        }
        Ok(Some(Self { modified: after.modified().ok(), len, id, hash: hash.finish() }))
    }

    pub fn metadata_matches(&self, path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|m| m.len() == self.len && m.modified().ok() == self.modified)
            && sys::file_id(None, path).is_ok_and(|id| id == self.id)
    }
}

/// Incremental content verification for the status indicator. Saves continue
/// using the complete DiskVersion read and its conflict checks.
pub(crate) struct DiskContentProbe {
    hash: std::collections::hash_map::DefaultHasher,
    scanned: u64,
}

pub(crate) const STATUS_READ_BUDGET: usize = 256 * 1024;

impl DiskContentProbe {
    pub(crate) fn new() -> Self {
        Self { hash: Default::default(), scanned: 0 }
    }

    /// Read at most one fixed-size budget. None means more work is needed;
    /// Some(true) confirms the complete bytes still match the baseline.
    pub(crate) fn advance(
        &mut self,
        path: &Path,
        expected: &DiskVersion,
    ) -> apperr::Result<Option<bool>> {
        // Reopen for each bounded step so switching away from a partially
        // checked document cannot leave one extra descriptor open per tab.
        let mut file = File::open(path)?;
        if sys::file_id(Some(&file), path)? != expected.id {
            return Ok(Some(false));
        }
        file.seek(SeekFrom::Start(self.scanned))?;
        let mut remaining = STATUS_READ_BUDGET;
        let mut chunk = [0; 16 * 1024];
        while remaining > 0 {
            let capacity = remaining.min(chunk.len());
            let read = file.read(&mut chunk[..capacity])?;
            if read == 0 {
                let same = self.scanned == expected.len
                    && self.hash.finish() == expected.hash
                    && expected.metadata_matches(path);
                return Ok(Some(same));
            }
            self.hash.write(&chunk[..read]);
            self.scanned += read as u64;
            if self.scanned > expected.len {
                return Ok(Some(false));
            }
            remaining -= read;
        }
        Ok(None)
    }
}

/// Resolve an existing symlink to its target; for a new file resolve its parent.
/// This also gives watchers the same identity as the document manager.
pub fn resolved_path(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let path = ogedit::path::normalize(&absolute);
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = fs::canonicalize(parent)
    {
        return parent.join(name);
    }
    path
}

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn stage_path(target: &Path) -> io::Result<Stage> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    for _ in 0..100 {
        let n = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".ogedit-{}-{n}.save", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(_) => return Ok(Stage(candidate)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "Cannot allocate a save staging file"))
}

fn check_target(path: &Path, metadata: &Metadata) -> apperr::Result<()> {
    if !metadata.is_file() {
        return Err(APP_SAVE_UNSUPPORTED);
    }
    if metadata.permissions().readonly() {
        #[cfg(windows)]
        return Err(io::Error::from_raw_os_error(5).into());
        #[cfg(unix)]
        return Err(io::Error::from_raw_os_error(libc::EACCES).into());
    }
    // Renaming in a writable directory must not bypass the target's write ACL.
    drop(OpenOptions::new().write(true).open(path)?);
    // Replacing one name of a hard-linked file silently severs that link. Require Save As.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(APP_SAVE_LINKED);
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(APP_SAVE_UNSUPPORTED);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let file = File::open(path)?;
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { info.assume_init() }.nNumberOfLinks > 1 {
            return Err(APP_SAVE_LINKED);
        }
    }
    let _ = path;
    Ok(())
}

/// Write and sync a staging file, recheck the expected disk version, then replace.
/// Like other portable editors, a final check cannot provide a cross-process CAS:
/// a writer that ignores coordination can still race the final rename.
pub fn save<F>(
    path: &Path,
    expected: Option<&DiskVersion>,
    write: F,
) -> apperr::Result<Option<DiskVersion>>
where
    F: FnOnce(&mut File) -> apperr::Result<()>,
{
    let target = resolved_path(path);
    let before = DiskVersion::read(&target)?;
    if before.as_ref() != expected {
        return Err(APP_SAVE_CONFLICT);
    }
    let metadata = match fs::metadata(&target) {
        Ok(metadata) => {
            check_target(&target, &metadata)?;
            Some(metadata)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    // Parent creation happens only after conflict/target validation. The staged
    // writer keeps its existing atomic replacement and permission guarantees.
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }
    let stage = stage_path(&target)?;
    #[cfg(windows)]
    if metadata.is_some() {
        preserve_stage_acl(&target, &stage.0)?;
    }
    {
        let mut file = OpenOptions::new().write(true).truncate(true).open(&stage.0)?;
        write(&mut file)?;
        file.sync_all()?;
    }
    if let Some(metadata) = &metadata {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::MetadataExt;
            let staged = OpenOptions::new().write(true).open(&stage.0)?;
            if unsafe { libc::fchown(staged.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
            staged.set_permissions(metadata.permissions())?;
            preserve_extended_metadata(&target, &staged)?;
            staged.sync_all()?;
        }
        #[cfg(not(unix))]
        fs::set_permissions(&stage.0, metadata.permissions())?;
    }
    let staged = DiskVersion::read(&stage.0)?.ok_or(APP_SAVE_UNSUPPORTED)?;
    if DiskVersion::read(&target)?.as_ref() != expected {
        return Err(APP_SAVE_CONFLICT);
    }
    commit(&stage.0, &target, metadata.is_some())?;
    // The write has committed even if a subsequent reader loses access. Never use an
    // unrelated concurrent version as our baseline: verify our staged bytes first.
    let committed = DiskVersion::read(&target)?;
    if committed
        .as_ref()
        .is_none_or(|version| version.len != staged.len || version.hash != staged.hash)
    {
        return Err(APP_SAVE_CONFLICT);
    }
    Ok(committed)
}

#[cfg(windows)]
fn preserve_stage_acl(target: &Path, stage: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetFileSecurityW, PROTECTED_DACL_SECURITY_INFORMATION,
        SetFileSecurityW,
    };
    let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let stage: Vec<_> = stage.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut length = 0;
    unsafe {
        GetFileSecurityW(
            target.as_ptr(),
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut length,
        );
    }
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut descriptor = vec![0u32; (length as usize).div_ceil(4)];
    if unsafe {
        GetFileSecurityW(
            target.as_ptr(),
            DACL_SECURITY_INFORMATION,
            descriptor.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // Protect the staging ACL from broader inherited permissions before any
    // document bytes are written. ReplaceFile restores the target's metadata.
    if unsafe {
        SetFileSecurityW(
            stage.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.as_mut_ptr().cast(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn preserve_extended_metadata(target: &Path, staged: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let original = File::open(target)?;
    let source = original.as_raw_fd();
    let destination = staged.as_raw_fd();
    let length = unsafe { libc::flistxattr(source, std::ptr::null_mut(), 0) };
    if length < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ENOTSUP) {
            return Ok(());
        }
        return Err(err);
    }
    let mut names = vec![0u8; length as usize];
    let length = unsafe { libc::flistxattr(source, names.as_mut_ptr().cast(), names.len()) };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    for name in names[..length as usize].split_inclusive(|byte| *byte == 0) {
        let size =
            unsafe { libc::fgetxattr(source, name.as_ptr().cast(), std::ptr::null_mut(), 0) };
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut value = vec![0u8; size as usize];
        let size = unsafe {
            libc::fgetxattr(source, name.as_ptr().cast(), value.as_mut_ptr().cast(), value.len())
        };
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe {
            libc::fsetxattr(
                destination,
                name.as_ptr().cast(),
                value.as_ptr().cast(),
                size as usize,
                0,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn preserve_extended_metadata(target: &Path, staged: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let original = File::open(target)?;
    if unsafe {
        libc::fcopyfile(
            original.as_raw_fd(),
            staged.as_raw_fd(),
            std::ptr::null_mut(),
            libc::COPYFILE_ACL | libc::COPYFILE_XATTR,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android", target_os = "macos"))))]
fn preserve_extended_metadata(_: &Path, _: &File) -> io::Result<()> {
    // Never silently discard ACLs on platforms without a metadata-copy backend.
    Err(io::Error::from_raw_os_error(libc::ENOTSUP))
}

fn commit(stage: &Path, target: &Path, existed: bool) -> io::Result<()> {
    if !existed {
        return atomicwrites::move_atomic(stage, target);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
        let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        let stage: Vec<_> = stage.as_os_str().encode_wide().chain(Some(0)).collect();
        // ReplaceFile preserves the original ACL, encryption, compression and streams.
        if unsafe {
            ReplaceFileW(
                target.as_ptr(),
                stage.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        atomicwrites::replace_atomic(stage, target)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn fixture() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ogedit-safe-{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.txt");
        fs::write(&file, b"original").unwrap();
        (dir, file)
    }

    #[test]
    fn status_content_probe_is_bounded_and_checks_same_metadata_changes() {
        let (dir, path) = fixture();
        let bytes = vec![b'a'; STATUS_READ_BUDGET * 3 + 10];
        fs::write(&path, &bytes).unwrap();
        let baseline = DiskVersion::read(&path).unwrap().unwrap();
        let mut probe = DiskContentProbe::new();
        for expected in 1..=3 {
            assert_eq!(probe.advance(&path, &baseline).unwrap(), None);
            assert_eq!(probe.scanned, (STATUS_READ_BUDGET * expected) as u64);
        }
        let mut writer = OpenOptions::new().write(true).open(&path).unwrap();
        writer.seek(SeekFrom::End(-1)).unwrap();
        writer.write_all(b"b").unwrap();
        writer.set_modified(baseline.modified.unwrap()).unwrap();
        drop(writer);
        assert!(baseline.metadata_matches(&path));
        assert_eq!(probe.advance(&path, &baseline).unwrap(), Some(false));

        let current = DiskVersion::read(&path).unwrap().unwrap();
        let mut probe = DiskContentProbe::new();
        for _ in 0..3 {
            assert_eq!(probe.advance(&path, &current).unwrap(), None);
        }
        assert_eq!(probe.advance(&path, &current).unwrap(), Some(true));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saving_creates_nested_parents_and_failed_staging_preserves_no_partial_file() {
        let (dir, existing) = fixture();
        let path = dir.join("missing").join("nested").join("new.txt");
        save(&path, None, |file| Ok(file.write_all(b"complete")?)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"complete");
        let failed = dir.join("other").join("nested").join("failed.txt");
        assert!(
            save(&failed, None, |file| {
                file.write_all(b"partial")?;
                Err(APP_SAVE_UNSUPPORTED)
            })
            .is_err()
        );
        assert!(!failed.exists());
        assert_eq!(fs::read_dir(failed.parent().unwrap()).unwrap().count(), 0);
        assert!(save(&existing.join("cannot-be-a-folder.txt"), None, |_| Ok(())).is_err());
        assert_eq!(fs::read(existing).unwrap(), b"original");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_write_failure_preserves_original() {
        let (dir, path) = fixture();
        let baseline = DiskVersion::read(&path).unwrap();
        let result = save(&path, baseline.as_ref(), |file| {
            file.write_all(b"partial")?;
            Err(APP_SAVE_UNSUPPORTED)
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn external_change_is_not_overwritten() {
        let (dir, path) = fixture();
        let baseline = DiskVersion::read(&path).unwrap();
        fs::write(&path, b"external").unwrap();
        assert!(matches!(
            save(&path, baseline.as_ref(), |f| Ok(f.write_all(b"mine")?)),
            Err(APP_SAVE_CONFLICT)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"external");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn edit_during_staging_is_not_overwritten() {
        let (dir, path) = fixture();
        let baseline = DiskVersion::read(&path).unwrap();
        let result = save(&path, baseline.as_ref(), |file| {
            file.write_all(b"mine")?;
            fs::write(&path, b"concurrent")?;
            Ok(())
        });
        assert!(matches!(result, Err(APP_SAVE_CONFLICT)));
        assert_eq!(fs::read(&path).unwrap(), b"concurrent");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn commit_and_create_are_complete() {
        let (dir, path) = fixture();
        let baseline = DiskVersion::read(&path).unwrap();
        let saved = save(&path, baseline.as_ref(), |f| Ok(f.write_all(b"complete")?)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"complete");
        assert!(saved == DiskVersion::read(&path).unwrap());
        let new = dir.join("new.txt");
        save(&new, None, |f| Ok(f.write_all(b"new")?)).unwrap();
        assert_eq!(fs::read(&new).unwrap(), b"new");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hard_links_are_not_silently_severed() {
        let (dir, path) = fixture();
        fs::hard_link(&path, dir.join("linked.txt")).unwrap();
        let baseline = DiskVersion::read(&path).unwrap();
        assert!(matches!(
            save(&path, baseline.as_ref(), |f| Ok(f.write_all(b"mine")?)),
            Err(APP_SAVE_LINKED)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"original");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn readonly_target_is_not_replaced() {
        let (dir, path) = fixture();
        let original_permissions = fs::metadata(&path).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let baseline = DiskVersion::read(&path).unwrap();
        assert!(save(&path, baseline.as_ref(), |f| Ok(f.write_all(b"mine")?)).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        fs::set_permissions(&path, original_permissions).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_alternate_stream_survives_save() {
        let (dir, path) = fixture();
        let stream = PathBuf::from(format!("{}:notes", path.display()));
        fs::write(&stream, b"keep metadata").unwrap();
        let baseline = DiskVersion::read(&path).unwrap();
        save(&path, baseline.as_ref(), |f| Ok(f.write_all(b"mine")?)).unwrap();
        assert_eq!(fs::read(&stream).unwrap(), b"keep metadata");
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_saved_without_replacing_link() {
        let (dir, path) = fixture();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let baseline = DiskVersion::read(&link).unwrap();
        save(&link, baseline.as_ref(), |f| Ok(f.write_all(b"mine")?)).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&path).unwrap(), b"mine");
        fs::remove_dir_all(dir).unwrap();
    }
}
