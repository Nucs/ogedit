// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Workspace search and guarded edits. Workers never mutate editor buffers or files.
//!
//! Byte ranges refer to the captured UTF-8 source, not the rendered result text. A
//! caller must validate the edit immediately before applying it as one undo step.
//! A disk UTF-8 BOM is excluded, matching TextBuffer's logical document text;
//! the buffer's encoding setting preserves the BOM when that document is saved.

use std::collections::HashSet;
use std::io::{self, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};

#[derive(Clone, Debug)]
pub struct OpenDocumentSnapshot {
    pub path: PathBuf,
    pub text: String,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub path: PathBuf,
    pub text: Arc<str>,
    /// Disk results have no editor revision until opened.
    pub revision: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub regex: bool,
    pub case_sensitive: bool,
    pub include_hidden: bool,
    pub max_results: usize,
    pub max_file_bytes: usize,
    pub max_snapshot_bytes: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            regex: false,
            case_sensitive: false,
            include_hidden: false,
            max_results: 10_000,
            max_file_bytes: 8 * 1024 * 1024,
            max_snapshot_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub source: Arc<SourceSnapshot>,
    /// One-based source line number.
    pub line_number: u64,
    /// The whole source line, without its original CRLF/LF terminator.
    pub line_range: Range<usize>,
    /// Every match on this line, in source byte coordinates.
    pub matches: Vec<Range<usize>>,
}

impl SearchHit {
    pub fn line_text(&self) -> &str {
        &self.source.text[self.line_range.clone()]
    }

    pub fn prepare_line_edit(
        &self,
        current: &str,
        revision: Option<u64>,
        replacement: &str,
    ) -> Result<PreparedEdit, String> {
        if replacement.contains(['\r', '\n']) {
            return Err(
                "Edit one result line at a time; use the source editor to insert lines.".into()
            );
        }
        self.prepare(current, revision, self.line_range.clone(), replacement)
    }

    #[allow(
        dead_code,
        reason = "Guarded match-level API is also available to future search actions."
    )]
    pub fn prepare_match_edit(
        &self,
        current: &str,
        revision: Option<u64>,
        match_index: usize,
        replacement: &str,
    ) -> Result<PreparedEdit, String> {
        let range = self.matches.get(match_index).ok_or("That match no longer exists.")?;
        self.prepare(current, revision, range.clone(), replacement)
    }

    fn prepare(
        &self,
        current: &str,
        revision: Option<u64>,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<PreparedEdit, String> {
        let edit = PreparedEdit::new(
            self.source.text.clone(),
            self.source.revision,
            range,
            replacement.to_owned(),
        )?;
        edit.validate(current, revision)?;
        Ok(edit)
    }
}

/// A replacement that can only apply to exactly the version used to derive it.
#[derive(Clone, Debug)]
pub struct PreparedEdit {
    pub range: Range<usize>,
    pub replacement: String,
    pub revision: Option<u64>,
    expected_source: Arc<str>,
}

impl PreparedEdit {
    pub fn new(
        expected_source: Arc<str>,
        revision: Option<u64>,
        range: Range<usize>,
        replacement: String,
    ) -> Result<Self, String> {
        if expected_source.get(range.clone()).is_none() {
            return Err("The edit does not cover a valid UTF-8 source range.".into());
        }
        Ok(Self { range, replacement, revision, expected_source })
    }

    pub fn validate(&self, current: &str, revision: Option<u64>) -> Result<(), String> {
        // Exact content comparison is deliberate: timestamps and hashes alone
        // cannot prove that a byte offset still identifies the same content.
        if self.revision.is_some() && self.revision != revision || current != &*self.expected_source
        {
            return Err("The source changed. Refresh this view before editing.".into());
        }
        if current.get(self.range.clone()).is_none() {
            return Err("The edit does not cover a valid UTF-8 source range.".into());
        }
        Ok(())
    }

    /// Useful for preview and for callers that will commit a whole new document.
    pub fn apply_to(&self, current: &str, revision: Option<u64>) -> Result<String, String> {
        self.validate(current, revision)?;
        let mut result = current.to_owned();
        result.replace_range(self.range.clone(), &self.replacement);
        Ok(result)
    }
}

#[derive(Debug, Default)]
pub struct SearchReport {
    pub hits: Vec<SearchHit>,
    pub matches_found: usize,
    pub files_searched: usize,
    pub skipped_files: usize,
    pub cancelled: bool,
    pub truncated: bool,
    /// Bounded diagnostics: a large inaccessible tree cannot flood the UI.
    pub warnings: Vec<String>,
}

impl SearchReport {
    fn warn(&mut self, warning: String) {
        if self.warnings.len() < 20 {
            self.warnings.push(warning);
        }
    }
}

pub struct SearchTask {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<SearchReport, String>>,
    finished: bool,
}

static WORKSPACE_WORKERS: AtomicUsize = AtomicUsize::new(0);

/// Cancellation cannot interrupt a blocked OS filesystem call. Bound the
/// combined discovery/search workers so repeatedly replacing tasks cannot
/// accumulate threads and captured buffers on a stalled network mount.
pub(crate) struct WorkspaceWorker(&'static AtomicUsize);

impl WorkspaceWorker {
    pub(crate) fn acquire() -> Result<Self, String> {
        Self::acquire_from(&WORKSPACE_WORKERS)
            .ok_or_else(|| "Previous workspace scans are still stopping. Try again shortly.".into())
    }

    fn acquire_from(active: &'static AtomicUsize) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(active))
    }
}

impl Drop for WorkspaceWorker {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

impl SearchTask {
    pub fn start(
        root: PathBuf,
        query: String,
        options: SearchOptions,
        snapshots: Vec<OpenDocumentSnapshot>,
    ) -> Result<Self, String> {
        // Compile on the caller so a malformed pattern reports immediately.
        let matcher = build_matcher(&query, &options)?;
        let worker = WorkspaceWorker::acquire()?;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("workspace-search".into())
            .spawn(move || {
                let _worker = worker;
                let report =
                    search_with_matcher(&root, &matcher, &options, snapshots, &worker_cancel);
                let _ = sender.send(report);
            })
            .map_err(|err| format!("Cannot start search: {err}"))?;
        Ok(Self { cancel, receiver, finished: false })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn try_finish(&mut self) -> Option<Result<SearchReport, String>> {
        if self.finished {
            return None;
        }
        match self.receiver.try_recv() {
            Ok(report) => {
                self.finished = true;
                Some(report)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.finished = true;
                Some(Err("Search stopped unexpectedly.".into()))
            }
        }
    }
}

impl Drop for SearchTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
pub fn search_workspace(
    root: &Path,
    query: &str,
    options: &SearchOptions,
    snapshots: Vec<OpenDocumentSnapshot>,
    cancel: &AtomicBool,
) -> Result<SearchReport, String> {
    let matcher = build_matcher(query, options)?;
    search_with_matcher(root, &matcher, options, snapshots, cancel)
}

fn build_matcher(query: &str, options: &SearchOptions) -> Result<RegexMatcher, String> {
    if query.is_empty() {
        return Err("Enter text to search for.".into());
    }
    if query.len() > 16 * 1024 {
        return Err("Search patterns are limited to 16 KiB.".into());
    }
    if options.max_results == 0 || options.max_file_bytes == 0 || options.max_snapshot_bytes == 0 {
        return Err("Search limits must be greater than zero.".into());
    }
    RegexMatcherBuilder::new()
        .fixed_strings(!options.regex)
        .case_insensitive(!options.case_sensitive)
        .line_terminator(Some(b'\n'))
        .multi_line(true)
        .crlf(true)
        .size_limit(8 * 1024 * 1024)
        .build(query)
        .map_err(|err| format!("Invalid search: {err}"))
}

fn identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        // A new, named document need not exist yet. Resolve its existing parent
        // so Windows verbatim paths and relative paths still compare correctly.
        if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
            && let Ok(parent) = std::fs::canonicalize(parent)
        {
            return parent.join(name);
        }
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
}

fn search_with_matcher(
    root: &Path,
    matcher: &RegexMatcher,
    options: &SearchOptions,
    snapshots: Vec<OpenDocumentSnapshot>,
    cancel: &AtomicBool,
) -> Result<SearchReport, String> {
    if !root.is_dir() {
        return Err(format!("Search folder does not exist: {}", root.display()));
    }
    let root = identity(root);
    let mut report = SearchReport::default();
    let mut visited = HashSet::new();
    let mut retained_bytes = 0;

    // Explicitly opened files in this workspace are included even if ignored.
    // Their unsaved text must take precedence over stale bytes on disk.
    for snapshot in snapshots {
        if cancel.load(Ordering::Relaxed) || report.truncated {
            break;
        }
        let path = identity(&snapshot.path);
        if !path.starts_with(&root) || !visited.insert(path.clone()) {
            continue;
        }
        search_source(
            SourceSnapshot { path, text: snapshot.text.into(), revision: Some(snapshot.revision) },
            matcher,
            options,
            cancel,
            &mut report,
            &mut retained_bytes,
        )?;
    }

    let walker = ignore::WalkBuilder::new(&root)
        .hidden(!options.include_hidden)
        .follow_links(false)
        .require_git(false)
        .build();
    for entry in walker {
        if cancel.load(Ordering::Relaxed) || report.truncated {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                report.warn(err.to_string());
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = identity(entry.path());
        // Only open-buffer snapshots need deduplication. Walk does not follow
        // directory symlinks, so retaining every visited disk path needlessly
        // made a no-match scan grow with the entire workspace.
        if visited.contains(&path) {
            continue;
        }
        let contents = (|| -> io::Result<Vec<u8>> {
            let file = std::fs::File::open(&path)?;
            if file.metadata()?.len() > options.max_file_bytes as u64 {
                return Ok(Vec::new());
            }
            let mut bytes = Vec::new();
            // A file can grow after metadata() so bound the read itself too.
            file.take(options.max_file_bytes as u64 + 1).read_to_end(&mut bytes)?;
            Ok(bytes)
        })();
        let bytes = match contents {
            Ok(bytes) => bytes,
            Err(err) => {
                report.skipped_files += 1;
                report.warn(format!("{}: {err}", path.display()));
                continue;
            }
        };
        if bytes.is_empty() || bytes.len() > options.max_file_bytes || bytes.contains(&0) {
            report.skipped_files += 1;
            continue;
        }
        let mut text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                report.skipped_files += 1;
                continue;
            }
        };
        if text.starts_with('\u{feff}') {
            text.drain(..3);
        }
        search_source(
            SourceSnapshot { path, text: text.into(), revision: None },
            matcher,
            options,
            cancel,
            &mut report,
            &mut retained_bytes,
        )?;
    }
    report.cancelled = cancel.load(Ordering::Relaxed);
    Ok(report)
}

fn search_source(
    source: SourceSnapshot,
    matcher: &RegexMatcher,
    options: &SearchOptions,
    cancel: &AtomicBool,
    report: &mut SearchReport,
    retained_bytes: &mut usize,
) -> Result<(), String> {
    if source.text.len() > options.max_file_bytes || source.text.as_bytes().contains(&0) {
        report.skipped_files += 1;
        return Ok(());
    }
    if retained_bytes.saturating_add(source.text.len()) > options.max_snapshot_bytes {
        report.truncated = true;
        report.warn("Search snapshot limit reached; narrow the search folder.".into());
        return Ok(());
    }
    let before = report.hits.len();
    let source = Arc::new(source);
    report.files_searched += 1;
    // Text is already normalized to the editor's UTF-8 representation. Do not
    // let grep apply any additional BOM detection or encoding transformation.
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .line_terminator(grep_matcher::LineTerminator::crlf())
        .bom_sniffing(false)
        .build();
    let mut sink = ResultSink { source: source.clone(), matcher, options, cancel, report };
    searcher
        .search_slice(matcher, source.text.as_bytes(), &mut sink)
        .map_err(|err| format!("{}: {err}", source.path.display()))?;
    if report.hits.len() != before {
        *retained_bytes += source.text.len();
    }
    Ok(())
}

struct ResultSink<'a> {
    source: Arc<SourceSnapshot>,
    matcher: &'a RegexMatcher,
    options: &'a SearchOptions,
    cancel: &'a AtomicBool,
    report: &'a mut SearchReport,
}

impl Sink for ResultSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, found: &SinkMatch<'_>) -> io::Result<bool> {
        if self.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if self.report.matches_found >= self.options.max_results {
            self.report.truncated = true;
            return Ok(false);
        }
        let start = found.absolute_byte_offset() as usize;
        let mut end = start + found.bytes().len();
        if self.source.text.as_bytes().get(end.wrapping_sub(1)) == Some(&b'\n') {
            end -= 1;
            if self.source.text.as_bytes().get(end.wrapping_sub(1)) == Some(&b'\r') {
                end -= 1;
            }
        }
        let line = &self.source.text[start..end];
        let mut matches = Vec::new();
        self.matcher
            .find_iter(line.as_bytes(), |m| {
                if self.report.matches_found + matches.len() >= self.options.max_results {
                    self.report.truncated = true;
                    return false;
                }
                // Regex byte mode may explicitly match half of a UTF-8 character.
                // Such ranges cannot be edited safely in a text editor.
                if line.is_char_boundary(m.start()) && line.is_char_boundary(m.end()) {
                    matches.push(start + m.start()..start + m.end());
                }
                !self.cancel.load(Ordering::Relaxed)
            })
            .map_err(io::Error::other)?;
        if !matches.is_empty() {
            self.report.matches_found += matches.len();
            self.report.hits.push(SearchHit {
                source: self.source.clone(),
                line_number: found.line_number().unwrap_or(1),
                line_range: start..end,
                matches,
            });
        }
        Ok(!self.report.truncated)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ogedit-search-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn worker_permits_bound_live_tasks_and_are_reusable_after_drop() {
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        let first = WorkspaceWorker::acquire_from(&ACTIVE).unwrap();
        let second = WorkspaceWorker::acquire_from(&ACTIVE).unwrap();
        assert!(WorkspaceWorker::acquire_from(&ACTIVE).is_none());
        drop(first);
        let replacement = WorkspaceWorker::acquire_from(&ACTIVE).unwrap();
        assert_eq!(ACTIVE.load(Ordering::Acquire), 2);
        drop(second);
        drop(replacement);
        assert_eq!(ACTIVE.load(Ordering::Acquire), 0);
    }

    #[test]
    fn repeatedly_cancelled_searches_release_worker_and_snapshot_ownership() {
        let dir = TestDir::new();
        let file = dir.write("source.txt", "disk");
        for _ in 0..48 {
            let task = SearchTask::start(
                dir.0.clone(),
                "match".into(),
                SearchOptions::default(),
                vec![OpenDocumentSnapshot {
                    path: file.clone(),
                    text: "match\n".repeat(10_000),
                    revision: 1,
                }],
            )
            .unwrap();
            let weak_cancel = Arc::downgrade(&task.cancel);
            drop(task);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while weak_cancel.upgrade().is_some() {
                assert!(std::time::Instant::now() < deadline, "cancelled worker did not exit");
                std::thread::yield_now();
            }
        }
    }

    #[test]
    fn unicode_and_crlf_ranges_are_source_bytes() {
        let dir = TestDir::new();
        let original = "first\r\nשלום café café\r\nlast";
        dir.write("unicode.txt", original);
        let report = search_workspace(
            &dir.0,
            "café",
            &SearchOptions::default(),
            vec![],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.hits.len(), 1);
        let hit = &report.hits[0];
        assert_eq!(hit.line_number, 2);
        assert_eq!(hit.line_text(), "שלום café café");
        assert_eq!(hit.matches.len(), 2);
        assert_eq!(&original[hit.matches[1].clone()], "café");
        let edit = hit.prepare_line_edit(original, None, "changed").unwrap();
        assert_eq!(edit.apply_to(original, None).unwrap(), "first\r\nchanged\r\nlast");
    }

    #[test]
    fn open_unsaved_buffers_override_disk_and_stale_edits_fail() {
        let dir = TestDir::new();
        let path = dir.write("open.txt", "disk only");
        let text = "unsaved match\n";
        let report = search_workspace(
            &dir.0,
            "match",
            &SearchOptions::default(),
            vec![OpenDocumentSnapshot { path, text: text.into(), revision: 7 }],
            &AtomicBool::new(false),
        )
        .unwrap();
        let hit = &report.hits[0];
        assert!(hit.prepare_line_edit(text, Some(8), "new").is_err());
        assert!(hit.prepare_line_edit("externally changed", Some(7), "new").is_err());
        let edit = hit.prepare_line_edit(text, Some(7), "new").unwrap();
        assert!(edit.apply_to(text, Some(8)).is_err());
        assert_eq!(edit.apply_to(text, Some(7)).unwrap(), "new\n");
        assert_eq!(report.files_searched, 1);
    }

    #[test]
    fn ignore_limits_regex_and_cancellation() {
        let dir = TestDir::new();
        dir.write(".gitignore", "ignored.txt\n");
        dir.write("ignored.txt", "match");
        dir.write("shown.txt", "MATCH\nmatch\nmatch\n");
        let options = SearchOptions { max_results: 2, regex: true, ..Default::default() };
        let report =
            search_workspace(&dir.0, "^match$", &options, vec![], &AtomicBool::new(false)).unwrap();
        assert_eq!(report.hits.len(), 2);
        assert!(report.truncated);
        assert!(report.hits.iter().all(|hit| hit.source.path.ends_with("shown.txt")));
        assert!(search_workspace(&dir.0, "[", &options, vec![], &AtomicBool::new(false)).is_err());
        let cancelled =
            search_workspace(&dir.0, "match", &options, vec![], &AtomicBool::new(true)).unwrap();
        assert!(cancelled.cancelled);
        assert!(cancelled.hits.is_empty());
    }

    #[test]
    fn exact_guard_rejects_same_size_external_rewrite() {
        let edit = PreparedEdit::new(Arc::from("one\ntwo\n"), None, 4..7, "new".into()).unwrap();
        assert!(edit.apply_to("ONE\ntwo\n", None).is_err());
        assert!(PreparedEdit::new(Arc::from("é"), None, 1..2, "x".into()).is_err());
    }

    #[test]
    fn bom_search_ranges_match_the_editor_buffer() {
        let dir = TestDir::new();
        let original = "\u{feff}café\r\ncafé\n";
        dir.write("bom.txt", original);
        let report = search_workspace(
            &dir.0,
            "café",
            &SearchOptions::default(),
            vec![],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.hits.len(), 2);
        let editor_text = original.strip_prefix('\u{feff}').unwrap();
        assert_eq!(report.hits[0].matches[0], 0..5);
        assert_eq!(report.hits[1].matches[0], 7..12);
        for hit in &report.hits {
            assert_eq!(&editor_text[hit.matches[0].clone()], "café");
        }
        let edit = report.hits[1].prepare_line_edit(editor_text, Some(1), "changed").unwrap();
        assert_eq!(edit.apply_to(editor_text, Some(1)).unwrap(), "café\r\nchanged\n");
    }

    #[test]
    fn new_named_buffers_and_match_count_limit() {
        let dir = TestDir::new();
        let options = SearchOptions { max_results: 3, ..Default::default() };
        let report = search_workspace(
            &dir.0,
            "x",
            &options,
            vec![OpenDocumentSnapshot {
                path: dir.0.join("not-created-yet.txt"),
                text: "xxxxxx".into(),
                revision: 1,
            }],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.matches_found, 3);
        assert!(report.truncated);
    }
}
