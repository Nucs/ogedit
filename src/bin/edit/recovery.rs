// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private, checksummed snapshots of unsaved buffers. Each running editor holds
//! an OS lock on its own session; other instances only offer abandoned sessions.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::hash::Hasher;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ogedit::buffer::TextBuffer;
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 8] = b"OGREC001";
const MAX_METADATA: usize = 65536;

/// A stable on-disk checksum, deliberately independent of Rust's DefaultHasher.
struct SnapshotHash(u64);
impl Default for SnapshotHash {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}
impl Hasher for SnapshotHash {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ byte as u64).wrapping_mul(0x100000001b3);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryMetadata {
    pub filename: String,
    pub original_path: Option<PathBuf>,
    pub encoding: String,
    pub newline_crlf: bool,
    pub cursor_x: i64,
    pub cursor_y: i64,
    pub saved_at_secs: u64,
    pub content_len: u64,
}

#[derive(Clone, Debug)]
pub struct RecoveryEntry {
    pub metadata: RecoveryMetadata,
    path: PathBuf,
}

pub struct RecoveryStore {
    directory: PathBuf,
    _lease: File,
    abandoned_leases: HashMap<PathBuf, File>,
    pub entries: Vec<RecoveryEntry>,
    pub warnings: Vec<String>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Incomplete or damaged recovery snapshot")
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

impl RecoveryStore {
    pub fn new(root: &Path) -> io::Result<Self> {
        private_directory(root)?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let directory = root.join(format!("{}-{nonce}", std::process::id()));
        private_directory(&directory)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.join("lease"))?;
        lease.lock()?;
        let mut store = Self {
            directory,
            _lease: lease,
            abandoned_leases: HashMap::new(),
            entries: Vec::new(),
            warnings: Vec::new(),
        };
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            if !kind.is_dir() || entry.path() == store.directory {
                continue;
            }
            let lease_path = entry.path().join("lease");
            let abandoned = match OpenOptions::new().read(true).write(true).open(lease_path) {
                Ok(file) => file,
                Err(_) => continue,
            };
            store.collect_abandoned(entry.path(), abandoned)?;
        }
        store.entries.sort_by_key(|entry| std::cmp::Reverse(entry.metadata.saved_at_secs));
        Ok(store)
    }

    fn collect_abandoned(&mut self, directory: PathBuf, lease: File) -> io::Result<()> {
        if lease.try_lock().is_err() {
            return Ok(());
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            // Another instance can remove the session after we open its lease
            // but before we acquire its lock. That stale handle still locks on
            // some platforms; it must not disable recovery for other sessions.
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        };
        let mut snapshots = 0;
        for snapshot in entries {
            let snapshot = snapshot?;
            if snapshot.path().extension().is_none_or(|ext| ext != "recovery") {
                continue;
            }
            let kind = match snapshot.file_type() {
                Ok(kind) => kind,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            if !kind.is_file() {
                continue;
            }
            snapshots += 1;
            match inspect(&snapshot.path()) {
                Ok(metadata) => {
                    self.entries.push(RecoveryEntry { metadata, path: snapshot.path() })
                }
                Err(_) => self.warnings.push(format!(
                    "A damaged recovery snapshot was left untouched: {}",
                    snapshot.path().display()
                )),
            }
        }
        if snapshots == 0 && only_lease_remains(&directory) {
            remove_empty_session(&directory, lease);
        } else {
            self.abandoned_leases.insert(directory, lease);
        }
        Ok(())
    }

    pub fn checkpoint(
        &self,
        id: u64,
        filename: &str,
        original_path: Option<&Path>,
        buffer: &TextBuffer,
    ) -> io::Result<()> {
        let cursor = buffer.cursor_logical_pos();
        let metadata = RecoveryMetadata {
            filename: filename.into(),
            original_path: original_path.map(Path::to_owned),
            encoding: buffer.encoding().into(),
            newline_crlf: buffer.is_crlf(),
            cursor_x: cursor.x as i64,
            cursor_y: cursor.y as i64,
            saved_at_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            content_len: buffer.text_length() as u64,
        };
        self.write(id, &metadata, |file, hash| {
            let mut offset = 0;
            loop {
                let chunk = buffer.read_forward(offset);
                if chunk.is_empty() {
                    break;
                }
                file.write_all(chunk)?;
                hash.write(chunk);
                offset += chunk.len();
            }
            Ok(())
        })
    }

    fn write<F>(&self, id: u64, metadata: &RecoveryMetadata, write_content: F) -> io::Result<()>
    where
        F: FnOnce(&mut File, &mut SnapshotHash) -> io::Result<()>,
    {
        let json = serde_json::to_vec(metadata)?;
        if json.len() > MAX_METADATA {
            return Err(invalid());
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        atomicwrites::AtomicFile::new(self.snapshot_path(id), atomicwrites::AllowOverwrite)
            .write_with_options(
                |file| {
                    file.write_all(MAGIC)?;
                    file.write_all(&(json.len() as u32).to_le_bytes())?;
                    file.write_all(&json)?;
                    let mut hash = SnapshotHash::default();
                    hash.write(&json);
                    write_content(file, &mut hash)?;
                    file.write_all(&hash.finish().to_le_bytes())?;
                    Ok(())
                },
                options,
            )
            .map_err(io::Error::from)
    }

    pub fn remove(&self, id: u64) -> io::Result<()> {
        remove_if_present(&self.snapshot_path(id))
    }
    pub fn snapshot_path(&self, id: u64) -> PathBuf {
        self.directory.join(format!("{id}.recovery"))
    }

    /// Read only after selection. Inspection has already validated the file, but
    /// validate again in case it changed while the chooser was open.
    pub fn read_entry(&self, index: usize) -> io::Result<(RecoveryMetadata, Vec<u8>)> {
        let entry = self.entries.get(index).ok_or_else(invalid)?;
        let metadata = inspect(&entry.path)?;
        let mut file = File::open(&entry.path)?;
        let (_, offset) = header(&mut file)?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(offset))?;
        let mut content = Vec::new();
        content
            .try_reserve_exact(usize::try_from(metadata.content_len).map_err(|_| invalid())?)
            .map_err(|_| {
                io::Error::new(io::ErrorKind::OutOfMemory, "Recovery buffer is too large")
            })?;
        file.take(metadata.content_len).read_to_end(&mut content)?;
        if content.len() as u64 != metadata.content_len {
            return Err(invalid());
        }
        Ok((metadata, content))
    }

    pub fn discard_entry(&mut self, index: usize) -> io::Result<()> {
        let entry = self.entries.get(index).ok_or_else(invalid)?;
        remove_if_present(&entry.path)?;
        let directory = entry.path.parent().map(Path::to_owned);
        self.entries.remove(index);
        if let Some(directory) = directory
            && only_lease_remains(&directory)
            && let Some(lease) = self.abandoned_leases.remove(&directory)
        {
            // No snapshots remain to protect in this abandoned session. Retaining
            // its lock until app exit leaks a handle for every discarded session.
            remove_empty_session(&directory, lease);
        }
        Ok(())
    }
}

fn remove_empty_session(directory: &Path, lease: File) {
    // Keep exclusive ownership until paths are gone. A contender that already
    // opened the lease can still acquire a stale handle later; collection also
    // treats that now-missing directory as a completed cleanup.
    let _ = fs::remove_file(directory.join("lease"));
    let _ = fs::remove_dir(directory);
    drop(lease);
}

impl Drop for RecoveryStore {
    fn drop(&mut self) {
        // Explicit saves/discards remove snapshots. Only a now-empty session may
        // be removed; dirty snapshots survive unwinding and abnormal exits.
        if only_lease_remains(&self.directory) {
            let _ = fs::remove_file(self.directory.join("lease"));
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn only_lease_remains(directory: &Path) -> bool {
    fs::read_dir(directory).is_ok_and(|entries| {
        entries.into_iter().all(|entry| entry.is_ok_and(|entry| entry.file_name() == "lease"))
    })
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn header(file: &mut File) -> io::Result<(Vec<u8>, u64)> {
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid());
    }
    let mut length = [0; 4];
    file.read_exact(&mut length)?;
    let len = u32::from_le_bytes(length) as usize;
    if len > MAX_METADATA {
        return Err(invalid());
    }
    let mut json = vec![0; len];
    file.read_exact(&mut json)?;
    Ok((json, 12 + len as u64))
}

fn inspect(path: &Path) -> io::Result<RecoveryMetadata> {
    let mut file = File::open(path)?;
    let (json, offset) = header(&mut file)?;
    let metadata: RecoveryMetadata = serde_json::from_slice(&json)?;
    if metadata.content_len.checked_add(offset).and_then(|n| n.checked_add(8))
        != Some(file.metadata()?.len())
    {
        return Err(invalid());
    }
    let mut hash = SnapshotHash::default();
    hash.write(&json);
    let mut remaining = metadata.content_len;
    let mut bytes = [0; 65536];
    while remaining > 0 {
        let wanted = remaining.min(bytes.len() as u64) as usize;
        file.read_exact(&mut bytes[..wanted])?;
        hash.write(&bytes[..wanted]);
        remaining -= wanted as u64;
    }
    let mut checksum = [0; 8];
    file.read_exact(&mut checksum)?;
    if u64::from_le_bytes(checksum) != hash.finish() {
        return Err(invalid());
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ogedit-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn write(store: &RecoveryStore, id: u64) {
        let bytes = b"unsaved\r\n\xff\0";
        store
            .write(
                id,
                &RecoveryMetadata {
                    filename: "notes.txt".into(),
                    original_path: None,
                    encoding: "UTF-8".into(),
                    newline_crlf: true,
                    cursor_x: 3,
                    cursor_y: 1,
                    saved_at_secs: 7,
                    content_len: bytes.len() as u64,
                },
                |file, hash| {
                    file.write_all(bytes)?;
                    hash.write(bytes);
                    Ok(())
                },
            )
            .unwrap();
    }

    #[test]
    fn live_sessions_are_isolated_abandoned_sessions_recover_exact_bytes() {
        let root = root();
        let first = RecoveryStore::new(&root).unwrap();
        write(&first, 1);
        let second = RecoveryStore::new(&root).unwrap();
        assert!(second.entries.is_empty());
        drop(second);
        drop(first);
        let recovered = RecoveryStore::new(&root).unwrap();
        assert_eq!(recovered.entries.len(), 1);
        assert_eq!(recovered.read_entry(0).unwrap().1, b"unsaved\r\n\xff\0");
        let third = RecoveryStore::new(&root).unwrap();
        assert!(third.entries.is_empty());
        drop(third);
        drop(recovered);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_or_discarded_snapshots_do_not_return() {
        let root = root();
        let first = RecoveryStore::new(&root).unwrap();
        write(&first, 1);
        first.remove(1).unwrap();
        drop(first);
        let next = RecoveryStore::new(&root).unwrap();
        assert!(next.entries.is_empty());
        drop(next);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discarding_abandoned_sessions_releases_their_file_handles() {
        let root = root();
        for _ in 0..32 {
            let store = RecoveryStore::new(&root).unwrap();
            write(&store, 1);
        }
        let mut recovered = RecoveryStore::new(&root).unwrap();
        assert_eq!(recovered.entries.len(), 32);
        assert_eq!(recovered.abandoned_leases.len(), 32);
        for remaining in (0..32).rev() {
            recovered.discard_entry(0).unwrap();
            assert_eq!(recovered.abandoned_leases.len(), remaining);
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        drop(recovered);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrently_opened_stale_lease_does_not_disable_recovery() {
        let root = root();
        let original = RecoveryStore::new(&root).unwrap();
        let directory = original.directory.clone();
        write(&original, 1);
        drop(original);
        // Reproduce the scheduling gap deterministically: one instance opens
        // the old lease, then another claims and fully discards that session.
        let stale_lease =
            OpenOptions::new().read(true).write(true).open(directory.join("lease")).unwrap();
        let mut recovered = RecoveryStore::new(&root).unwrap();
        recovered.discard_entry(0).unwrap();
        assert!(!directory.exists());
        let mut contender = RecoveryStore::new(&root).unwrap();
        contender.collect_abandoned(directory, stale_lease).unwrap();
        assert!(contender.entries.is_empty());
        assert!(contender.abandoned_leases.is_empty());
        // The surviving instance can still checkpoint and recover new work.
        write(&contender, 2);
        drop(contender);
        let next = RecoveryStore::new(&root).unwrap();
        assert_eq!(next.entries.len(), 1);
        assert_eq!(next.read_entry(0).unwrap().1, b"unsaved\r\n\xff\0");
        drop(next);
        drop(recovered);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_and_partial_snapshots_are_left_untouched_and_reported() {
        let root = root();
        let first = RecoveryStore::new(&root).unwrap();
        write(&first, 1);
        let path = first.snapshot_path(1);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        fs::write(first.snapshot_path(2), b"OGREC001").unwrap();
        drop(first);
        let next = RecoveryStore::new(&root).unwrap();
        assert!(next.entries.is_empty());
        assert_eq!(next.warnings.len(), 2);
        assert!(path.exists());
        drop(next);
        fs::remove_dir_all(root).unwrap();
    }
}
