// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::LinkedList;
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ogedit::buffer::{RcTextBuffer, TextBuffer};
use ogedit::helpers::{CoordType, Point};
use ogedit::{apperr, path, sys};

use crate::config::Config;
use crate::state::DisplayablePathBuf;

pub struct Document {
    pub buffer: RcTextBuffer,
    pub path: Option<PathBuf>,
    pub dir: Option<DisplayablePathBuf>,
    pub filename: String,
    pub file_id: Option<sys::FileId>,
    pub new_file_counter: usize,

    // File change detection (lightweight - hybrid approach)
    pub last_modified: Option<SystemTime>,
}

impl Document {
    pub fn save(&mut self, new_path: Option<PathBuf>) -> apperr::Result<()> {
        let path = new_path.as_deref().unwrap_or_else(|| self.path.as_ref().unwrap().as_path());
        let mut file = DocumentManager::open_for_writing(path)?;

        {
            let mut tb = self.buffer.borrow_mut();
            tb.write_file(&mut file)?;
        }

        if let Ok(id) = sys::file_id(None, path) {
            self.file_id = Some(id);
        }

        // Capture file modification timestamp after successful save
        self.last_modified = std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok());

        if let Some(path) = new_path {
            self.set_path(path);
        }

        Ok(())
    }

    pub fn reread(&mut self, encoding: Option<&'static str>) -> apperr::Result<()> {
        let path = self.path.as_ref().unwrap().as_path();
        let mut file = DocumentManager::open_for_reading(path)?;

        // Save cursor state before reload
        let (saved_pos, saved_line_content) = {
            let tb = self.buffer.borrow();
            let pos = tb.cursor_logical_pos();
            let line_content = tb.extract_line_content(pos.y);
            (pos, line_content)
        };

        // Reload the file
        {
            let mut tb = self.buffer.borrow_mut();
            tb.read_file(&mut file, encoding)?;
        }

        // Restore cursor position after reload
        {
            let mut tb = self.buffer.borrow_mut();
            let new_line_count = tb.logical_line_count();

            // Try to find the same line content in the new file
            let target_line = if let Some((content, _)) = saved_line_content {
                if !content.is_empty() {
                    let matches = tb.find_line_matches(&content);
                    if matches.len() == 1 {
                        // Unique match - use that line
                        Some(matches[0].0)
                    } else if matches.len() > 1 {
                        // Multiple matches - find the one closest to the original line number
                        matches
                            .iter()
                            .min_by_key(|(line, _)| (*line - saved_pos.y).abs())
                            .map(|(line, _)| *line)
                    } else {
                        // No matches - fall back to clamping
                        None
                    }
                } else {
                    // Empty line content - just try to restore the line number
                    None
                }
            } else {
                None
            };

            // Calculate the target position
            let target_y = target_line.unwrap_or_else(|| saved_pos.y.min(new_line_count - 1).max(0));
            let target_x = saved_pos.x; // Column will be clamped by cursor_move_to_logical

            // Move cursor to the target position
            tb.cursor_move_to_logical(Point { x: target_x, y: target_y });
            tb.make_cursor_visible();
        }

        if let Ok(id) = sys::file_id(None, path) {
            self.file_id = Some(id);
        }

        // Capture file modification timestamp after reload
        self.last_modified = std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok());

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
        tb.set_ruler(if self.filename == "COMMIT_EDITMSG" { 72 } else { 0 });
    }

    /// Check if the file has been modified on disk since we last read/saved it.
    /// Returns false if:
    /// - The document has no associated file path
    /// - We don't have a baseline timestamp
    /// - The file no longer exists on disk
    /// - The timestamps match (file hasn't changed)
    pub fn has_file_changed_on_disk(&self) -> bool {
        if let (Some(path), Some(last_modified)) = (&self.path, self.last_modified) {
            if let Ok(metadata) = std::fs::metadata(path) {
                if let Ok(current_modified) = metadata.modified() {
                    let changed = current_modified != last_modified;
                    // DEBUG: Log the check result
                    if changed {
                        crate::logging::log_action(&format!(
                            "FILE_CHANGED_DETECTED: {} (last: {:?}, current: {:?})",
                            path.display(), last_modified, current_modified
                        ));
                    }
                    return changed;
                }
            }
        }
        false
    }
}

#[derive(Default)]
pub struct DocumentManager {
    list: LinkedList<Document>,
}

impl DocumentManager {
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
        let mut cursor = self.list.cursor_front_mut();
        while let Some(doc) = cursor.current() {
            if func(doc) {
                let list = cursor.remove_current_as_list().unwrap();
                self.list.cursor_front_mut().splice_before(list);
                return true;
            }
            cursor.move_next();
        }
        false
    }

    pub fn remove_active(&mut self) {
        self.list.pop_front();
    }

    /// Check if a file path is currently open in any document
    pub fn is_path_open(&self, path: &Path) -> bool {
        self.list.iter().any(|doc| doc.path.as_deref() == Some(path))
    }

    pub fn add_untitled(&mut self, config: &Config) -> apperr::Result<&mut Document> {
        let buffer = Self::create_buffer(config)?;
        let mut doc = Document {
            buffer,
            path: None,
            dir: Default::default(),
            filename: Default::default(),
            file_id: None,
            new_file_counter: 0,
            last_modified: None,
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
        let (path, goto) = Self::parse_filename_goto(path);
        let path = path::normalize(path);

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
                doc.buffer.borrow_mut().cursor_move_to_logical(goto);
            }
            return Ok(doc);
        }

        let buffer = Self::create_buffer(config)?;
        {
            if let Some(file) = &mut file {
                let mut tb = buffer.borrow_mut();
                tb.read_file(file, None)?;

                if let Some(goto) = goto
                    && goto != Default::default()
                {
                    tb.cursor_move_to_logical(goto);
                }
            }
        }

        // Capture file modification timestamp when loading file
        let last_modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());

        // DEBUG: Log timestamp capture
        crate::logging::log_action(&format!(
            "FILE_LOAD_TIMESTAMP: {} = {:?}",
            path.display(), last_modified
        ));

        let mut doc = Document {
            buffer,
            path: None,
            dir: None,
            filename: Default::default(),
            file_id,
            new_file_counter: 0,
            last_modified,
        };
        doc.set_path(path);

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

    pub fn open_for_writing(path: &Path) -> apperr::Result<File> {
        File::create(path).map_err(apperr::Error::from)
    }

    fn create_buffer(config: &Config) -> apperr::Result<RcTextBuffer> {
        let buffer = TextBuffer::new_rc(config.newline_crlf)?;
        {
            let mut tb = buffer.borrow_mut();
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
    fn parse_filename_goto(path: &Path) -> (&Path, Option<Point>) {
        fn parse(s: &[u8]) -> Option<CoordType> {
            if s.is_empty() {
                return None;
            }

            let mut num: CoordType = 0;
            for &b in s {
                if !b.is_ascii_digit() {
                    return None;
                }
                let digit = (b - b'0') as CoordType;
                num = num.checked_mul(10)?.checked_add(digit)?;
            }
            Some(num)
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
        let last = (last - 1).max(0);
        let mut len = colend;
        let mut goto = Point { x: 0, y: last };

        if let Some(colbeg) = find_colon_rev(bytes, colend) {
            // Same here: Don't allow empty filenames.
            if colbeg != 0
                && let Some(first) = parse(&bytes[colbeg + 1..colend])
            {
                let first = (first - 1).max(0);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_last_numbers() {
        fn parse(s: &str) -> (&str, Option<Point>) {
            let (p, g) = DocumentManager::parse_filename_goto(Path::new(s));
            (p.to_str().unwrap(), g)
        }

        assert_eq!(parse("123"), ("123", None));
        assert_eq!(parse("abc"), ("abc", None));
        assert_eq!(parse(":123"), (":123", None));
        assert_eq!(parse("abc:123"), ("abc", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse("45:123"), ("45", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse(":45:123"), (":45", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse("abc:45:123"), ("abc", Some(Point { x: 122, y: 44 })));
        assert_eq!(parse("abc:def:123"), ("abc:def", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse("1:2:3"), ("1", Some(Point { x: 2, y: 1 })));
        assert_eq!(parse("::3"), (":", Some(Point { x: 0, y: 2 })));
        assert_eq!(parse("1::3"), ("1:", Some(Point { x: 0, y: 2 })));
        assert_eq!(parse(""), ("", None));
        assert_eq!(parse(":"), (":", None));
        assert_eq!(parse("::"), ("::", None));
        assert_eq!(parse("a:1"), ("a", Some(Point { x: 0, y: 0 })));
        assert_eq!(parse("1:a"), ("1:a", None));
        assert_eq!(parse("file.txt:10"), ("file.txt", Some(Point { x: 0, y: 9 })));
        assert_eq!(parse("file.txt:10:5"), ("file.txt", Some(Point { x: 4, y: 9 })));
    }
}
