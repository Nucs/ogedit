// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parent-directory watches survive atomic replacement. A metadata poll also covers
//! missed notifications, failed native watches and network filesystems.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ogedit::sys;

use crate::safe_save::resolved_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    Modified(PathBuf),
    Deleted(PathBuf),
}

#[derive(PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    id: Option<sys::FileId>,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
        id: sys::file_id(None, path).ok(),
    })
}

struct Inner {
    watcher: Option<RecommendedWatcher>,
    files: HashMap<PathBuf, Option<Stamp>>,
    directories: HashMap<PathBuf, usize>,
    last_poll: Instant,
}

pub struct FileWatcher {
    events: Receiver<notify::Result<notify::Event>>,
    overflow: Arc<AtomicBool>,
    inner: RefCell<Inner>,
}

const EVENT_QUEUE_CAPACITY: usize = 256;

fn queue_event(
    sender: &SyncSender<notify::Result<notify::Event>>,
    overflow: &AtomicBool,
    event: notify::Result<notify::Event>,
) {
    // Native callbacks must never block behind a busy renderer. A saturated
    // queue needs one metadata rescan, not one allocation per filesystem event.
    if let Err(mpsc::TrySendError::Full(_)) = sender.try_send(event) {
        overflow.store(true, Ordering::Release);
    }
}

impl FileWatcher {
    #[cfg(test)]
    pub(crate) fn registration_counts(&self) -> (usize, usize) {
        let inner = self.inner.borrow();
        (inner.files.len(), inner.directories.len())
    }

    pub fn new() -> Self {
        let (tx, events) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let overflow = Arc::new(AtomicBool::new(false));
        let worker_overflow = overflow.clone();
        let watcher = notify::recommended_watcher(move |event| {
            queue_event(&tx, &worker_overflow, event);
        })
        .ok();
        Self {
            events,
            overflow,
            inner: RefCell::new(Inner {
                watcher,
                files: HashMap::new(),
                directories: HashMap::new(),
                last_poll: Instant::now(),
            }),
        }
    }

    pub fn watch(&self, path: &Path) {
        let path = resolved_path(path);
        let mut inner = self.inner.borrow_mut();
        if inner.files.contains_key(&path) {
            inner.files.insert(path.clone(), stamp(&path));
            return;
        }
        if let Some(parent) = path.parent() {
            let count = inner.directories.entry(parent.to_owned()).or_default();
            *count += 1;
            if *count == 1
                && let Some(watcher) = &mut inner.watcher
            {
                // Failure is safe: all files still participate in the polling fallback.
                let _ = watcher.watch(parent, RecursiveMode::NonRecursive);
            }
        }
        inner.files.insert(path.clone(), stamp(&path));
    }

    pub fn unwatch(&self, path: &Path) {
        let path = resolved_path(path);
        let mut inner = self.inner.borrow_mut();
        if inner.files.remove(&path).is_none() {
            return;
        }
        if let Some(parent) = path.parent()
            && let Some(count) = inner.directories.get_mut(parent)
        {
            *count -= 1;
            if *count == 0 {
                inner.directories.remove(parent);
                if let Some(watcher) = &mut inner.watcher {
                    let _ = watcher.unwatch(parent);
                }
            }
        }
    }

    pub fn poll_events(&self) -> Vec<WatchEvent> {
        let mut inner = self.inner.borrow_mut();
        let mut touched = HashSet::new();
        let mut rescan = self.overflow.swap(false, Ordering::AcqRel)
            || inner.last_poll.elapsed() >= Duration::from_millis(500);
        for _ in 0..EVENT_QUEUE_CAPACITY {
            let Ok(event) = self.events.try_recv() else { break };
            match event {
                Ok(event) => {
                    if event.kind.is_access() {
                        continue;
                    }
                    if event.need_rescan() {
                        rescan = true;
                    }
                    for path in event.paths {
                        let path = resolved_path(&path);
                        if inner.files.contains_key(&path) {
                            touched.insert(path);
                        }
                    }
                }
                Err(_) => rescan = true,
            }
        }
        if rescan {
            inner.last_poll = Instant::now();
        }
        let mut events = Vec::new();
        for (path, previous) in &mut inner.files {
            if !rescan && !touched.contains(path) {
                continue;
            }
            let current = stamp(path);
            let changed = *previous != current;
            // Native writes can occur without a distinguishable timestamp on some
            // filesystems. Let the document compare its baseline before warning.
            if changed || (current.is_some() && touched.contains(path)) {
                events.push(if current.is_some() {
                    WatchEvent::Modified(path.clone())
                } else {
                    WatchEvent::Deleted(path.clone())
                });
            }
            *previous = current;
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ogedit-watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        fs::write(&path, "original").unwrap();
        (dir, resolved_path(&path))
    }

    fn wait(watcher: &FileWatcher, deleted: bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if watcher
                .poll_events()
                .iter()
                .any(|event| matches!(event, WatchEvent::Deleted(_)) == deleted)
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn native_event_storm_is_bounded_and_overflow_rescans_watched_files() {
        let (dir, path) = fixture();
        let (sender, events) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let overflow = Arc::new(AtomicBool::new(false));
        let watcher = FileWatcher {
            events,
            overflow: overflow.clone(),
            inner: RefCell::new(Inner {
                watcher: None,
                files: HashMap::new(),
                directories: HashMap::new(),
                last_poll: Instant::now(),
            }),
        };
        watcher.watch(&path);
        for _ in 0..20_000 {
            queue_event(
                &sender,
                &overflow,
                Ok(notify::Event::new(notify::EventKind::Any).add_path(dir.join("unrelated"))),
            );
        }
        assert!(overflow.load(Ordering::Acquire));
        fs::write(&path, "changed while the queue was full").unwrap();
        assert_eq!(watcher.poll_events(), [WatchEvent::Modified(path.clone())]);
        assert!(!overflow.load(Ordering::Acquire));
        assert!(watcher.events.try_recv().is_err());
        watcher.unwatch(&path);
        assert_eq!(watcher.registration_counts(), (0, 0));
        assert!(watcher.inner.borrow().directories.is_empty());
        assert!(watcher.inner.borrow().files.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn first_change_after_registration_is_reported() {
        let (dir, path) = fixture();
        let watcher = FileWatcher::new();
        watcher.watch(&path);
        fs::write(&path, "changed immediately").unwrap();
        assert!(wait(&watcher, false));
        drop(watcher);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_replacement_remains_watched() {
        let (dir, path) = fixture();
        let watcher = FileWatcher::new();
        watcher.watch(&path);
        for value in [b"first".as_slice(), b"second replacement"] {
            use std::io::Write;
            atomicwrites::AtomicFile::new(&path, atomicwrites::AllowOverwrite)
                .write(|f| f.write_all(value))
                .unwrap();
            assert!(wait(&watcher, false));
        }
        fs::remove_file(&path).unwrap();
        assert!(wait(&watcher, true));
        fs::write(&path, "recreated").unwrap();
        assert!(wait(&watcher, false));
        drop(watcher);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unwatch_deleted_path_and_other_file_same_directory() {
        let (dir, path) = fixture();
        let other = dir.join("other.txt");
        fs::write(&other, "other").unwrap();
        let watcher = FileWatcher::new();
        watcher.watch(&path);
        watcher.watch(&other);
        fs::remove_file(&path).unwrap();
        watcher.unwatch(&path);
        fs::write(&other, "changed").unwrap();
        assert!(wait(&watcher, false));
        assert!(
            !watcher
                .poll_events()
                .iter()
                .any(|e| matches!(e, WatchEvent::Deleted(p) if *p == path))
        );
        drop(watcher);
        fs::remove_dir_all(dir).unwrap();
    }
}
