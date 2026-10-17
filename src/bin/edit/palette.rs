//! Cancellable workspace discovery and background fuzzy matching.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nucleo::Nucleo;
use nucleo::pattern::{CaseMatching, Normalization};

use crate::workspace_search::WorkspaceWorker;

const MAX_INDEX_BYTES: usize = 64 * 1024 * 1024;

pub struct FileIndex {
    matcher: Nucleo<PathBuf>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    limited: Arc<AtomicBool>,
    query: String,
    matching: bool,
}

impl FileIndex {
    pub fn start(root: PathBuf) -> Result<Self, String> {
        let worker = WorkspaceWorker::acquire()?;
        // Resolve once, rather than canonicalizing every candidate on every UI
        // frame. Open document paths use the same canonical representation.
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let matcher =
            Nucleo::new(nucleo::Config::DEFAULT.match_paths(), Arc::new(|| {}), Some(2), 1);
        let injector = matcher.injector();
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let limited = Arc::new(AtomicBool::new(false));
        let (stop, finished, truncated) = (cancel.clone(), done.clone(), limited.clone());
        std::thread::Builder::new()
            .name("workspace-files".into())
            .spawn(move || {
                let _worker = worker;
                let mut count = 0;
                let mut retained_bytes = 0usize;
                for entry in ignore::WalkBuilder::new(&root).require_git(false).build() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(entry) = entry else {
                        continue;
                    };
                    if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                        continue;
                    }
                    if count == 200_000 {
                        truncated.store(true, Ordering::Relaxed);
                        break;
                    }
                    let path = entry.into_path();
                    // Account conservatively for the owned path, matching column
                    // (up to UTF-32), and per-candidate storage. The file-count cap
                    // alone is insufficient for workspaces with very long names.
                    retained_bytes = retained_bytes
                        .saturating_add(path.as_os_str().len().saturating_mul(5))
                        .saturating_add(128);
                    if retained_bytes > MAX_INDEX_BYTES {
                        truncated.store(true, Ordering::Relaxed);
                        break;
                    }
                    injector.push(path, |path, columns| {
                        columns[0] = path
                            .strip_prefix(&root)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .as_ref()
                            .into();
                    });
                    count += 1;
                }
                finished.store(true, Ordering::Release);
            })
            .map_err(|err| format!("Cannot start file discovery: {err}"))?;
        Ok(Self { matcher, cancel, done, limited, query: String::new(), matching: false })
    }

    pub fn results(&mut self, query: &str, limit: usize) -> Vec<PathBuf> {
        if query != self.query {
            self.matcher.pattern.reparse(
                0,
                query,
                CaseMatching::Smart,
                Normalization::Smart,
                false,
            );
            self.query = query.to_owned();
        }
        let status = self.matcher.tick(0);
        self.matching = status.running;
        let snapshot = self.matcher.snapshot();
        // Nucleo intentionally retains the last complete snapshot while a new
        // query runs. Never offer an old query's results for activation.
        if snapshot.pattern().column_pattern(0).atoms
            != self.matcher.pattern.column_pattern(0).atoms
        {
            self.matching = true;
            return Vec::new();
        }
        snapshot
            .matched_items(..snapshot.matched_item_count().min(limit as u32))
            .map(|item| item.data.clone())
            .collect()
    }

    pub fn status(&self) -> &'static str {
        if self.limited.load(Ordering::Relaxed) {
            "File index limit reached · narrow the workspace for more"
        } else if self.matching {
            "Matching…"
        } else if !self.done.load(Ordering::Acquire) {
            "Scanning…"
        } else {
            ""
        }
    }
}

impl Drop for FileIndex {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_query_never_returns_previous_query_matches() {
        let mut index =
            FileIndex::start(std::env::temp_dir().join("ogedit-nonexistent-index-root")).unwrap();
        let injector = index.matcher.injector();
        for name in ["alpha.rs", "beta.rs"] {
            injector.push(PathBuf::from(name), |path, cols| {
                cols[0] = path.to_str().unwrap().into();
            });
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while index.results("alpha", 10).is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        // This invocation commonly still has the alpha snapshot available.
        assert!(index.results("beta", 10).iter().all(|path| path == &PathBuf::from("beta.rs")));
        while index.results("beta", 10).is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(index.results("beta", 10), [PathBuf::from("beta.rs")]);
    }
}
