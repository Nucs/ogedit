// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ogedit::buffer::{RcTextBuffer, TextBuffer};
use ogedit::helpers::{CoordType, Point};
use ogedit::{apperr, icu, sys};

use crate::config::Config;
use crate::recovery::{RecoveryEntry, RecoveryStore};
use crate::safe_save::{self, DiskContentProbe, DiskVersion};
use crate::state::DisplayablePathBuf;
use crate::watch::WatchEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    LocalOnly,
    Unchanged,
    Modified,
    Deleted,
    Unavailable,
}

pub struct Document {
    pub buffer: RcTextBuffer,
    pub path: Option<PathBuf>,
    pub dir: Option<DisplayablePathBuf>,
    pub filename: String,
    pub file_id: Option<sys::FileId>,
    pub new_file_counter: usize,
    /// A per-document syntax override; None follows configured/path associations.
    pub language_override: Option<String>,
    configured_ruler: CoordType,

    // File change detection (lightweight - hybrid approach)
    pub last_modified: Option<SystemTime>,
    disk_version: Option<DiskVersion>,
    disk_event: Cell<bool>,
    content_changed: Cell<bool>,
    disk_probe: RefCell<Option<DiskContentProbe>>,
    next_content_check: Cell<Option<Instant>>,
    recovery_id: u64,
    recovery_generation: Option<(u64, &'static str, bool)>,
    recovery_path: Option<PathBuf>,
    recovery_error: Option<String>,
}

impl Document {
    /// Move to a one-based character/line, with negative lines counting from EOF.
    pub fn cursor_move_to_goto(&self, goto: Point) {
        let mut tb = self.buffer.borrow_mut();
        let y = if goto.y < 0 {
            tb.logical_line_count().saturating_add(goto.y)
        } else {
            goto.y.saturating_sub(1)
        };
        tb.cursor_move_to_logical(Point { x: goto.x.saturating_sub(1), y });
    }

    pub fn reload(&mut self) -> apperr::Result<()> {
        let encoding = self.buffer.borrow().encoding();
        self.reread(Some(encoding))
    }

    pub fn save(&mut self, new_path: Option<PathBuf>) -> apperr::Result<()> {
        let path =
            new_path.as_deref().or(self.path.as_deref()).ok_or(safe_save::APP_SAVE_UNSUPPORTED)?;
        let path = safe_save::resolved_path(path);
        let same_file = self.path.as_ref().is_some_and(|old| safe_save::resolved_path(old) == path);
        // Save As receives overwrite consent in the picker. Its baseline still
        // protects against a writer racing the actual serialization.
        let other_baseline = if same_file { None } else { DiskVersion::read(&path)? };
        let baseline = if same_file { self.disk_version.as_ref() } else { other_baseline.as_ref() };
        let version = safe_save::save(&path, baseline, |file| {
            self.buffer.borrow_mut().write_file_staged(file)
        })?;
        self.disk_version = version;
        self.last_modified = self.disk_version.as_ref().and_then(|version| version.modified);
        self.file_id = sys::file_id(None, &path).ok();
        self.disk_event.set(false);
        self.content_changed.set(false);
        self.disk_probe.get_mut().take();
        self.next_content_check.set(None);
        self.set_path(path);
        self.buffer.borrow_mut().mark_saved();
        self.clear_recovery();
        Ok(())
    }

    pub fn reread(&mut self, encoding: Option<&'static str>) -> apperr::Result<()> {
        let path = self.path.as_deref().ok_or(safe_save::APP_SAVE_UNSUPPORTED)?;
        let before = DiskVersion::read(path)?;
        let mut file = DocumentManager::open_for_reading(path)?;

        // Save cursor state before reload
        let (saved_pos, saved_line_content) = {
            let tb = self.buffer.borrow();
            let pos = tb.cursor_logical_pos();
            let line_content = tb.extract_line_content(pos.y);
            (pos, line_content)
        };

        // Reload the file
        let mut loaded = TextBuffer::new(false)?;
        loaded.read_file(&mut file, encoding)?;
        let after = DiskVersion::read(path)?;
        if before != after {
            return Err(safe_save::APP_SAVE_CONFLICT);
        }
        self.buffer.borrow_mut().replace_loaded_content(loaded);

        // Restore cursor position after reload
        {
            let mut tb = self.buffer.borrow_mut();
            let new_line_count = tb.logical_line_count();

            // Try to find the same line content in the new file
            let target_line = saved_line_content
                .and_then(|(content, _)| tb.nearest_matching_line(&content, saved_pos.y));

            // Calculate the target position
            let target_y =
                target_line.unwrap_or_else(|| saved_pos.y.min(new_line_count - 1).max(0));
            let target_x = saved_pos.x; // Column will be clamped by cursor_move_to_logical

            // Move cursor to the target position
            tb.cursor_move_to_logical(Point { x: target_x, y: target_y });
            tb.make_cursor_visible();
        }

        if let Ok(id) = sys::file_id(None, path) {
            self.file_id = Some(id);
        }

        self.disk_version = after;
        self.last_modified = self.disk_version.as_ref().and_then(|version| version.modified);
        self.disk_event.set(false);
        self.content_changed.set(false);
        self.disk_probe.get_mut().take();
        self.next_content_check.set(None);
        self.clear_recovery();
        Ok(())
    }

    fn set_path(&mut self, path: PathBuf) {
        let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dir = path.parent().map(ToOwned::to_owned).unwrap_or_default();
        self.filename = filename;
        self.dir = Some(DisplayablePathBuf::from_path(dir));
        self.path = Some(path);
        self.update_file_mode();
    }

    fn update_file_mode(&mut self) {
        let mut tb = self.buffer.borrow_mut();
        tb.set_ruler(if self.filename == "COMMIT_EDITMSG" { 72 } else { self.configured_ruler });
    }

    pub fn disk_status(&self) -> DiskStatus {
        self.disk_status_at(Instant::now())
    }

    fn disk_status_at(&self, now: Instant) -> DiskStatus {
        let Some(path) = &self.path else {
            return DiskStatus::LocalOnly;
        };
        match std::fs::metadata(path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.disk_probe.borrow_mut().take();
                if self.disk_version.is_some() {
                    DiskStatus::Deleted
                } else {
                    DiskStatus::Unchanged
                }
            }
            Err(_) => {
                self.disk_probe.borrow_mut().take();
                DiskStatus::Unavailable
            }
            Ok(_) => {
                let Some(version) = &self.disk_version else {
                    return DiskStatus::Modified;
                };
                if !version.metadata_matches(path) {
                    self.disk_probe.borrow_mut().take();
                    return DiskStatus::Modified;
                }
                let event = self.disk_event.replace(false);
                if event {
                    // Bytes already scanned may have changed even when mtime
                    // and size did not. Restart from the new observed event.
                    self.disk_probe.borrow_mut().take();
                }
                if event || self.next_content_check.get().is_none_or(|next| now >= next) {
                    let result = {
                        let mut probe = self.disk_probe.borrow_mut();
                        if probe.is_none() {
                            *probe = Some(DiskContentProbe::new());
                        }
                        probe.as_mut().unwrap().advance(path, version)
                    };
                    match result {
                        Ok(Some(same)) => {
                            self.disk_probe.borrow_mut().take();
                            self.content_changed.set(!same);
                            self.next_content_check.set(Some(now + Duration::from_secs(2)));
                        }
                        Ok(None) => {
                            // Large files make bounded progress without hashing
                            // their entire contents in one UI polling call.
                            self.next_content_check.set(Some(now + Duration::from_millis(100)));
                        }
                        Err(_) => {
                            self.disk_probe.borrow_mut().take();
                            self.next_content_check.set(Some(now + Duration::from_secs(2)));
                            return DiskStatus::Unavailable;
                        }
                    }
                }
                if self.content_changed.get() {
                    DiskStatus::Modified
                } else {
                    DiskStatus::Unchanged
                }
            }
        }
    }

    pub fn has_file_changed_on_disk(&self) -> bool {
        matches!(
            self.disk_status(),
            DiskStatus::Modified | DiskStatus::Deleted | DiskStatus::Unavailable
        )
    }

    /// Compare decoded disk text with local text without changing either version.
    pub fn external_diff(&self) -> apperr::Result<String> {
        let path = self.path.as_deref().ok_or(safe_save::APP_SAVE_UNSUPPORTED)?;
        let disk_len = std::fs::metadata(path).map(|metadata| metadata.len()).unwrap_or_default();
        if disk_len.saturating_add(self.buffer.borrow().text_length() as u64) > 32 * 1024 * 1024 {
            return Err(safe_save::APP_SAVE_UNSUPPORTED);
        }
        let mut disk = TextBuffer::new(false)?;
        match File::open(path) {
            Ok(mut file) => disk.read_file(&mut file, Some(self.buffer.borrow().encoding()))?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        let local = self.buffer.borrow();
        // Diff construction is intentionally bounded so comparison cannot freeze
        // the editor on a multi-gigabyte file. Saving itself has no such limit.
        if disk.text_length().saturating_add(local.text_length()) > 32 * 1024 * 1024 {
            return Err(safe_save::APP_SAVE_UNSUPPORTED);
        }
        let disk_bytes = disk.extract_bytes(0..disk.text_length());
        let local_bytes = local.extract_bytes(0..local.text_length());
        let disk_text = String::from_utf8_lossy(&disk_bytes);
        let local_text = String::from_utf8_lossy(&local_bytes);
        if disk_bytes != local_bytes && disk_text == local_text {
            let offset = disk_bytes
                .iter()
                .zip(&local_bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(disk_bytes.len().min(local_bytes.len()));
            return Ok(format!(
                "The bytes differ at offset {offset}. Invalid UTF-8 bytes have the same text preview.\n"
            ));
        }
        Ok(similar::TextDiff::configure()
            .timeout(std::time::Duration::from_millis(500))
            .diff_lines(disk_text.as_ref(), local_text.as_ref())
            .unified_diff()
            .context_radius(3)
            .header("Disk", "Your edits")
            .to_string())
    }

    fn clear_recovery(&mut self) {
        if let Some(path) = self.recovery_path.take() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    self.recovery_error = Some(err.to_string());
                    self.recovery_path = Some(path);
                }
            }
        }
        self.recovery_generation = None;
    }
}

#[derive(Default)]
pub struct DocumentManager {
    list: VecDeque<Document>,
    next_recovery_id: u64,
    recovery: Option<RecoveryStore>,
    recovery_warnings: Vec<String>,
}

impl DocumentManager {
    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.list.iter()
    }
    pub fn activate_path(&mut self, path: &Path) -> bool {
        let path = safe_save::resolved_path(path);
        self.update_active(|doc| doc.path.as_ref() == Some(&path))
    }

    pub fn record_watch_event(&mut self, event: &WatchEvent) {
        let (WatchEvent::Modified(path) | WatchEvent::Deleted(path)) = event;
        let path = safe_save::resolved_path(path);
        for doc in &mut self.list {
            if doc.path.as_ref() == Some(&path) {
                doc.disk_event.set(true);
            }
        }
    }

    pub fn init_recovery(&mut self) -> io::Result<()> {
        if self.recovery.is_none() {
            let root = Config::data_dir().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "No user data directory for recovery")
            })?;
            self.recovery = Some(RecoveryStore::new(&root.join("recovery"))?);
        }
        Ok(())
    }

    pub fn recoverable(&self) -> &[RecoveryEntry] {
        self.recovery.as_ref().map_or(&[], |store| store.entries.as_slice())
    }

    pub fn take_recovery_warnings(&mut self) -> Vec<String> {
        let mut warnings = std::mem::take(&mut self.recovery_warnings);
        if let Some(store) = &mut self.recovery {
            warnings.append(&mut store.warnings);
        }
        for doc in &mut self.list {
            if let Some(warning) = doc.recovery_error.take() {
                warnings.push(warning);
            }
        }
        warnings
    }

    /// Writes only changed generations. Each buffer is streamed directly into an
    /// atomic snapshot; no second full-size copy is needed while editing.
    pub fn checkpoint_recovery(&mut self) -> io::Result<()> {
        let Some(store) = &self.recovery else {
            return Ok(());
        };
        for doc in &mut self.list {
            let buffer = doc.buffer.borrow();
            if buffer.is_dirty() {
                let version = (buffer.content_revision(), buffer.encoding(), buffer.is_crlf());
                if doc.recovery_generation != Some(version) {
                    store.checkpoint(
                        doc.recovery_id,
                        &doc.filename,
                        doc.path.as_deref(),
                        &buffer,
                    )?;
                    doc.recovery_generation = Some(version);
                    doc.recovery_path = Some(store.snapshot_path(doc.recovery_id));
                }
            } else {
                store.remove(doc.recovery_id)?;
                doc.recovery_generation = None;
                doc.recovery_path = None;
            }
        }
        Ok(())
    }

    pub fn discard_recovery(&mut self, index: usize) -> io::Result<()> {
        self.recovery
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Recovery is unavailable"))?
            .discard_entry(index)
    }

    pub fn restore_recovery(
        &mut self,
        index: usize,
        config: &Config,
    ) -> apperr::Result<&mut Document> {
        let store = self.recovery.as_ref().ok_or(safe_save::APP_RECOVERY_INVALID)?;
        let (metadata, bytes) =
            store.read_entry(index).map_err(|_| safe_save::APP_RECOVERY_INVALID)?;
        let encoding = icu::get_available_encodings()
            .all
            .iter()
            .find(|encoding| encoding.canonical == metadata.encoding)
            .map(|encoding| encoding.canonical)
            .ok_or(safe_save::APP_RECOVERY_INVALID)?;
        let doc = self.add_untitled(config)?;
        doc.filename = format!("{} (recovered)", metadata.filename);
        {
            let mut buffer = doc.buffer.borrow_mut();
            buffer.set_insert_final_newline(false);
            if buffer.apply_byte_edits(&[(0..0, bytes)]).is_err() {
                drop(buffer);
                self.remove_active();
                return Err(safe_save::APP_RECOVERY_INVALID);
            }
            buffer.set_insert_final_newline(config.insert_final_newline);
            buffer.set_encoding(encoding);
            buffer.set_crlf(metadata.newline_crlf);
            buffer.mark_as_dirty();
            buffer.cursor_move_to_logical(Point {
                x: metadata.cursor_x.clamp(0, CoordType::MAX as i64) as CoordType,
                y: metadata.cursor_y.clamp(0, CoordType::MAX as i64) as CoordType,
            });
            buffer.make_cursor_visible();
        }
        // The old snapshot stays recoverable unless the new copy is durable.
        self.checkpoint_recovery()?;
        self.discard_recovery(index)?;
        Ok(self.active_mut().unwrap())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[inline]
    pub fn active(&self) -> Option<&Document> {
        self.list.front()
    }

    #[inline]
    pub fn active_mut(&mut self) -> Option<&mut Document> {
        self.list.front_mut()
    }

    #[inline]
    pub fn update_active<F: FnMut(&Document) -> bool>(&mut self, mut func: F) -> bool {
        if let Some(index) = self.list.iter().position(&mut func) {
            let doc = self.list.remove(index).unwrap();
            self.list.push_front(doc);
            return true;
        }
        false
    }

    pub fn remove_active(&mut self) {
        if let Some(mut doc) = self.list.pop_front() {
            doc.clear_recovery();
            if let Some(err) = doc.recovery_error.take() {
                self.recovery_warnings.push(err);
            }
        }
    }

    /// Check if a file path is currently open in any document
    pub fn is_path_open(&self, path: &Path) -> bool {
        let path = safe_save::resolved_path(path);
        self.list.iter().any(|doc| doc.path.as_ref() == Some(&path))
    }

    pub fn add_untitled(&mut self, config: &Config) -> apperr::Result<&mut Document> {
        let buffer = Self::create_buffer(config)?;
        self.next_recovery_id += 1;
        let mut doc = Document {
            buffer,
            path: None,
            dir: Default::default(),
            filename: Default::default(),
            file_id: None,
            new_file_counter: 0,
            language_override: None,
            configured_ruler: config.ruler_column as CoordType,
            last_modified: None,
            disk_version: None,
            disk_event: Cell::new(false),
            content_changed: Cell::new(false),
            disk_probe: RefCell::new(None),
            next_content_check: Cell::new(None),
            recovery_id: self.next_recovery_id,
            recovery_generation: None,
            recovery_path: None,
            recovery_error: None,
        };
        self.gen_untitled_name(&mut doc);

        self.list.push_front(doc);
        Ok(self.list.front_mut().unwrap())
    }

    pub fn gen_untitled_name(&self, doc: &mut Document) {
        let mut new_file_counter = 0;
        for doc in &self.list {
            new_file_counter = new_file_counter.max(doc.new_file_counter);
        }
        new_file_counter += 1;

        doc.filename = format!("Untitled-{new_file_counter}.txt");
        doc.new_file_counter = new_file_counter;
    }

    pub fn add_file_path(&mut self, path: &Path, config: &Config) -> apperr::Result<&mut Document> {
        // Windows filenames cannot contain quote characters. Accept the shell-
        // style wrappers commonly pasted from Explorer or terminal output. Unix
        // callers may supply real quoted filenames, so preserve those verbatim.
        #[cfg(windows)]
        let input = ogedit::path::from_user_input(path);
        #[cfg(windows)]
        let path = input.as_path();
        let (path, goto) = Self::resolve_filename_goto(path);
        self.add_file_path_with_goto(path, config, goto)
    }

    /// CLI filenames are literal unless --goto explicitly requests a suffix.
    pub fn add_file_path_literal(
        &mut self,
        path: &Path,
        config: &Config,
    ) -> apperr::Result<&mut Document> {
        self.add_file_path_with_goto(path, config, None)
    }

    fn add_file_path_with_goto(
        &mut self,
        path: &Path,
        config: &Config,
        goto: Option<Point>,
    ) -> apperr::Result<&mut Document> {
        let path = safe_save::resolved_path(path);
        // Atomic replacement changes file IDs. The open buffer still owns this
        // path and must not be shadowed by a second buffer after an external save.
        if self.update_active(|doc| doc.path.as_ref() == Some(&path)) {
            let doc = self.active_mut().unwrap();
            if let Some(goto) = goto {
                doc.cursor_move_to_goto(goto);
            }
            return Ok(doc);
        }
        let disk_version = DiskVersion::read(&path)?;

        let mut file = match Self::open_for_reading(&path) {
            Ok(file) => Some(file),
            Err(err) if sys::apperr_is_not_found(err) => None,
            Err(err) => return Err(err),
        };

        let file_id = if file.is_some() { Some(sys::file_id(file.as_ref(), &path)?) } else { None };

        // Check if the file is already open.
        if file_id.is_some() && self.update_active(|doc| doc.file_id == file_id) {
            let doc = self.active_mut().unwrap();
            if let Some(goto) = goto {
                doc.cursor_move_to_goto(goto);
            }
            return Ok(doc);
        }

        let buffer = Self::create_buffer(config)?;
        {
            if let Some(file) = &mut file {
                let mut tb = buffer.borrow_mut();
                tb.read_file(file, None)?;
            }
        }

        if disk_version != DiskVersion::read(&path)? {
            return Err(safe_save::APP_SAVE_CONFLICT);
        }

        let last_modified = disk_version.as_ref().and_then(|version| version.modified);
        self.next_recovery_id += 1;
        let mut doc = Document {
            buffer,
            path: None,
            dir: None,
            filename: Default::default(),
            file_id,
            new_file_counter: 0,
            language_override: None,
            configured_ruler: config.ruler_column as CoordType,
            last_modified,
            disk_version,
            disk_event: Cell::new(false),
            content_changed: Cell::new(false),
            disk_probe: RefCell::new(None),
            next_content_check: Cell::new(None),
            recovery_id: self.next_recovery_id,
            recovery_generation: None,
            recovery_path: None,
            recovery_error: None,
        };
        doc.set_path(path);
        if let Some(goto) = goto {
            doc.cursor_move_to_goto(goto);
        }

        if let Some(active) = self.active()
            && active.path.is_none()
            && active.file_id.is_none()
            && !active.buffer.borrow().is_dirty()
        {
            // If the current document is a pristine Untitled document with no
            // name and no ID, replace it with the new document.
            self.remove_active();
        }

        self.list.push_front(doc);
        Ok(self.list.front_mut().unwrap())
    }

    pub fn reflow_all(&self) {
        for doc in &self.list {
            let mut tb = doc.buffer.borrow_mut();
            tb.reflow();
        }
    }

    pub fn open_for_reading(path: &Path) -> apperr::Result<File> {
        File::open(path).map_err(apperr::Error::from)
    }

    fn create_buffer(config: &Config) -> apperr::Result<RcTextBuffer> {
        let buffer = TextBuffer::new_rc(false)?;
        {
            let mut tb = buffer.borrow_mut();
            tb.set_crlf(config.newline_crlf);
            tb.set_insert_final_newline(config.insert_final_newline);
            tb.set_margin_enabled(config.line_numbers);
            tb.set_line_highlight_enabled(config.line_highlight);
            tb.set_word_wrap(config.word_wrap);
            tb.set_indent_with_tabs(config.indent_with_tabs);
            tb.set_tab_size(config.tab_size as ogedit::helpers::CoordType);
            if config.ruler_column > 0 {
                tb.set_ruler(config.ruler_column as ogedit::helpers::CoordType);
            }
        }
        Ok(buffer)
    }

    // Parse a filename in the form of "filename:line:char".
    // Returns the position of the first colon and the line/char coordinates.
    pub(crate) fn parse_filename_goto(path: &Path) -> (&Path, Option<Point>) {
        fn parse(s: &[u8]) -> Option<CoordType> {
            // Signed parsing permits the minimum integer without overflowing its magnitude.
            if s.is_empty()
                || !s.iter().enumerate().all(|(i, b)| b.is_ascii_digit() || (i == 0 && *b == b'-'))
            {
                return None;
            }
            std::str::from_utf8(s).ok()?.parse().ok()
        }

        fn find_colon_rev(bytes: &[u8], offset: usize) -> Option<usize> {
            (0..offset.min(bytes.len())).rev().find(|&i| bytes[i] == b':')
        }

        let bytes = path.as_os_str().as_encoded_bytes();
        let colend = match find_colon_rev(bytes, bytes.len()) {
            // Reject filenames that would result in an empty filename after stripping off the :line:char suffix.
            // For instance, a filename like ":123:456" will not be processed by this function.
            Some(colend) if colend > 0 => colend,
            _ => return (path, None),
        };

        let last = match parse(&bytes[colend + 1..]) {
            Some(last) => last,
            None => return (path, None),
        };
        let mut len = colend;
        let mut goto = Point { x: 1, y: last };

        if last >= 0
            && let Some(colbeg) = find_colon_rev(bytes, colend)
        {
            // Same here: Don't allow empty filenames.
            if colbeg != 0
                && let Some(first) = parse(&bytes[colbeg + 1..colend])
            {
                len = colbeg;
                goto = Point { x: last, y: first };
            }
        }

        // Strip off the :line:char suffix.
        let path = &bytes[..len];
        let path = unsafe { OsStr::from_encoded_bytes_unchecked(path) };
        let path = Path::new(path);
        (path, Some(goto))
    }

    /// Prefer an existing literal filename over the editor's location suffix.
    /// In particular, Windows C:123 and Unix names containing :digits can be
    /// real files. With no existing target, preserve the established a:1 syntax;
    /// a new numeric drive-relative filename can be spelled C:.\123 explicitly.
    pub(crate) fn resolve_filename_goto(path: &Path) -> (&Path, Option<Point>) {
        if path.is_file() {
            return (path, None);
        }
        let bytes = path.as_os_str().as_encoded_bytes();
        if let Some(colon) = bytes.iter().rposition(|&byte| byte == b':')
            && colon > 0
            && colon + 1 < bytes.len()
        {
            let line = std::str::from_utf8(&bytes[colon + 1..])
                .ok()
                .and_then(|v| v.parse::<CoordType>().ok());
            let candidate =
                Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&bytes[..colon]) });
            if let Some(line) = line
                && candidate.is_file()
            {
                return (candidate, Some(Point { x: 1, y: line }));
            }
        }
        Self::parse_filename_goto(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_detects_same_metadata_rewrites_without_notifications_or_sleeping() {
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!(
            "ogedit-status-probe-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("same.txt");
        std::fs::write(&path, b"original\n").unwrap();
        let mut documents = DocumentManager::default();
        let doc = documents.add_file_path(&path, &Config::default()).unwrap();
        let now = Instant::now();
        assert_eq!(doc.disk_status_at(now), DiskStatus::Unchanged);
        let modified = doc.disk_version.as_ref().unwrap().modified.unwrap();
        std::fs::write(&path, b"external\n").unwrap();
        File::options().write(true).open(&path).unwrap().set_modified(modified).unwrap();
        assert!(doc.disk_version.as_ref().unwrap().metadata_matches(&path));
        assert_eq!(doc.disk_status_at(now + Duration::from_secs(3)), DiskStatus::Modified);
        assert_eq!(doc.save(None), Err(safe_save::APP_SAVE_CONFLICT));
        assert_eq!(std::fs::read(&path).unwrap(), b"external\n");

        std::fs::write(&path, b"original\n").unwrap();
        File::options().write(true).open(&path).unwrap().set_modified(modified).unwrap();
        assert_eq!(doc.disk_status_at(now + Duration::from_secs(6)), DiskStatus::Unchanged);
        drop(documents);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_status_probes_reset_after_save_save_as_and_reload() {
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!(
            "ogedit-probe-reset-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("first.txt");
        std::fs::write(&path, vec![b'a'; safe_save::STATUS_READ_BUDGET + 1]).unwrap();
        let mut documents = DocumentManager::default();
        let doc = documents.add_file_path(&path, &Config::default()).unwrap();
        let now = Instant::now();
        assert_eq!(doc.disk_status_at(now), DiskStatus::Unchanged);
        assert!(doc.disk_probe.borrow().is_some());
        doc.save(None).unwrap();
        assert!(doc.disk_probe.borrow().is_none());
        assert!(doc.next_content_check.get().is_none());

        assert_eq!(doc.disk_status_at(now), DiskStatus::Unchanged);
        assert!(doc.disk_probe.borrow().is_some());
        let second = root.join("second.txt");
        doc.save(Some(second.clone())).unwrap();
        assert!(doc.disk_probe.borrow().is_none());
        assert!(doc.next_content_check.get().is_none());
        assert_eq!(doc.path.as_deref(), Some(safe_save::resolved_path(&second).as_path()));

        assert_eq!(doc.disk_status_at(now), DiskStatus::Unchanged);
        assert!(doc.disk_probe.borrow().is_some());
        doc.reread(None).unwrap();
        assert!(doc.disk_probe.borrow().is_none());
        assert!(doc.next_content_check.get().is_none());
        drop(documents);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_events_restart_a_probe_after_its_scanned_prefix_changes() {
        use std::io::Write;
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!(
            "ogedit-probe-event-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("same.txt");
        std::fs::write(&path, vec![b'a'; safe_save::STATUS_READ_BUDGET * 2 + 1]).unwrap();
        let mut documents = DocumentManager::default();
        let doc = documents.add_file_path(&path, &Config::default()).unwrap();
        let now = Instant::now();
        assert_eq!(doc.disk_status_at(now), DiskStatus::Unchanged);
        assert!(doc.disk_probe.borrow().is_some());
        let modified = doc.disk_version.as_ref().unwrap().modified.unwrap();
        let mut file = File::options().write(true).open(&path).unwrap();
        file.write_all(b"b").unwrap();
        file.set_modified(modified).unwrap();
        drop(file);
        assert!(doc.disk_version.as_ref().unwrap().metadata_matches(&path));
        doc.disk_event.set(true);
        for step in 1..=2 {
            assert_eq!(
                doc.disk_status_at(now + Duration::from_millis(100 * step)),
                DiskStatus::Unchanged
            );
            assert!(doc.disk_probe.borrow().is_some());
        }
        assert_eq!(doc.disk_status_at(now + Duration::from_millis(300)), DiskStatus::Modified);
        assert!(doc.disk_probe.borrow().is_none());
        drop(documents);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn signed_goto_clamps_and_counts_from_the_last_line() {
        let _runtime = crate::test_runtime::lock();
        let mut documents = DocumentManager::default();
        let doc = documents.add_untitled(&Config::default()).unwrap();
        doc.buffer.borrow_mut().set_insert_final_newline(false);
        doc.buffer.borrow_mut().write_raw(b"first\nsecond\nlast");
        doc.cursor_move_to_goto(Point { x: 2, y: -1 });
        assert_eq!(doc.buffer.borrow().cursor_logical_pos(), Point { x: 1, y: 2 });
        doc.cursor_move_to_goto(Point { x: 99, y: -2 });
        assert_eq!(doc.buffer.borrow().cursor_logical_pos(), Point { x: 6, y: 1 });
        doc.cursor_move_to_goto(Point { x: 1, y: CoordType::MIN });
        assert_eq!(doc.buffer.borrow().cursor_logical_pos(), Point { x: 0, y: 0 });
        let (path, goto) = DocumentManager::parse_filename_goto(Path::new("file.txt:-2:3"));
        assert_eq!(path, Path::new("file.txt"));
        assert_eq!(goto, Some(Point { x: 3, y: -2 }));
    }

    #[cfg(unix)]
    #[test]
    fn literal_cli_path_does_not_strip_a_new_colon_numeric_filename() {
        let _runtime = crate::test_runtime::lock();
        let path =
            std::env::temp_dir().join(format!("ogedit-colon-new-{}:123", std::process::id()));
        let mut documents = DocumentManager::default();
        let doc = documents.add_file_path_literal(&path, &Config::default()).unwrap();
        assert_eq!(doc.path, Some(crate::safe_save::resolved_path(&path)));
    }

    #[test]
    fn reload_tracks_whole_lines_and_untitled_reload_is_safe() {
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!(
            "ogedit-legacy-reload-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("reload.txt");
        std::fs::write(&path, b"needle\nother\n").unwrap();
        let mut documents = DocumentManager::default();
        let config = Config::default();
        let doc = documents.add_file_path(&path, &config).unwrap();
        doc.buffer.borrow_mut().cursor_move_to_logical(Point { x: 3, y: 0 });
        std::fs::write(&path, b"prefix needle suffix\nother\nneedle\n").unwrap();
        doc.reread(None).unwrap();
        assert_eq!(doc.buffer.borrow().cursor_logical_pos(), Point { x: 3, y: 2 });
        icu::init().unwrap();
        std::fs::write(&path, b"caf\xe9\n").unwrap();
        doc.reread(Some("windows-1252")).unwrap();
        let encoding = doc.buffer.borrow().encoding();
        std::fs::write(&path, b"caf\xe9 changed\n").unwrap();
        doc.reload().unwrap();
        assert_eq!(doc.buffer.borrow().encoding(), encoding);
        assert_eq!(doc.buffer.borrow().extract_bytes(0..14), "café changed\n".as_bytes());
        let doc = documents.add_untitled(&config).unwrap();
        doc.buffer.borrow_mut().write_raw(b"keep this");
        assert_eq!(doc.reread(None), Err(safe_save::APP_SAVE_UNSUPPORTED));
        assert!(doc.buffer.borrow().is_dirty());
        assert_eq!(doc.buffer.borrow().extract_bytes(0..9), b"keep this");
        drop(documents);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_conflicts_reload_and_recovery_preserve_work() {
        // Binary tests link the non-test library, whose arena is intentionally
        // single-threaded. Keep all buffer-backed document checks in one test.
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!(
            "ogedit-doc-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        std::fs::write(&path, b"original\n").unwrap();
        let config = Config::default();
        let mut documents = DocumentManager {
            recovery: Some(RecoveryStore::new(&root.join("recovery")).unwrap()),
            ..Default::default()
        };
        let doc = documents.add_file_path(&path, &config).unwrap();
        doc.buffer.borrow_mut().write_raw(b"mine ");
        assert!(doc.buffer.borrow().is_dirty());
        std::fs::write(&path, b"external\n").unwrap();
        assert_eq!(doc.disk_status(), DiskStatus::Modified);
        assert_eq!(doc.save(None), Err(safe_save::APP_SAVE_CONFLICT));
        assert!(doc.buffer.borrow().is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), b"external\n");
        let diff = doc.external_diff().unwrap();
        assert!(diff.contains("-external"));
        assert!(diff.contains("+mine original"));
        // A failed reload must leave both the bytes and dirty state untouched.
        let before = doc.buffer.borrow().extract_bytes(0..doc.buffer.borrow().text_length());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(doc.disk_status(), DiskStatus::Deleted);
        assert!(doc.reread(None).is_err());
        assert!(doc.buffer.borrow().is_dirty());
        assert_eq!(doc.buffer.borrow().extract_bytes(0..doc.buffer.borrow().text_length()), before);
        documents.checkpoint_recovery().unwrap();
        let snapshot_path = documents.active().unwrap().recovery_path.clone().unwrap();
        let checkpoint_time = std::fs::metadata(&snapshot_path).unwrap().modified().unwrap();
        documents.checkpoint_recovery().unwrap();
        assert_eq!(std::fs::metadata(&snapshot_path).unwrap().modified().unwrap(), checkpoint_time);
        // Undoing and branching creates a new revision, never the abandoned
        // branch's generation. Recovery must checkpoint the new bytes.
        documents.active_mut().unwrap().buffer.borrow_mut().write_raw(b"A");
        documents.checkpoint_recovery().unwrap();
        let branch_a = std::fs::read(&snapshot_path).unwrap();
        let reused_generation = documents.active().unwrap().buffer.borrow().generation();
        {
            let mut buffer = documents.active_mut().unwrap().buffer.borrow_mut();
            buffer.undo();
            buffer.write_raw(b"B");
            assert_ne!(buffer.generation(), reused_generation);
        }
        documents.checkpoint_recovery().unwrap();
        assert_ne!(std::fs::read(&snapshot_path).unwrap(), branch_a);
        let before = {
            let buffer = documents.active().unwrap().buffer.borrow();
            buffer.extract_bytes(0..buffer.text_length())
        };
        drop(documents); // Simulate a lost process: no explicit discard/save cleanup.
        let mut restored = DocumentManager {
            recovery: Some(RecoveryStore::new(&root.join("recovery")).unwrap()),
            ..Default::default()
        };
        assert_eq!(restored.recoverable().len(), 1);
        let doc = restored.restore_recovery(0, &config).unwrap();
        assert!(doc.path.is_none());
        assert!(doc.buffer.borrow().is_dirty());
        assert_eq!(doc.buffer.borrow().extract_bytes(0..doc.buffer.borrow().text_length()), before);
        let saved_path = root.join("saved.txt");
        doc.save(Some(saved_path.clone())).unwrap();
        assert!(!doc.buffer.borrow().is_dirty());
        assert_eq!(std::fs::read(&saved_path).unwrap(), before);
        assert!(restored.recoverable().is_empty());
        std::fs::write(&saved_path, b"reloaded\n").unwrap();
        let doc = restored.active_mut().unwrap();
        doc.reread(None).unwrap();
        assert!(!doc.buffer.borrow().is_dirty());
        assert_eq!(doc.disk_status(), DiskStatus::Unchanged);
        assert_eq!(
            doc.buffer.borrow().extract_bytes(0..doc.buffer.borrow().text_length()),
            b"reloaded\n"
        );
        std::fs::write(&saved_path, [0xff]).unwrap();
        doc.reread(Some("UTF-8")).unwrap();
        doc.buffer.borrow_mut().apply_byte_edits(&[(0..1, vec![0xfe])]).unwrap();
        assert!(
            !doc.external_diff().unwrap().is_empty(),
            "Lossy decoding must not hide different invalid bytes"
        );
        drop(restored);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_parse_last_numbers() {
        fn parse(s: &str) -> (&str, Option<Point>) {
            let (p, g) = DocumentManager::parse_filename_goto(Path::new(s));
            (p.to_str().unwrap(), g)
        }

        assert_eq!(parse("123"), ("123", None));
        assert_eq!(parse("abc"), ("abc", None));
        assert_eq!(parse(":123"), (":123", None));
        assert_eq!(parse("abc:123"), ("abc", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse("45:123"), ("45", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse(":45:123"), (":45", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse("abc:45:123"), ("abc", Some(Point { x: 123, y: 45 })));
        assert_eq!(parse("abc:def:123"), ("abc:def", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse("1:2:3"), ("1", Some(Point { x: 3, y: 2 })));
        assert_eq!(parse("::3"), (":", Some(Point { x: 1, y: 3 })));
        assert_eq!(parse("1::3"), ("1:", Some(Point { x: 1, y: 3 })));
        assert_eq!(parse(""), ("", None));
        assert_eq!(parse(":"), (":", None));
        assert_eq!(parse("::"), ("::", None));
        assert_eq!(parse("a:1"), ("a", Some(Point { x: 1, y: 1 })));
        assert_eq!(parse("1:a"), ("1:a", None));
        assert_eq!(parse("file.txt:10"), ("file.txt", Some(Point { x: 1, y: 10 })));
        assert_eq!(parse("file.txt:10:5"), ("file.txt", Some(Point { x: 5, y: 10 })));
        #[cfg(windows)]
        {
            assert_eq!(parse(r"C:.\123"), (r"C:.\123", None));
            assert_eq!(parse(r"C:.\123:4"), (r"C:.\123", Some(Point { x: 1, y: 4 })));
            assert_eq!(parse("C:/file.txt:10:5"), ("C:/file.txt", Some(Point { x: 5, y: 10 })));
            assert_eq!(parse("a:1:2"), ("a", Some(Point { x: 2, y: 1 })));
        }
    }

    #[test]
    #[cfg(windows)]
    fn open_quoted_mixed_separator_paths_and_location_suffixes() {
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!("ogedit quoted paths {}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("my file.txt");
        std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
        let forward = path.to_string_lossy().replace('\\', "/");
        let mut documents = DocumentManager::default();
        let config = Config::default();
        for input in [format!("\"{forward}\":2:3"), format!("\"{forward}:2:3\"")] {
            let document = documents.add_file_path(Path::new(&input), &config).unwrap();
            assert_eq!(document.path.as_ref(), Some(&safe_save::resolved_path(&path)));
            assert_eq!(document.buffer.borrow().cursor_logical_pos(), Point { x: 2, y: 1 });
        }
        assert_eq!(documents.iter().count(), 1);
        drop(documents);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn opening_actual_unix_quotes_backslashes_and_colons_preserves_identity() {
        let _runtime = crate::test_runtime::lock();
        let root = std::env::temp_dir().join(format!("ogedit-literal-path-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(r#""a\b":12"#);
        std::fs::write(&path, "literal").unwrap();
        let mut documents = DocumentManager::default();
        let document = documents.add_file_path(&path, &Config::default()).unwrap();
        assert_eq!(document.path.as_ref(), Some(&safe_save::resolved_path(&path)));
        drop(documents);
        std::fs::remove_dir_all(root).unwrap();
    }
}
