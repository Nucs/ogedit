// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A text buffer for a text editor.
//!
//! Implements a Unicode-aware, layout-aware text buffer for terminals.
//! It's based on a gap buffer. It has no line cache and instead relies
//! on the performance of the ucd module for fast text navigation.
//!
//! ---
//!
//! If the project ever outgrows a basic gap buffer (e.g. to add time travel)
//! an ideal, alternative architecture would be a piece table with immutable trees.
//! The tree nodes can be allocated on the same arena allocator as the added chunks,
//! making lifetime management fairly easy. The algorithm is described here:
//! * <https://cdacamar.github.io/data%20structures/algorithms/benchmarking/text%20editors/c++/editor-data-structures/>
//! * <https://github.com/cdacamar/fredbuf>
//!
//! The downside is that text navigation & search takes a performance hit due to small chunks.
//! The solution to the former is to keep line caches, which further complicates the architecture.
//! There's no solution for the latter. However, there's a chance that the performance will still be sufficient.

mod gap_buffer;
mod navigation;
#[cfg(test)]
mod upstream_tests;

use std::borrow::Cow;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::mem::{self, MaybeUninit};
use std::ops::Range;
use std::rc::Rc;
use std::str;

pub use gap_buffer::GapBuffer;

use crate::arena::{Arena, ArenaString, scratch_arena};
use crate::cell::SemiRefCell;
use crate::clipboard::Clipboard;
use crate::document::{ReadableDocument, WriteableDocument};
use crate::framebuffer::{Attributes, Framebuffer, IndexedColor};
use crate::helpers::*;
use crate::oklab::StraightRgba;
use crate::simd::memchr2;
use crate::unicode::{Cursor, MeasurementConfig, Utf8Chars};
use crate::{apperr, icu, simd};

/// The margin template is used for line numbers.
/// The max. line number we should ever expect is probably 64-bit,
/// and so this template fits 19 digits, followed by " │ ".
const MARGIN_TEMPLATE: &str = "                    │ ";
/// Just a bunch of whitespace you can use for turning tabs into spaces.
/// Happens to reuse MARGIN_TEMPLATE, because it has sufficient whitespace.
const TAB_WHITESPACE: &str = MARGIN_TEMPLATE;
const VISUAL_SPACE: &str = "･";
const VISUAL_SPACE_PREFIX_ADD: usize = '･'.len_utf8() - 1;
const VISUAL_TAB: &str = "￫       ";
const VISUAL_TAB_PREFIX_ADD: usize = '￫'.len_utf8() - 1;

/// Stores statistics about the whole document.
#[derive(Copy, Clone)]
pub struct TextBufferStatistics {
    logical_lines: CoordType,
    visual_lines: CoordType,
}

/// Stores the active text selection anchors.
///
/// The two points are not sorted. Instead, `beg` refers to where the selection
/// started being made and `end` refers to the currently being updated position.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct TextBufferSelection {
    beg: Point,
    end: Point,
}

/// In order to group actions into a single undo step,
/// we need to know the type of action that was performed.
/// This stores the action type.
#[derive(Copy, Clone, Eq, PartialEq)]
enum HistoryType {
    Other,
    Write,
    Delete,
}

/// An undo/redo entry.
struct HistoryEntry {
    /// [`TextBuffer::cursor`] position before the change was made.
    cursor_before: Point,
    /// [`TextBuffer::selection`] before the change was made.
    selection_before: Option<TextBufferSelection>,
    secondary_before: Vec<(usize, usize)>,
    newlines_before: bool,
    /// [`TextBuffer::stats`] before the change was made.
    stats_before: TextBufferStatistics,
    layout_before: (CoordType, CoordType, bool, bool),
    /// [`GapBuffer::generation`] before the change was made.
    ///
    /// **NOTE:** Entries with the same generation are grouped together.
    generation_before: u32,
    /// Exact replacement offset; the edit may join graphemes around this byte.
    offset: usize,
    /// Text that was deleted from the buffer.
    deleted: Vec<u8>,
    /// Text that was added to the buffer.
    added: Vec<u8>,
}

/// Caches an ICU search operation.
struct ActiveSearch {
    /// The search pattern.
    pattern: String,
    /// The search options.
    options: SearchOptions,
    // Drop the regex before its backing provider.
    regex: icu::Regex,
    text: icu::Text,
    /// [`GapBuffer::generation`] when the search was created.
    /// This is used to detect if we need to refresh the
    /// [`ActiveSearch::regex`] object.
    buffer_generation: u64,
    /// [`TextBuffer::selection_generation`] when the search was
    /// created. When the user manually selects text, we need to
    /// refresh the [`ActiveSearch::pattern`] with it.
    selection_generation: u32,
    /// Stores the text buffer offset in between searches.
    next_search_offset: usize,
    /// If we know there were no hits, we can skip searching.
    no_matches: bool,
}

/// Options for a search operation.
#[derive(Default, Clone, Copy, Eq, PartialEq)]
pub struct SearchOptions {
    /// If true, the search is case-sensitive.
    pub match_case: bool,
    /// If true, the search matches whole words.
    pub whole_word: bool,
    /// If true, the search uses regex.
    pub use_regex: bool,
}

enum RegexReplacement<'a> {
    Group(i32),
    Text(Vec<u8, &'a Arena>),
}

/// Caches the start and length of the active edit line for a single edit.
/// This helps us avoid having to remeasure the buffer after an edit.
struct ActiveEditLineInfo {
    /// Points to the start of the currently being edited line.
    safe_start: Cursor,
    /// Number of visual rows of the line that starts
    /// at [`ActiveEditLineInfo::safe_start`].
    line_height_in_rows: CoordType,
    /// Byte distance from the start of the line at
    /// [`ActiveEditLineInfo::safe_start`] to the next line.
    distance_next_line_start: usize,
}

/// Maximum number of auto-highlight matches to find (performance limit).
const SELECTION_HIGHLIGHT_MAX_MATCHES: usize = 1000;

/// Caches auto-highlight matches for the current selection.
/// When text is selected, all identical occurrences are highlighted.
struct SelectionHighlights {
    /// The pattern to match (selected text as bytes).
    pattern: Vec<u8>,
    /// Cached match positions (byte offset ranges).
    matches: Vec<Range<usize>>,
    /// [`TextBuffer::selection_generation`] when matches were computed.
    cached_selection_generation: u32,
    /// [`GapBuffer::generation`] when matches were computed.
    cached_buffer_generation: u32,
}

impl SelectionHighlights {
    fn new() -> Self {
        Self {
            pattern: Vec::new(),
            matches: Vec::new(),
            cached_selection_generation: 0,
            cached_buffer_generation: 0,
        }
    }

    /// Highlight repeated text, excluding whitespace and a lone letter.
    /// A character here is a grapheme, matching editor selection/navigation.
    fn should_highlight(text: &[u8]) -> bool {
        let Ok(value) = std::str::from_utf8(text) else {
            return false;
        };
        let Some(first) = value.chars().next() else {
            return false;
        };
        if value.chars().all(char::is_whitespace) {
            return false;
        }
        if first.is_alphabetic() {
            let end = MeasurementConfig::new(&text).goto_logical(Point { x: 2, y: 0 });
            return end.offset < text.len() || end.logical_pos.x > 1;
        }
        true
    }

    /// Update the cache if the selection or buffer has changed.
    fn update(
        &mut self,
        selection_range: Option<Range<usize>>,
        selection_generation: u32,
        buffer: &GapBuffer,
    ) {
        let buffer_generation = buffer.generation();

        // Check if cache is still valid
        if self.cached_selection_generation == selection_generation
            && self.cached_buffer_generation == buffer_generation
        {
            return;
        }

        // Cache is stale, update it
        self.cached_selection_generation = selection_generation;
        self.cached_buffer_generation = buffer_generation;
        self.matches.clear();
        self.pattern.clear();

        let Some(range) = selection_range else {
            return;
        };

        if range.is_empty() {
            return;
        }

        // Extract the selected text
        buffer.extract_raw(range.clone(), &mut self.pattern, 0);

        if !Self::should_highlight(&self.pattern) {
            self.pattern.clear();
            return;
        }

        self.matches = find_all_matches(buffer, &self.pattern, SELECTION_HIGHLIGHT_MAX_MATCHES)
            .into_iter()
            .filter(|m| *m != range)
            .collect();
    }
}

/// Streaming KMP search avoids copying a document and crosses the gap correctly.
fn find_all_matches(buffer: &GapBuffer, pattern: &[u8], limit: usize) -> Vec<Range<usize>> {
    if pattern.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut prefix = vec![0; pattern.len()];
    let mut matched = 0;
    for i in 1..pattern.len() {
        while matched > 0 && pattern[i] != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if pattern[i] == pattern[matched] {
            matched += 1;
        }
        prefix[i] = matched;
    }
    let mut matches = Vec::new();
    let mut offset = 0;
    matched = 0;
    while offset < buffer.len() {
        let chunk = buffer.read_forward(offset);
        for (i, &byte) in chunk.iter().enumerate() {
            while matched > 0 && byte != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if byte == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                let end = offset + i + 1;
                matches.push(end - pattern.len()..end);
                if matches.len() >= limit {
                    return matches;
                }
                matched = 0;
            }
        }
        offset += chunk.len();
    }
    matches
}

/// Undo/redo grouping works by recording a set of "overrides",
/// which are then applied in [`TextBuffer::edit_begin()`].
/// This allows us to create a group of edits that all share a
/// common `generation_before` and can be undone/redone together.
/// This struct stores those overrides.
struct ActiveEditGroupInfo {
    redo_before: VecDeque<SemiRefCell<HistoryEntry>>,
    /// [`TextBuffer::cursor`] position before the change was made.
    cursor_before: Point,
    /// [`TextBuffer::selection`] before the change was made.
    selection_before: Option<TextBufferSelection>,
    secondary_before: Vec<(usize, usize)>,
    newlines_before: bool,
    /// [`TextBuffer::stats`] before the change was made.
    stats_before: TextBufferStatistics,
    layout_before: (CoordType, CoordType, bool, bool),
    /// [`GapBuffer::generation`] before the change was made.
    ///
    /// **NOTE:** Entries with the same generation are grouped together.
    generation_before: u32,
}

/// Char- or word-wise navigation? Your choice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CursorMovement {
    Grapheme,
    Word,
}

/// See [`TextBuffer::move_selected_lines`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MoveLineDirection {
    Up,
    Down,
}

/// The result of a call to [`TextBuffer::render()`].
pub struct RenderResult {
    /// The maximum visual X position we encountered during rendering.
    pub visual_pos_x_max: CoordType,
}

/// A [`TextBuffer`] with inner mutability.
pub type TextBufferCell = SemiRefCell<TextBuffer>;

/// A [`TextBuffer`] inside an [`Rc`].
///
/// We need this because the TUI system needs to borrow
/// the given text buffer(s) until after the layout process.
pub type RcTextBuffer = Rc<TextBufferCell>;

/// A text buffer for a text editor.
pub struct TextBuffer {
    // ICU retains a pointer to this provider. A stable allocation keeps safe
    // moves of TextBuffer from invalidating active searches.
    buffer: Box<GapBuffer>,

    undo_stack: VecDeque<SemiRefCell<HistoryEntry>>,
    redo_stack: VecDeque<SemiRefCell<HistoryEntry>>,
    last_history_type: HistoryType,
    last_save_generation: Option<u32>,
    content_revision: u64,
    syntax: Option<crate::syntax::SyntaxCache>,
    syntax_path: Option<std::path::PathBuf>,
    syntax_language: Option<String>,
    syntax_enabled: bool,

    active_edit_group: Option<ActiveEditGroupInfo>,
    edit_group_depth: usize,
    active_edit_line_info: Option<ActiveEditLineInfo>,
    active_edit_depth: i32,
    active_edit_off: usize,
    active_edit_needs_reflow: bool,

    stats: TextBufferStatistics,
    cursor: Cursor,
    // When scrolling significant amounts of text away from the cursor,
    // rendering will naturally slow down proportionally to the distance.
    // To avoid this, we cache the cursor position for rendering.
    // Must be cleared on every edit or reflow.
    cursor_for_rendering: Option<Cursor>,
    selection: Option<TextBufferSelection>,
    secondary_selections: Vec<(usize, usize)>,
    rectangle: Option<(Point, Point)>,
    edit_capacity_error: bool,
    selection_generation: u32,
    search: Option<UnsafeCell<ActiveSearch>>,
    selection_highlights: SelectionHighlights,

    width: CoordType,
    margin_width: CoordType,
    margin_enabled: bool,
    word_wrap_column: CoordType,
    word_wrap_enabled: bool,
    tab_size: CoordType,
    indent_with_tabs: bool,
    line_highlight_enabled: bool,
    ruler: CoordType,
    encoding: &'static str,
    newlines_are_crlf: bool,
    insert_final_newline: bool,
    overtype: bool,

    wants_cursor_visibility: bool,
}

impl Drop for TextBuffer {
    fn drop(&mut self) {
        // Native regex/provider objects must close before their backing storage.
        self.search = None;
    }
}

impl TextBuffer {
    /// Creates a new text buffer inside an [`Rc`].
    /// See [`TextBuffer::new()`].
    pub fn new_rc(small: bool) -> apperr::Result<RcTextBuffer> {
        let buffer = Self::new(small)?;
        Ok(Rc::new(SemiRefCell::new(buffer)))
    }

    /// Creates a new text buffer. With `small` you can control
    /// if the buffer is optimized for <1MiB contents.
    pub fn new(small: bool) -> apperr::Result<Self> {
        Ok(Self {
            buffer: Box::new(GapBuffer::new(small)?),

            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            last_history_type: HistoryType::Other,
            last_save_generation: Some(0),
            content_revision: 0,
            syntax: None,
            syntax_path: None,
            syntax_language: None,
            syntax_enabled: true,

            active_edit_group: None,
            edit_group_depth: 0,
            active_edit_line_info: None,
            active_edit_depth: 0,
            active_edit_off: 0,
            active_edit_needs_reflow: false,

            stats: TextBufferStatistics { logical_lines: 1, visual_lines: 1 },
            cursor: Default::default(),
            cursor_for_rendering: None,
            selection: None,
            secondary_selections: Vec::new(),
            rectangle: None,
            edit_capacity_error: false,
            selection_generation: 0,
            search: None,
            selection_highlights: SelectionHighlights::new(),

            width: 0,
            margin_width: 0,
            margin_enabled: false,
            word_wrap_column: 0,
            word_wrap_enabled: false,
            tab_size: 4,
            indent_with_tabs: false,
            line_highlight_enabled: false,
            ruler: 0,
            encoding: "UTF-8",
            newlines_are_crlf: cfg!(windows), // Windows users want CRLF
            insert_final_newline: false,
            overtype: false,

            wants_cursor_visibility: false,
        })
    }

    /// Length of the document in bytes.
    pub fn text_length(&self) -> usize {
        self.buffer.len()
    }

    /// Number of logical lines in the document,
    /// that is, lines separated by newlines.
    pub fn logical_line_count(&self) -> CoordType {
        self.stats.logical_lines
    }

    /// Number of visual lines in the document,
    /// that is, the number of lines after layout.
    pub fn visual_line_count(&self) -> CoordType {
        self.stats.visual_lines
    }

    /// Does the buffer need to be saved?
    pub fn is_dirty(&self) -> bool {
        self.last_save_generation != Some(self.buffer.generation())
    }

    /// The buffer generation changes on every edit.
    /// With this you can check if it has changed since
    /// the last time you called this function.
    pub fn generation(&self) -> u32 {
        self.buffer.generation()
    }

    /// Monotonic edit revision, including undo/redo and reloads.
    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    /// Force the buffer to be dirty.
    pub fn mark_as_dirty(&mut self) {
        self.last_save_generation = None;
    }

    fn mark_as_clean(&mut self) {
        self.last_save_generation = Some(self.buffer.generation());
        // Saving must establish a reachable undo boundary.
        self.last_history_type = HistoryType::Other;
    }

    /// The encoding used during reading/writing. "UTF-8" is the default.
    pub fn encoding(&self) -> &'static str {
        self.encoding
    }

    /// Set the encoding used during reading/writing.
    pub fn set_encoding(&mut self, encoding: &'static str) {
        if self.encoding != encoding {
            self.encoding = encoding;
            self.mark_as_dirty();
        }
    }

    /// The newline type used in the document. LF or CRLF.
    pub fn is_crlf(&self) -> bool {
        self.newlines_are_crlf
    }

    /// Changes the newline type without normalizing the document.
    pub fn set_crlf(&mut self, crlf: bool) {
        self.newlines_are_crlf = crlf;
    }

    /// Convert line endings as one reversible, capacity-checked edit.
    pub fn normalize_newlines(&mut self, crlf: bool) {
        let newline: &[u8] = if crlf { b"\r\n" } else { b"\n" };
        let mut edits = Vec::new();
        let mut offset = 0;
        while offset < self.text_length() {
            let chunk = self.read_forward(offset);
            for (index, &byte) in chunk.iter().enumerate() {
                if byte != b'\n' {
                    continue;
                }
                let end = offset + index + 1;
                let has_cr = self.read_backward(end - 1).last() == Some(&b'\r');
                if has_cr != crlf {
                    let start = end - if has_cr { 2 } else { 1 };
                    edits.push((start..end, newline.to_vec()));
                }
            }
            offset += chunk.len();
        }
        if edits.is_empty() {
            if self.newlines_are_crlf != crlf {
                self.edit_begin(HistoryType::Other, self.cursor);
                self.buffer.bump_generation();
                self.newlines_are_crlf = crlf;
                self.edit_end();
            }
            return;
        }
        let selections = self.selection_ranges();
        let moved: Vec<_> = selections
            .iter()
            .map(|&(a, b)| {
                let map = |offset: usize| {
                    let mut mapped = offset;
                    for (range, text) in &edits {
                        if range.end > offset {
                            break;
                        }
                        mapped = mapped - range.len() + text.len();
                    }
                    mapped
                };
                (map(a), map(b))
            })
            .collect();
        if self.apply_byte_edits(&edits).is_err() {
            self.edit_capacity_error = true;
            return;
        }
        self.newlines_are_crlf = crlf;
        self.set_selection_ranges(&moved);
    }

    /// If enabled, automatically insert a final newline
    /// when typing at the end of the file.
    pub fn set_insert_final_newline(&mut self, enabled: bool) {
        self.insert_final_newline = enabled;
    }

    /// Whether to insert or overtype text when writing.
    pub fn is_overtype(&self) -> bool {
        self.overtype
    }

    /// Set the overtype mode.
    pub fn set_overtype(&mut self, overtype: bool) {
        self.overtype = overtype;
    }

    /// Gets the logical cursor position, that is,
    /// the position in lines and graphemes per line.
    pub fn cursor_logical_pos(&self) -> Point {
        self.cursor.logical_pos
    }

    /// Gets the visual cursor position, that is,
    /// the position in laid out rows and columns.
    pub fn cursor_visual_pos(&self) -> Point {
        self.cursor.visual_pos
    }

    /// Gets the cursor byte offset in the buffer.
    pub fn cursor_offset(&self) -> usize {
        self.cursor.offset
    }

    /// Gets the width of the left margin.
    pub fn margin_width(&self) -> CoordType {
        self.margin_width
    }

    /// Is the left margin enabled?
    pub fn set_margin_enabled(&mut self, enabled: bool) -> bool {
        if self.margin_enabled == enabled {
            false
        } else {
            self.margin_enabled = enabled;
            self.reflow();
            true
        }
    }

    /// Gets the width of the text contents for layout.
    pub fn text_width(&self) -> CoordType {
        self.width - self.margin_width
    }

    /// Ask the TUI system to scroll the buffer and make the cursor visible.
    ///
    /// TODO: This function shows that [`TextBuffer`] is poorly abstracted
    /// away from the TUI system. The only reason this exists is so that
    /// if someone outside the TUI code enables word-wrap, the TUI code
    /// recognizes this and scrolls the cursor into view. But outside of this
    /// scrolling, views, etc., are all UI concerns = this should not be here.
    pub fn make_cursor_visible(&mut self) {
        self.wants_cursor_visibility = true;
    }

    /// For the TUI code to retrieve a prior [`TextBuffer::make_cursor_visible()`] request.
    pub fn take_cursor_visibility_request(&mut self) -> bool {
        mem::take(&mut self.wants_cursor_visibility)
    }

    /// Is word-wrap enabled?
    ///
    /// Technically, this is a misnomer, because it's line-wrapping.
    pub fn is_word_wrap_enabled(&self) -> bool {
        self.word_wrap_enabled
    }

    /// Enable or disable word-wrap.
    ///
    /// NOTE: It's expected that the tui code calls `set_width()` sometime after this.
    /// This will then trigger the actual recalculation of the cursor position.
    pub fn set_word_wrap(&mut self, enabled: bool) {
        if self.word_wrap_enabled != enabled {
            self.word_wrap_enabled = enabled;
            self.width = 0; // Force a reflow.
            self.make_cursor_visible();
        }
    }

    /// Set the width available for layout.
    ///
    /// Ideally this would be a pure UI concern, but the text buffer needs this
    /// so that it can abstract away  visual cursor movement such as "go a line up".
    /// What would that even mean if it didn't know how wide a line is?
    pub fn set_width(&mut self, width: CoordType) -> bool {
        if width <= 0 || width == self.width {
            false
        } else {
            self.width = width;
            self.reflow();
            true
        }
    }

    /// Set the tab width. Could be anything, but is expected to be 1-8.
    pub fn tab_size(&self) -> CoordType {
        self.tab_size
    }

    /// Set the tab size. Clamped to 1-8.
    pub fn set_tab_size(&mut self, width: CoordType) -> bool {
        let width = width.clamp(1, 8);
        if width == self.tab_size {
            false
        } else {
            self.tab_size = width;
            self.reflow();
            true
        }
    }

    /// Calculates the amount of spaces a tab key press would insert at the given column.
    /// This also equals the visual width of an actual tab character.
    ///
    /// This exists because Rust doesn't have range constraints yet, and without
    /// them assembly blows up in size by 7x. It's a recurring issue with Rust.
    #[inline]
    fn tab_size_eval(&self, column: CoordType) -> CoordType {
        // SAFETY: `set_tab_size` clamps `self.tab_size` to 1-8.
        unsafe { std::hint::assert_unchecked(self.tab_size >= 1 && self.tab_size <= 8) };
        self.tab_size - (column % self.tab_size)
    }

    /// If the cursor is at an indentation of `column`, this returns
    /// the column to which a backspace key press would delete to.
    #[inline]
    fn tab_size_prev_column(&self, column: CoordType) -> CoordType {
        // SAFETY: `set_tab_size` clamps `self.tab_size` to 1-8.
        unsafe { std::hint::assert_unchecked(self.tab_size >= 1 && self.tab_size <= 8) };
        (column - 1).max(0) / self.tab_size * self.tab_size
    }

    /// Returns whether tabs are used for indentation.
    pub fn indent_with_tabs(&self) -> bool {
        self.indent_with_tabs
    }

    /// Sets whether tabs or spaces are used for indentation.
    pub fn set_indent_with_tabs(&mut self, indent_with_tabs: bool) {
        self.indent_with_tabs = indent_with_tabs;
    }

    /// Sets whether the line the cursor is on should be highlighted.
    pub fn set_line_highlight_enabled(&mut self, enabled: bool) {
        self.line_highlight_enabled = enabled;
    }

    /// Sets a ruler column, e.g. 80.
    pub fn set_ruler(&mut self, column: CoordType) {
        self.ruler = column;
    }

    pub fn reflow(&mut self) {
        self.reflow_internal(true);
    }

    fn recalc_after_content_changed(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
        self.reflow_internal(false);
    }

    fn reflow_internal(&mut self, force: bool) {
        let word_wrap_column_before = self.word_wrap_column;

        {
            // +1 onto logical_lines, because line numbers are 1-based.
            // +1 onto log10, because we want the digit width and not the actual log10.
            // +3 onto log10, because we append " | " to the line numbers to form the margin.
            self.margin_width = if self.margin_enabled {
                self.stats.logical_lines.ilog10() as CoordType + 4
            } else {
                0
            };

            let text_width = self.text_width();
            // 2 columns are required, because otherwise wide glyphs wouldn't ever fit.
            self.word_wrap_column =
                if self.word_wrap_enabled && text_width >= 2 { text_width } else { 0 };
        }

        self.cursor_for_rendering = None;

        if force || self.word_wrap_column != word_wrap_column_before {
            // Recalculate the cursor position.
            self.cursor = self.cursor_move_to_logical_internal(
                if self.word_wrap_column > 0 {
                    Default::default()
                } else {
                    // Disabling wrap at a line start can make goto_line_start
                    // return the cached cursor unchanged. Remove its old wrapped
                    // row before using it as the unwrapped measurement origin.
                    let mut cursor = self.cursor;
                    cursor.visual_pos = Point { x: cursor.column, y: cursor.logical_pos.y };
                    cursor.wrap_opp = false;
                    self.goto_line_start(cursor, cursor.logical_pos.y)
                },
                self.cursor.logical_pos,
            );

            // Recalculate the line statistics.
            if self.word_wrap_column > 0 {
                let end = self.cursor_move_to_logical_internal(self.cursor, Point::MAX);
                self.stats.visual_lines = end.visual_pos.y + 1;
            } else {
                self.stats.visual_lines = self.stats.logical_lines;
            }
        }
    }

    /// Replaces the entire buffer contents with the given `text`.
    /// Assumes that the line count doesn't change.
    pub fn copy_from_str(&mut self, text: &dyn ReadableDocument) {
        if self.buffer.copy_from(text) {
            self.recalc_after_content_swap();
            self.cursor_move_to_logical(Point { x: CoordType::MAX, y: 0 });

            let delete = self.buffer.len() - self.cursor.offset;
            if delete != 0 {
                self.buffer.allocate_gap(self.cursor.offset, 0, delete);
            }
        }
    }

    fn recalc_after_content_swap(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
        // If the buffer was changed, nothing we previously saved can be relied upon.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_history_type = HistoryType::Other;
        self.cursor = Default::default();
        self.secondary_selections.clear();
        self.rectangle = None;
        self.set_selection(None);
        self.mark_as_clean();
        self.reflow();
    }

    /// Commit a fully loaded replacement while retaining the current view settings.
    /// Loading into a temporary buffer keeps a failed reload from damaging edits.
    pub fn replace_loaded_content(&mut self, mut loaded: TextBuffer) {
        self.search = None;
        loaded.search = None;
        mem::swap(&mut self.buffer, &mut loaded.buffer);
        self.buffer.advance_revision_from(&loaded.buffer);
        self.encoding = loaded.encoding;
        self.newlines_are_crlf = loaded.newlines_are_crlf;
        self.recalc_after_content_swap();
    }

    /// Copies the contents of the buffer into a string and marks the buffer as clean.
    /// Use this when saving to a file or when the content should be considered "saved".
    pub fn save_as_string(&mut self, dst: &mut dyn WriteableDocument) {
        self.buffer.copy_into(dst);
        self.mark_as_clean();
    }

    /// Copies the contents of the buffer into a string without modifying state.
    /// Use this for read-only operations like logging or previewing.
    pub fn copy_content(&self, dst: &mut dyn WriteableDocument) {
        self.buffer.copy_into(dst);
    }

    /// Reads a file from disk into the text buffer, detecting encoding and BOM.
    pub fn read_file(
        &mut self,
        file: &mut File,
        encoding: Option<&'static str>,
    ) -> apperr::Result<()> {
        let scratch = scratch_arena(None);
        let mut buf = scratch.alloc_uninit().transpose();
        let mut first_chunk_len = 0;
        let mut read = 0;

        // Read enough bytes to detect the BOM.
        while first_chunk_len < BOM_MAX_LEN {
            read = file_read_uninit(file, &mut buf[first_chunk_len..])?;
            if read == 0 {
                break;
            }
            first_chunk_len += read;
        }

        if let Some(encoding) = encoding {
            self.encoding = encoding;
        } else {
            let bom = detect_bom(unsafe { buf[..first_chunk_len].assume_init_ref() });
            self.encoding = bom.unwrap_or("UTF-8");
        }

        // TODO: Since reading the file can fail, we should ensure that we also reset the cursor here.
        // I don't do it, so that `recalc_after_content_swap()` works.
        self.buffer.clear();

        let done = read == 0;
        if self.encoding == "UTF-8" {
            self.read_file_as_utf8(file, &mut buf, first_chunk_len, done)?;
        } else {
            self.read_file_with_icu(file, &mut buf, first_chunk_len, done)?;
        }

        // Figure out
        // * the logical line count
        // * the newline type (LF or CRLF)
        // * the indentation type (tabs or spaces)
        // * whether there's a final newline
        {
            let chunk = self.read_forward(0);
            let mut offset = 0;
            let mut lines = 0;
            // Number of lines ending in CRLF.
            let mut crlf_count = 0;
            // Number of lines starting with a tab.
            let mut tab_indentations = 0;
            // Number of lines starting with a space.
            let mut space_indentations = 0;
            // Histogram of the indentation depth of lines starting with between 2 and 8 spaces.
            // In other words, `space_indentation_sizes[0]` is the number of lines starting with 2 spaces.
            let mut space_indentation_sizes = [0; 7];

            loop {
                // Check if the line starts with a tab.
                if offset < chunk.len() && chunk[offset] == b'\t' {
                    tab_indentations += 1;
                } else {
                    // Otherwise, check how many spaces the line starts with. Searching for >8 spaces
                    // allows us to reject lines that have more than 1 level of indentation.
                    let space_indentation =
                        chunk[offset..].iter().take(9).take_while(|&&c| c == b' ').count();

                    // We'll also reject lines starting with 1 space, because that's too fickle as a heuristic.
                    if (2..=8).contains(&space_indentation) {
                        space_indentations += 1;

                        // If we encounter an indentation depth of 6, it may either be a 6-space indentation,
                        // two 3-space indentation or 3 2-space indentations. To make this work, we increment
                        // all 3 possible histogram slots.
                        //   2 -> 2
                        //   3 -> 3
                        //   4 -> 4 2
                        //   5 -> 5
                        //   6 -> 6 3 2
                        //   7 -> 7
                        //   8 -> 8 4 2
                        space_indentation_sizes[space_indentation - 2] += 1;
                        if space_indentation & 4 != 0 {
                            space_indentation_sizes[0] += 1;
                        }
                        if space_indentation == 6 || space_indentation == 8 {
                            space_indentation_sizes[space_indentation / 2 - 2] += 1;
                        }
                    }
                }

                (offset, lines) = simd::lines_fwd(chunk, offset, lines, lines + 1);

                // Check if the preceding line ended in CRLF.
                if offset >= 2 && &chunk[offset - 2..offset] == b"\r\n" {
                    crlf_count += 1;
                }

                // We'll limit our heuristics to the first 1000 lines.
                // That should hopefully be enough in practice.
                if offset >= chunk.len() || lines >= 1000 {
                    break;
                }
            }

            // We'll assume CRLF if more than half of the lines end in CRLF.
            let newlines_are_crlf =
                if lines == 0 { self.newlines_are_crlf } else { crlf_count > lines / 2 };

            // We'll assume tabs if there are more lines starting with tabs than with spaces.
            let indent_with_tabs = tab_indentations > space_indentations;
            let tab_size = if indent_with_tabs {
                // Tabs will get a visual size of 4 spaces by default.
                4
            } else {
                // Otherwise, we'll assume the most common indentation depth.
                // If there are conflicting indentation depths, we'll prefer the maximum, because in the loop
                // above we incremented the histogram slot for 2-spaces when encountering 4-spaces and so on.
                let mut max = 1;
                let mut tab_size = 4;
                for (i, &count) in space_indentation_sizes.iter().enumerate() {
                    if count >= max {
                        max = count;
                        tab_size = i as CoordType + 2;
                    }
                }
                tab_size
            };

            // If the file has more than 1000 lines, figure out how many are remaining.
            if offset < chunk.len() {
                (_, lines) = simd::lines_fwd(chunk, offset, lines, CoordType::MAX);
            }

            let final_newline = chunk.ends_with(b"\n");

            // Add 1, because the last line doesn't end in a newline (it ends in the literal end).
            self.stats.logical_lines = lines + 1;
            self.stats.visual_lines = self.stats.logical_lines;
            self.newlines_are_crlf = newlines_are_crlf;
            self.insert_final_newline = final_newline;
            self.indent_with_tabs = indent_with_tabs;
            self.tab_size = tab_size;
        }

        self.recalc_after_content_swap();
        Ok(())
    }

    fn read_file_as_utf8(
        &mut self,
        file: &mut File,
        buf: &mut [MaybeUninit<u8>; 4 * KIBI],
        first_chunk_len: usize,
        done: bool,
    ) -> apperr::Result<()> {
        let file_len = if done {
            0
        } else {
            file.metadata().ok().and_then(|m| usize::try_from(m.len()).ok()).unwrap_or(0)
        };
        if file_len > 0 {
            self.buffer.reserve_empty(file_len)?;
        }
        {
            let mut first_chunk = unsafe { buf[..first_chunk_len].assume_init_ref() };
            if first_chunk.starts_with(b"\xEF\xBB\xBF") {
                first_chunk = &first_chunk[3..];
                self.encoding = "UTF-8 BOM";
            }

            if !self.buffer.reserve_insert(first_chunk.len()) {
                return Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory).into());
            }
            self.buffer.replace(0..0, first_chunk);
        }

        if done {
            return Ok(());
        }

        loop {
            // Metadata is a hint: streams may report zero, and files can grow.
            let chunk_size = if file_len > 0 {
                file_len.saturating_sub(self.text_length()).clamp(4 * KIBI, 8 * MEBI)
            } else {
                128 * KIBI
            };
            let gap = self.buffer.allocate_gap(self.text_length(), chunk_size, 0);
            if gap.is_empty() {
                // Do not present a truncated file as a successful load.
                let mut probe = [0];
                if file.read(&mut probe)? == 0 {
                    break;
                }
                return Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory).into());
            }

            let read = file.read(gap)?;
            if read == 0 {
                break;
            }

            self.buffer.commit_gap(read);
        }

        Ok(())
    }

    fn read_file_with_icu(
        &mut self,
        file: &mut File,
        buf: &mut [MaybeUninit<u8>; 4 * KIBI],
        first_chunk_len: usize,
        mut done: bool,
    ) -> apperr::Result<()> {
        let scratch = scratch_arena(None);
        let pivot_buffer = scratch.alloc_uninit_slice(4 * KIBI);
        let mut c = icu::Converter::new(pivot_buffer, self.encoding, "UTF-8")?;
        let mut first_chunk = unsafe { buf[..first_chunk_len].assume_init_ref() };

        while !first_chunk.is_empty() {
            let off = self.text_length();
            let gap = self.buffer.allocate_gap(off, 8 * KIBI, 0);
            if gap.is_empty() {
                return Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory).into());
            }
            let (input_advance, mut output_advance) =
                c.convert(first_chunk, slice_as_uninit_mut(gap))?;
            if input_advance == 0 && output_advance == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidData).into());
            }

            // Remove the BOM from the file, if this is the first chunk.
            // Our caller ensures to only call us once the BOM has been identified,
            // which means that if there's a BOM it must be wholly contained in this chunk.
            if off == 0 {
                let written = &mut gap[..output_advance];
                if written.starts_with(b"\xEF\xBB\xBF") {
                    written.copy_within(3.., 0);
                    output_advance -= 3;
                }
            }

            self.buffer.commit_gap(output_advance);
            first_chunk = &first_chunk[input_advance..];
        }

        let mut buf_len = 0;

        loop {
            if !done {
                let read = file_read_uninit(file, &mut buf[buf_len..])?;
                buf_len += read;
                done = read == 0;
            }

            let gap = self.buffer.allocate_gap(self.text_length(), 8 * KIBI, 0);
            if gap.is_empty() {
                if done && buf_len == 0 {
                    break;
                }
                return Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory).into());
            }

            let read = unsafe { buf[..buf_len].assume_init_ref() };
            let (input_advance, output_advance) = c.convert(read, slice_as_uninit_mut(gap))?;
            if !read.is_empty() && input_advance == 0 && output_advance == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidData).into());
            }

            self.buffer.commit_gap(output_advance);

            let flush = done && buf_len == 0;
            buf_len -= input_advance;
            buf.copy_within(input_advance.., 0);

            if flush && output_advance == 0 {
                break;
            }
        }

        Ok(())
    }

    /// Writes the text buffer contents to a file, handling BOM and encoding.
    pub fn write_file(&mut self, file: &mut File) -> apperr::Result<()> {
        self.write_file_staged(file)?;
        self.mark_saved();
        Ok(())
    }

    /// Serialize without declaring the document saved before its atomic commit.
    pub fn write_file_staged(&mut self, file: &mut File) -> apperr::Result<()> {
        let mut offset = 0;

        if self.encoding.starts_with("UTF-8") {
            if self.encoding == "UTF-8 BOM" {
                file.write_all(b"\xEF\xBB\xBF")?;
            }
            loop {
                let chunk = self.read_forward(offset);
                if chunk.is_empty() {
                    break;
                }
                file.write_all(chunk)?;
                offset += chunk.len();
            }
        } else {
            self.write_file_with_icu(file)?;
        }

        Ok(())
    }

    /// Mark a successfully committed save as the clean document generation.
    pub fn mark_saved(&mut self) {
        self.mark_as_clean();
    }

    fn write_file_with_icu(&mut self, file: &mut File) -> apperr::Result<()> {
        // Keep conversion chunks small without issuing one OS write per chunk.
        let mut file = std::io::BufWriter::with_capacity(128 * KIBI, file);
        let scratch = scratch_arena(None);
        let pivot_buffer = scratch.alloc_uninit_slice(4 * KIBI);
        let buf = scratch.alloc_uninit_slice(4 * KIBI);
        let mut c = icu::Converter::new(pivot_buffer, "UTF-8", self.encoding)?;
        let mut offset = 0;

        // Write the BOM for the encodings we know need it.
        if self.encoding.starts_with("UTF-16")
            || self.encoding.starts_with("UTF-32")
            || self.encoding == "GB18030"
        {
            let (_, output_advance) = c.convert(b"\xEF\xBB\xBF", buf)?;
            let chunk = unsafe { buf[..output_advance].assume_init_ref() };
            file.write_all(chunk)?;
        }

        loop {
            let input = self.read_forward(offset);
            let (input_advance, output_advance) = c.convert(input, buf)?;
            let chunk = unsafe { buf[..output_advance].assume_init_ref() };

            file.write_all(chunk)?;
            offset += input_advance;

            if input.is_empty() && output_advance == 0 {
                break;
            }
            if !input.is_empty() && input_advance == 0 && output_advance == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidData).into());
            }
        }
        file.flush()?;
        Ok(())
    }

    /// Returns the current selection.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// Number of active selections, including empty cursors.
    pub fn selection_count(&self) -> usize {
        self.secondary_selections.len() + 1
    }

    pub fn set_syntax_path(&mut self, path: &std::path::Path) {
        self.set_syntax_path_with_language(path, None);
    }

    pub fn set_syntax_path_with_language(
        &mut self,
        path: &std::path::Path,
        language: Option<&str>,
    ) {
        if self.syntax_path.as_deref() == Some(path) && self.syntax_language.as_deref() == language
        {
            return;
        }
        self.syntax_path = Some(path.to_owned());
        self.syntax_language = language.map(str::to_owned);
        self.syntax = match language {
            Some(language) => crate::syntax::SyntaxCache::from_language(language),
            None => crate::syntax::SyntaxCache::for_path(path),
        };
    }

    pub fn set_syntax_enabled(&mut self, enabled: bool) {
        self.syntax_enabled = enabled;
    }

    pub fn jump_to_matching_bracket(&mut self) -> bool {
        let Some(mut syntax) = self.syntax.take() else {
            return false;
        };
        let target =
            syntax.matching_bracket(&*self.buffer, self.content_revision, self.cursor.offset);
        self.syntax = Some(syntax);
        if let Some(target) = target {
            self.cursor_move_to_offset(target);
            self.make_cursor_visible();
            true
        } else {
            false
        }
    }

    pub fn clear_secondary_cursors(&mut self) -> bool {
        let changed = !self.secondary_selections.is_empty();
        self.secondary_selections.clear();
        self.rectangle = None;
        changed
    }

    /// Current byte anchors, sorted by document order. Reversed selections retain
    /// their direction. Empty selections represent insertion cursors.
    pub fn selection_ranges(&self) -> Vec<(usize, usize)> {
        let mut ranges = self.secondary_selections.clone();
        ranges.push(match self.selection {
            Some(s) => (
                self.cursor_move_to_logical_internal(self.cursor, s.beg).offset,
                self.cursor_move_to_logical_internal(self.cursor, s.end).offset,
            ),
            None => (self.cursor.offset, self.cursor.offset),
        });
        ranges.sort_by_key(|&(a, b)| (a.min(b), a.max(b)));
        ranges
    }

    /// Install selections, clamping to grapheme boundaries and coalescing
    /// overlaps so no byte can be replaced twice in a single transaction.
    pub fn set_selection_ranges(&mut self, ranges: &[(usize, usize)]) {
        let mut normalized: Vec<(usize, usize)> = ranges
            .iter()
            .map(|&(a, b)| {
                (
                    self.cursor_move_to_offset_internal(self.cursor, a).offset,
                    self.cursor_move_to_offset_internal(self.cursor, b).offset,
                )
            })
            .collect();
        normalized.sort_by_key(|&(a, b)| (a.min(b), a.max(b)));
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (a, b) in normalized {
            let start = a.min(b);
            let end = a.max(b);
            if let Some(last) = merged.last_mut() {
                let last_start = last.0.min(last.1);
                let last_end = last.0.max(last.1);
                if start < last_end || (start == last_end && (a == b || last.0 == last.1)) {
                    let new_end = end.max(last_end);
                    *last =
                        if last.0 > last.1 { (new_end, last_start) } else { (last_start, new_end) };
                    continue;
                }
            }
            merged.push((a, b));
        }
        let Some((a, b)) = merged.pop() else {
            return;
        };
        let anchor = self.cursor_move_to_offset_internal(self.cursor, a);
        let head = self.cursor_move_to_offset_internal(anchor, b);
        self.set_cursor_internal(head);
        self.set_selection(Some(TextBufferSelection {
            beg: anchor.logical_pos,
            end: head.logical_pos,
        }));
        self.secondary_selections = merged;
        self.last_history_type = HistoryType::Other;
        self.wants_cursor_visibility = true;
    }

    /// Select the current word, or add the next literal occurrence with wraparound.
    /// Existing ranges are skipped; repeated calls stop when every match is selected.
    pub fn add_next_occurrence(&mut self) -> bool {
        if self.selection.is_none() {
            self.select_word();
            return self.selection.is_some();
        }
        let Some((beg, end)) = self.selection_range() else {
            return false;
        };
        let pattern = self.extract_bytes(beg.offset..end.offset);
        let mut ranges = self.selection_ranges();
        let matches = find_all_matches(&self.buffer, &pattern, usize::MAX);
        let next = matches
            .iter()
            .filter(|r| r.start >= end.offset)
            .chain(matches.iter().filter(|r| r.start < end.offset))
            .find(|r| !ranges.iter().any(|&(a, b)| r.start < a.max(b) && r.end > a.min(b)));
        if let Some(range) = next {
            ranges.push((range.start, range.end));
            self.set_selection_ranges(&ranges);
            true
        } else {
            false
        }
    }

    /// Add an insertion cursor on an adjacent visual row. The latest cursor is
    /// the one used for the next addition; short lines clamp at their end.
    pub fn add_cursor_vertical(&mut self, delta: CoordType) {
        let ranges = self.selection_ranges();
        let &(a, b) = if delta < 0 { ranges.first().unwrap() } else { ranges.last().unwrap() };
        let cursor = self.cursor_move_to_offset_internal(self.cursor, b.max(a));
        let y = cursor.visual_pos.y + delta;
        if y < 0 || y >= self.stats.visual_lines {
            return;
        }
        let target =
            self.cursor_move_to_visual_internal(cursor, Point { x: cursor.visual_pos.x, y });
        let mut ranges = ranges;
        ranges.push((target.offset, target.offset));
        self.set_selection_ranges(&ranges);
    }

    pub fn add_cursor_visual(&mut self, pos: Point) {
        let mut ranges = self.selection_ranges();
        let target = self.cursor_move_to_visual_internal(self.cursor, pos);
        ranges.push((target.offset, target.offset));
        self.set_selection_ranges(&ranges);
    }

    /// Select a rectangle in terminal display columns (tabs and wide Unicode
    /// characters remain whole). Rows beyond EOF are never manufactured.
    pub fn select_rectangle(&mut self, anchor: Point, head: Point) {
        let top = anchor.y.min(head.y).max(0);
        let bottom = anchor.y.max(head.y).min(self.stats.visual_lines - 1);
        let mut ranges = Vec::new();
        let mut cursor = self.cursor;
        for y in top..=bottom {
            let a = self.cursor_move_to_visual_internal(cursor, Point { x: anchor.x, y });
            let b = self.cursor_move_to_visual_internal(a, Point { x: head.x, y });
            ranges.push((a.offset, b.offset));
            cursor = b;
        }
        self.set_selection_ranges(&ranges);
        self.rectangle = Some((anchor, head));
    }

    pub fn extend_rectangle(&mut self, dx: CoordType, dy: CoordType) {
        let (anchor, head) =
            self.rectangle.unwrap_or((self.cursor.visual_pos, self.cursor.visual_pos));
        let head = Point {
            x: (head.x + dx).max(0),
            y: (head.y + dy).clamp(0, self.stats.visual_lines - 1),
        };
        self.select_rectangle(anchor, head);
    }

    pub fn move_cursors_delta(
        &mut self,
        granularity: CursorMovement,
        delta: CoordType,
        select: bool,
    ) {
        let ranges: Vec<_> = self
            .selection_ranges()
            .into_iter()
            .map(|(a, b)| {
                if !select && a != b {
                    let end = if delta < 0 { a.min(b) } else { a.max(b) };
                    return (end, end);
                }
                let head = self.cursor_move_to_offset_internal(self.cursor, b);
                let end = self.cursor_move_delta_internal(head, granularity, delta).offset;
                (if select { a } else { end }, end)
            })
            .collect();
        self.set_selection_ranges(&ranges);
        self.rectangle = None;
    }

    pub fn move_cursors_vertical(&mut self, delta: CoordType, select: bool) {
        let ranges: Vec<_> = self
            .selection_ranges()
            .into_iter()
            .map(|(a, b)| {
                let head = self.cursor_move_to_offset_internal(self.cursor, b);
                let end = self
                    .cursor_move_to_visual_internal(
                        head,
                        Point { x: head.visual_pos.x, y: head.visual_pos.y + delta },
                    )
                    .offset;
                (if select { a } else { end }, end)
            })
            .collect();
        self.set_selection_ranges(&ranges);
        self.rectangle = None;
    }

    pub fn move_cursors_line_boundary(&mut self, to_end: bool, select: bool) {
        let ranges: Vec<_> = self
            .selection_ranges()
            .into_iter()
            .map(|(a, b)| {
                let head = self.cursor_move_to_offset_internal(self.cursor, b);
                let end = self
                    .cursor_move_to_logical_internal(
                        head,
                        Point { x: if to_end { CoordType::MAX } else { 0 }, y: head.logical_pos.y },
                    )
                    .offset;
                (if select { a } else { end }, end)
            })
            .collect();
        self.set_selection_ranges(&ranges);
        self.rectangle = None;
    }

    /// Group logical commands (for macro playback) into one undo operation.
    /// Groups may nest; pair every begin with an end, including on errors.
    pub fn begin_undo_group(&mut self) {
        self.edit_begin_grouping();
    }
    pub fn end_undo_group(&mut self) {
        self.edit_end_grouping();
        self.last_history_type = HistoryType::Other;
    }

    /// Roll back the outermost command transaction, including its cursor state.
    /// Used when a macro hits the capacity limit partway through playback.
    pub fn abort_undo_group(&mut self) {
        let Some(info) = self.active_edit_group.take() else {
            return;
        };
        self.edit_group_depth = 0;
        if self.buffer.generation() != info.generation_before {
            self.undo();
        }
        self.redo_stack = info.redo_before;
        self.cursor = self.cursor_move_to_logical_internal(self.cursor, info.cursor_before);
        self.set_selection(info.selection_before);
        self.secondary_selections = info.secondary_before;
        self.rectangle = None;
        self.last_history_type = HistoryType::Other;
    }

    pub fn take_edit_capacity_error(&mut self) -> bool {
        mem::take(&mut self.edit_capacity_error)
    }

    /// Apply validated, disjoint byte replacements as one all-or-nothing undo step.
    /// Source bytes are preserved exactly, including line-ending style.
    pub fn apply_byte_edits(
        &mut self,
        edits: &[(Range<usize>, Vec<u8>)],
    ) -> Result<(), &'static str> {
        let mut edits: Vec<_> = edits.iter().collect();
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let mut end = 0;
        let mut previous_start = None;
        let mut validation_cursor = Cursor::default();
        for (range, _) in &edits {
            if range.start > range.end
                || range.end > self.text_length()
                || range.start < end
                || previous_start == Some(range.start)
            {
                return Err("Edits overlap or fall outside the document");
            }
            let start_cursor = self.cursor_move_to_offset_internal(validation_cursor, range.start);
            let end_cursor = self.cursor_move_to_offset_internal(start_cursor, range.end);
            if start_cursor.offset != range.start || end_cursor.offset != range.end {
                return Err("Edit splits a character or line ending");
            }
            validation_cursor = end_cursor;
            end = range.end;
            previous_start = Some(range.start);
        }
        let mut length = self.text_length();
        let mut maximum = length;
        for (range, replacement) in edits.iter().rev() {
            length = (length - range.len())
                .checked_add(replacement.len())
                .ok_or("Edit exceeds buffer capacity")?;
            maximum = maximum.max(length);
        }
        let needed = maximum - self.text_length();
        if !self.buffer.reserve_insert(needed) {
            return Err("Edit exceeds buffer capacity");
        }
        self.edit_begin_grouping();
        self.secondary_selections.clear();
        for (range, replacement) in edits.into_iter().rev() {
            if range.is_empty() && replacement.is_empty() {
                continue;
            }
            let beg = self.cursor_move_to_offset_internal(self.cursor, range.start);
            let end = self.cursor_move_to_offset_internal(beg, range.end);
            self.edit_begin(HistoryType::Other, beg);
            self.edit_delete(end);
            self.edit_write(replacement);
            self.edit_end();
        }
        self.set_selection(None);
        self.rectangle = None;
        self.edit_end_grouping();
        Ok(())
    }

    // Upper bound includes canonical tab expansion and indentation copied at
    // every newline. Reserving before deletion makes capacity failures atomic.
    fn write_capacity_bound(&self, text: &[u8], raw: bool, at: Cursor) -> usize {
        if text.is_empty() {
            return 0;
        }
        let newlines = text.iter().filter(|&&b| b == b'\n' || b == b'\r').count();
        if raw {
            return text.len().saturating_add(newlines).saturating_add(2);
        }
        let expanded = text.len().saturating_mul(self.tab_size as usize);
        let indent = at.column.max(0) as usize;
        expanded
            .saturating_add(
                newlines.saturating_mul(indent.saturating_add(expanded).saturating_add(2)),
            )
            .saturating_add(2)
    }

    fn write_multiple(&mut self, text: &[u8], raw: bool) {
        let texts = vec![text; self.selection_count()];
        self.write_multiple_texts(&texts, raw);
    }

    fn write_multiple_fragments(&mut self, fragments: &[Vec<u8>]) {
        let texts: Vec<_> = fragments.iter().map(Vec::as_slice).collect();
        self.write_multiple_texts(&texts, true);
    }

    fn write_multiple_texts(&mut self, texts: &[&[u8]], raw: bool) {
        let ranges = self.selection_ranges();
        let mut length = self.text_length();
        let mut maximum = length;
        for (&(a, b), text) in ranges.iter().zip(texts).rev() {
            let at = self.cursor_move_to_offset_internal(self.cursor, a.min(b));
            length =
                (length - a.abs_diff(b)).saturating_add(self.write_capacity_bound(text, raw, at));
            maximum = maximum.max(length);
        }
        let needed = maximum - self.text_length();
        if !self.buffer.reserve_insert(needed) {
            self.edit_capacity_error = true;
            return;
        }
        self.edit_begin_grouping();
        self.secondary_selections.clear();
        self.rectangle = None;
        let mut cursors: Vec<(usize, usize)> = Vec::new();
        for ((a, b), text) in ranges.into_iter().zip(texts).rev() {
            let anchor = self.cursor_move_to_offset_internal(self.cursor, a);
            let head = self.cursor_move_to_offset_internal(anchor, b);
            self.set_cursor_internal(head);
            self.set_selection(Some(TextBufferSelection {
                beg: anchor.logical_pos,
                end: head.logical_pos,
            }));
            self.last_history_type = HistoryType::Other;
            let before = self.text_length();
            self.write(text, head, raw);
            let after = self.text_length();
            for (a, b) in &mut cursors {
                *a = if after >= before { *a + after - before } else { *a - (before - after) };
                *b = *a;
            }
            cursors.push((self.cursor.offset, self.cursor.offset));
        }
        self.set_selection_ranges(&cursors);
        self.edit_end_grouping();
    }

    fn set_selection(&mut self, selection: Option<TextBufferSelection>) -> u32 {
        self.selection = selection.filter(|s| s.beg != s.end);
        self.selection_generation = self.selection_generation.wrapping_add(1);
        self.selection_generation
    }

    /// Moves the cursor by `offset` and updates the selection to contain it.
    pub fn selection_update_offset(&mut self, offset: usize) {
        self.set_cursor_for_selection(self.cursor_move_to_offset_internal(self.cursor, offset));
    }

    /// Moves the cursor to `visual_pos` and updates the selection to contain it.
    pub fn selection_update_visual(&mut self, visual_pos: Point) {
        self.set_cursor_for_selection(self.cursor_move_to_visual_internal(self.cursor, visual_pos));
    }

    /// Moves the cursor to `logical_pos` and updates the selection to contain it.
    pub fn selection_update_logical(&mut self, logical_pos: Point) {
        self.set_cursor_for_selection(
            self.cursor_move_to_logical_internal(self.cursor, logical_pos),
        );
    }

    /// Moves the cursor by `delta` and updates the selection to contain it.
    pub fn selection_update_delta(&mut self, granularity: CursorMovement, delta: CoordType) {
        self.set_cursor_for_selection(self.cursor_move_delta_internal(
            self.cursor,
            granularity,
            delta,
        ));
    }

    /// Select the current word.
    pub fn select_word(&mut self) {
        let Range { start, end } = navigation::word_select(&*self.buffer, self.cursor.offset);
        let beg = self.cursor_move_to_offset_internal(self.cursor, start);
        let end = self.cursor_move_to_offset_internal(beg, end);
        unsafe { self.set_cursor(end) };
        self.set_selection(Some(TextBufferSelection {
            beg: beg.logical_pos,
            end: end.logical_pos,
        }));
    }

    /// Select the current line.
    pub fn select_line(&mut self) {
        let beg = self.cursor_move_to_logical_internal(
            self.cursor,
            Point { x: 0, y: self.cursor.logical_pos.y },
        );
        let end = self
            .cursor_move_to_logical_internal(beg, Point { x: 0, y: self.cursor.logical_pos.y + 1 });
        unsafe { self.set_cursor(end) };
        self.set_selection(Some(TextBufferSelection {
            beg: beg.logical_pos,
            end: end.logical_pos,
        }));
    }

    /// Logical line under a visual row, including wrapped continuation rows.
    pub fn logical_line_at_visual_row(&self, row: CoordType) -> CoordType {
        self.cursor_move_to_visual_internal(self.cursor, Point { x: 0, y: row }).logical_pos.y
    }

    /// Select inclusive logical lines, retaining the drag direction and their
    /// terminators. The final unterminated line naturally ends at EOF.
    pub fn select_lines(&mut self, anchor: CoordType, head: CoordType) {
        let last = self.logical_line_count() - 1;
        let anchor = anchor.clamp(0, last);
        let head = head.clamp(0, last);
        let first = self.goto_line_start(self.cursor, anchor.min(head));
        let end =
            self.cursor_move_to_logical_internal(first, Point { x: 0, y: anchor.max(head) + 1 });
        let (beg, end) = if head < anchor { (end, first) } else { (first, end) };
        self.clear_secondary_cursors();
        self.set_cursor_internal(end);
        self.set_selection(Some(TextBufferSelection {
            beg: beg.logical_pos,
            end: end.logical_pos,
        }));
        self.last_history_type = HistoryType::Other;
    }

    /// The fixed line for extending a selection through the gutter. A reverse
    /// range anchored at the next line's start ends on the preceding line.
    pub fn selection_anchor_line(&self) -> CoordType {
        self.selection.map_or(self.cursor.logical_pos.y, |selection| {
            selection.beg.y - CoordType::from(selection.beg.x == 0 && selection.beg > selection.end)
        })
    }

    /// Select the entire document.
    pub fn select_all(&mut self) {
        let beg = Default::default();
        let end = self.cursor_move_to_logical_internal(beg, Point::MAX);
        unsafe { self.set_cursor(end) };
        self.set_selection(Some(TextBufferSelection {
            beg: beg.logical_pos,
            end: end.logical_pos,
        }));
    }

    /// Starts a new selection, if there's none already.
    pub fn start_selection(&mut self) {
        if self.selection.is_none() {
            self.set_selection(Some(TextBufferSelection {
                beg: self.cursor.logical_pos,
                end: self.cursor.logical_pos,
            }));
        }
    }

    /// Destroy the current selection.
    pub fn clear_selection(&mut self) -> bool {
        let had_selection = self.selection.is_some() || !self.secondary_selections.is_empty();
        self.clear_secondary_cursors();
        self.set_selection(None);
        had_selection
    }

    /// Find the next occurrence of the given `pattern` and select it.
    pub fn find_and_select(&mut self, pattern: &str, options: SearchOptions) -> apperr::Result<()> {
        self.clear_secondary_cursors();
        if let Some(search) = &mut self.search {
            let search = search.get_mut();
            // When the search input changes we must reset the search.
            if search.pattern != pattern || search.options != options {
                self.search = None;
            }

            // When transitioning from some search to no search, we must clear the selection.
            if pattern.is_empty()
                && let Some(TextBufferSelection { beg, .. }) = self.selection
            {
                self.cursor_move_to_logical(beg);
            }
        }

        if pattern.is_empty() {
            return Ok(());
        }

        let search = match &self.search {
            Some(search) => unsafe { &mut *search.get() },
            None => {
                let search = self.find_construct_search(pattern, options)?;
                self.search = Some(UnsafeCell::new(search));
                unsafe { &mut *self.search.as_ref().unwrap().get() }
            }
        };

        // If we previously searched through the entire document and found 0 matches,
        // then we can avoid searching again.
        if search.no_matches && search.buffer_generation == self.content_revision {
            return Ok(());
        }

        // If the user moved the cursor since the last search, but the needle remained the same,
        // we still need to move the start of the search to the new cursor position.
        let next_search_offset = if self.selection_generation == search.selection_generation {
            search.next_search_offset
        } else {
            match self.selection {
                Some(TextBufferSelection { beg, end }) => {
                    self.cursor_move_to_logical_internal(self.cursor, beg.min(end)).offset
                }
                _ => self.cursor.offset,
            }
        };

        self.find_select_next(search, next_search_offset, true);
        Ok(())
    }

    /// Find the next occurrence of the given `pattern` and replace it with `replacement`.
    pub fn find_and_replace(
        &mut self,
        pattern: &str,
        options: SearchOptions,
        replacement: &[u8],
    ) -> apperr::Result<()> {
        // Editors traditionally replace the previous search hit, not the next possible one.
        if let Some(search) = &self.search {
            let search = unsafe { &mut *search.get() };
            if search.selection_generation == self.selection_generation
                && !search.no_matches
                && search.pattern == pattern
                && search.options == options
            {
                let scratch = scratch_arena(None);
                let zero_width = self.selection_range().is_none();
                let parsed_replacements =
                    Self::find_parse_replacement(&scratch, &mut *search, replacement);
                let replacement =
                    self.find_fill_replacement(&mut *search, replacement, &parsed_replacements);
                self.write(&replacement, self.cursor, true);
                if zero_width {
                    let offset = if replacement.is_empty() {
                        self.cursor.offset
                    } else {
                        self.active_edit_off
                    };
                    search.next_search_offset =
                        self.find_advance_past_zero_width(offset).unwrap_or(0);
                }
            }
        }

        self.find_and_select(pattern, options)
    }

    /// Find all occurrences of the given `pattern` and replace them with `replacement`.
    pub fn find_and_replace_all(
        &mut self,
        pattern: &str,
        options: SearchOptions,
        replacement: &[u8],
    ) -> apperr::Result<()> {
        let scratch = scratch_arena(None);
        let mut search = self.find_construct_search(pattern, options)?;
        let mut offset = 0;
        let parsed_replacements = Self::find_parse_replacement(&scratch, &mut search, replacement);
        // Construct the regex before grouping so a bad pattern cannot leave an
        // unfinished transaction behind.
        self.edit_begin_grouping();
        while let Some(range) = self.find_select_next(&mut search, offset, false) {
            let replacement =
                self.find_fill_replacement(&mut search, replacement, &parsed_replacements);
            self.write(&replacement, self.cursor, true);
            // An empty replacement of an empty match is a no-op, so write()
            // does not update active_edit_off. Advance from the actual match.
            offset = if range.is_empty() && replacement.is_empty() {
                range.end
            } else {
                self.active_edit_off
            };
            if self.edit_capacity_error {
                break;
            }
            if range.is_empty() {
                let Some(next) = self.find_advance_past_zero_width(offset) else {
                    break;
                };
                offset = next;
            }
        }
        self.edit_end_grouping();
        Ok(())
    }

    fn find_advance_past_zero_width(&self, offset: usize) -> Option<usize> {
        let cursor = self.cursor_move_to_offset_internal(self.cursor, offset);
        let next = self.cursor_move_delta_internal(cursor, CursorMovement::Grapheme, 1);
        (next.offset > offset).then_some(next.offset)
    }

    fn find_construct_search(
        &self,
        pattern: &str,
        options: SearchOptions,
    ) -> apperr::Result<ActiveSearch> {
        if pattern.is_empty() {
            return Err(apperr::Error::Icu(1)); // U_ILLEGAL_ARGUMENT_ERROR
        }

        let sanitized_pattern = if options.whole_word && options.use_regex {
            Cow::Owned(format!(r"\b(?:{pattern})\b"))
        } else if options.whole_word {
            let mut p = String::with_capacity(pattern.len() + 16);
            p.push_str(r"\b");

            // Escape regex special characters.
            let b = unsafe { p.as_mut_vec() };
            for &byte in pattern.as_bytes() {
                match byte {
                    b'*' | b'?' | b'+' | b'[' | b'(' | b')' | b'{' | b'}' | b'^' | b'$' | b'|'
                    | b'\\' | b'.' => {
                        b.push(b'\\');
                        b.push(byte);
                    }
                    _ => b.push(byte),
                }
            }

            p.push_str(r"\b");
            Cow::Owned(p)
        } else {
            Cow::Borrowed(pattern)
        };

        let mut flags = icu::Regex::MULTILINE;
        if !options.match_case {
            flags |= icu::Regex::CASE_INSENSITIVE;
        }
        if !options.use_regex && !options.whole_word {
            flags |= icu::Regex::LITERAL;
        }

        // Move the start of the search to the start of the selection,
        // or otherwise to the current cursor position.

        let text = unsafe { icu::Text::new(&self.buffer)? };
        let regex = unsafe { icu::Regex::new(&sanitized_pattern, flags, &text)? };

        Ok(ActiveSearch {
            pattern: pattern.to_string(),
            options,
            text,
            regex,
            buffer_generation: self.content_revision,
            selection_generation: 0,
            next_search_offset: 0,
            no_matches: false,
        })
    }

    fn find_select_next(
        &mut self,
        search: &mut ActiveSearch,
        offset: usize,
        wrap: bool,
    ) -> Option<Range<usize>> {
        if search.buffer_generation != self.content_revision {
            unsafe { search.regex.set_text(&mut search.text, offset) };
            search.buffer_generation = self.content_revision;
            search.no_matches = false;
            search.next_search_offset = offset;
        } else if search.next_search_offset != offset {
            search.next_search_offset = offset;
            search.regex.reset(offset);
        }

        let mut hit = search.regex.next();

        // If we hit the end of the buffer, and we know that there's something to find,
        // start the search again from the beginning (= wrap around).
        if wrap && hit.is_none() && search.next_search_offset != 0 {
            search.next_search_offset = 0;
            search.regex.reset(0);
            hit = search.regex.next();
        }

        search.selection_generation = if let Some(range) = &hit {
            // Now the search offset is no more at the start of the buffer.
            search.next_search_offset = range.end;

            let beg = self.cursor_move_to_offset_internal(self.cursor, range.start);
            let end = self.cursor_move_to_offset_internal(beg, range.end);

            unsafe { self.set_cursor(end) };
            self.make_cursor_visible();

            self.set_selection(Some(TextBufferSelection {
                beg: beg.logical_pos,
                end: end.logical_pos,
            }))
        } else {
            // Avoid searching through the entire document again if we know there's nothing to find.
            search.no_matches = true;
            self.set_selection(None)
        };
        hit
    }

    fn find_parse_replacement<'a>(
        arena: &'a Arena,
        search: &mut ActiveSearch,
        replacement: &[u8],
    ) -> Vec<RegexReplacement<'a>, &'a Arena> {
        let mut res = Vec::new_in(arena);

        if !search.options.use_regex {
            return res;
        }

        let group_count = search.regex.group_count();
        let mut text = Vec::new_in(arena);
        let mut text_beg = 0;

        loop {
            let mut off = memchr2(b'$', b'\\', replacement, text_beg);

            // Push the raw, unescaped text, if any.
            if text_beg < off {
                text.extend_from_slice(&replacement[text_beg..off]);
            }

            // Unescape any escaped characters.
            while off < replacement.len() && replacement[off] == b'\\' {
                off += 2;

                // If this backslash is the last character (e.g. because
                // `replacement` is just 1 byte long, holding just b"\\"),
                // we can't unescape it. In that case, we map it to `b'\\'` here.
                // This results in us appending a literal backslash to the text.
                let ch = replacement.get(off - 1).map_or(b'\\', |&c| c);

                // Unescape and append the character.
                text.push(match ch {
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    ch => ch,
                });
            }

            // Parse out a group number, if any.
            let mut group = -1;
            if off < replacement.len() && replacement[off] == b'$' {
                let mut beg = off;
                let mut end = off + 1;
                let mut acc = 0i32;
                let mut acc_bad = true;

                if end < replacement.len() {
                    let ch = replacement[end];

                    if ch == b'$' {
                        // Translate "$$" to "$".
                        beg += 1;
                        end += 1;
                    } else if ch.is_ascii_digit() {
                        // Parse "$1234" into 1234i32.
                        // If the number is larger than the group count,
                        // we flag `acc_bad` which causes us to treat it as text.
                        acc_bad = false;
                        while {
                            acc =
                                acc.wrapping_mul(10).wrapping_add((replacement[end] - b'0') as i32);
                            acc_bad |= acc > group_count;
                            end += 1;
                            end < replacement.len() && replacement[end].is_ascii_digit()
                        } {}
                    }
                }

                if !acc_bad {
                    group = acc;
                } else {
                    text.extend_from_slice(&replacement[beg..end]);
                }

                off = end;
            }

            if !text.is_empty() {
                res.push(RegexReplacement::Text(text));
                text = Vec::new_in(arena);
            }
            if group >= 0 {
                res.push(RegexReplacement::Group(group));
            }

            text_beg = off;
            if text_beg >= replacement.len() {
                break;
            }
        }

        res
    }

    fn find_fill_replacement<'a>(
        &self,
        search: &mut ActiveSearch,
        replacement: &'a [u8],
        parsed_replacements: &[RegexReplacement],
    ) -> Cow<'a, [u8]> {
        if !search.options.use_regex {
            Cow::Borrowed(replacement)
        } else {
            let mut res = Vec::new();

            for replacement in parsed_replacements {
                match replacement {
                    RegexReplacement::Text(text) => res.extend_from_slice(text),
                    RegexReplacement::Group(group) => {
                        if let Some(range) = search.regex.group(*group) {
                            self.buffer.extract_raw(range, &mut res, usize::MAX);
                        }
                    }
                }
            }

            Cow::Owned(res)
        }
    }

    fn measurement_config(&self) -> MeasurementConfig<'_> {
        MeasurementConfig::new(&*self.buffer)
            .with_word_wrap_column(self.word_wrap_column)
            .with_tab_size(self.tab_size)
    }

    fn goto_line_start(&self, cursor: Cursor, y: CoordType) -> Cursor {
        let mut result = cursor;
        let mut seek_to_line_start = true;

        if y > result.logical_pos.y {
            while y > result.logical_pos.y {
                let chunk = self.read_forward(result.offset);
                if chunk.is_empty() {
                    break;
                }

                let (delta, line) = simd::lines_fwd(chunk, 0, result.logical_pos.y, y);
                result.offset += delta;
                result.logical_pos.y = line;
            }

            // If we're at the end of the buffer, we could either be there because the last
            // character in the buffer is genuinely a newline, or because the buffer ends in a
            // line of text without trailing newline. The only way to make sure is to seek
            // backwards to the line start again. But otherwise we can skip that.
            seek_to_line_start =
                result.offset == self.text_length() && result.offset != cursor.offset;
        }

        if seek_to_line_start {
            loop {
                let chunk = self.read_backward(result.offset);
                if chunk.is_empty() {
                    break;
                }

                let (delta, line) = simd::lines_bwd(chunk, chunk.len(), result.logical_pos.y, y);
                result.offset -= chunk.len() - delta;
                result.logical_pos.y = line;
                if delta > 0 {
                    break;
                }
            }
        }

        if result.offset == cursor.offset {
            return result;
        }

        result.logical_pos.x = 0;
        result.visual_pos.x = 0;
        result.visual_pos.y = result.logical_pos.y;
        result.column = 0;
        result.wrap_opp = false;

        if self.word_wrap_column > 0 {
            let upward = result.offset < cursor.offset;
            let (top, bottom) = if upward { (result, cursor) } else { (cursor, result) };

            let mut bottom_remeasured =
                self.measurement_config().with_cursor(top).goto_logical(bottom.logical_pos);

            // The second problem is that visual positions can be ambiguous. A single logical position
            // can map to two visual positions: One at the end of the preceding line in front of
            // a word wrap, and another at the start of the next line after the same word wrap.
            //
            // This, however, only applies if we go upwards, because only then `bottom ≅ cursor`,
            // and thus only then this `bottom` is ambiguous. Otherwise, `bottom ≅ result`
            // and `result` is at a line start which is never ambiguous.
            if upward {
                let a = bottom_remeasured.visual_pos.x;
                let b = bottom.visual_pos.x;
                bottom_remeasured.visual_pos.y = bottom_remeasured.visual_pos.y
                    + (a != 0 && b == 0) as CoordType
                    - (a == 0 && b != 0) as CoordType;
            }

            let mut delta = bottom_remeasured.visual_pos.y - top.visual_pos.y;
            if upward {
                delta = -delta;
            }

            result.visual_pos.y = cursor.visual_pos.y + delta;
        }

        result
    }

    fn cursor_move_to_offset_internal(&self, mut cursor: Cursor, offset: usize) -> Cursor {
        if offset == cursor.offset {
            return cursor;
        }

        // goto_line_start() is fast for seeking across lines _if_ line wrapping is disabled.
        // For backward seeking we have to use it either way, so we're covered there.
        // This implements the forward seeking portion, if it's approx. worth doing so.
        if self.word_wrap_column <= 0 && offset.saturating_sub(cursor.offset) > 1024 {
            // Replacing this with a more optimal, direct memchr() loop appears
            // to improve performance only marginally by another 2% or so.
            // Still, it's kind of "meh" looking at how poorly this is implemented...
            loop {
                let next = self.goto_line_start(cursor, cursor.logical_pos.y + 1);
                // Stop when we either ran past the target offset,
                // or when we hit the end of the buffer and `goto_line_start` backtracked to the line start.
                if next.offset > offset || next.offset <= cursor.offset {
                    break;
                }
                cursor = next;
            }
        }

        while offset < cursor.offset {
            cursor = self.goto_line_start(cursor, cursor.logical_pos.y - 1);
        }

        self.measurement_config().with_cursor(cursor).goto_offset(offset)
    }

    fn cursor_move_to_logical_internal(&self, mut cursor: Cursor, pos: Point) -> Cursor {
        let pos = Point { x: pos.x.max(0), y: pos.y.max(0) };

        if pos == cursor.logical_pos {
            return cursor;
        }

        // goto_line_start() is the fastest way for seeking across lines. As such we always
        // use it if the requested `.y` position is different. We still need to use it if the
        // `.x` position is smaller, but only because `goto_logical()` cannot seek backwards.
        if pos.y != cursor.logical_pos.y || pos.x < cursor.logical_pos.x {
            cursor = self.goto_line_start(cursor, pos.y);
        }

        self.measurement_config().with_cursor(cursor).goto_logical(pos)
    }

    fn cursor_move_to_visual_internal(&self, mut cursor: Cursor, pos: Point) -> Cursor {
        let pos = Point { x: pos.x.max(0), y: pos.y.max(0) };

        if pos == cursor.visual_pos {
            return cursor;
        }

        if self.word_wrap_column <= 0 {
            // Identical to the fast-pass in `cursor_move_to_logical_internal()`.
            if pos.y != cursor.visual_pos.y || pos.x < cursor.visual_pos.x {
                cursor = self.goto_line_start(cursor, pos.y);
            }
        } else {
            // `goto_visual()` can only seek forward, so we need to seek backward here if needed.
            // NOTE that this intentionally doesn't use the `Eq` trait of `Point`, because if
            // `pos.y == cursor.visual_pos.y` we don't need to go to `cursor.logical_pos.y - 1`.
            while pos.y < cursor.visual_pos.y {
                cursor = self.goto_line_start(cursor, cursor.logical_pos.y - 1);
            }
            if pos.y == cursor.visual_pos.y && pos.x < cursor.visual_pos.x {
                cursor = self.goto_line_start(cursor, cursor.logical_pos.y);
            }
        }

        self.measurement_config().with_cursor(cursor).goto_visual(pos)
    }

    fn cursor_move_delta_internal(
        &self,
        mut cursor: Cursor,
        granularity: CursorMovement,
        mut delta: CoordType,
    ) -> Cursor {
        if delta == 0 {
            return cursor;
        }

        let sign = if delta > 0 { 1 } else { -1 };

        match granularity {
            CursorMovement::Grapheme => {
                let start_x = if delta > 0 { 0 } else { CoordType::MAX };

                loop {
                    let target_x = cursor.logical_pos.x + delta;

                    cursor = self.cursor_move_to_logical_internal(
                        cursor,
                        Point { x: target_x, y: cursor.logical_pos.y },
                    );

                    // We can stop if we ran out of remaining delta
                    // (or perhaps ran past the goal; in either case the sign would've changed),
                    // or if we hit the beginning or end of the buffer.
                    delta = target_x - cursor.logical_pos.x;
                    if delta.signum() != sign
                        || (delta < 0 && cursor.offset == 0)
                        || (delta > 0 && cursor.offset >= self.text_length())
                    {
                        break;
                    }

                    cursor = self.cursor_move_to_logical_internal(
                        cursor,
                        Point { x: start_x, y: cursor.logical_pos.y + sign },
                    );

                    // We crossed a newline which counts for 1 grapheme cluster.
                    // So, we also need to run the same check again.
                    delta -= sign;
                    if delta.signum() != sign
                        || cursor.offset == 0
                        || cursor.offset >= self.text_length()
                    {
                        break;
                    }
                }
            }
            CursorMovement::Word => {
                let doc = &*self.buffer as &dyn ReadableDocument;
                let mut offset = cursor.offset;

                while delta != 0 {
                    if delta < 0 {
                        offset = navigation::word_backward(doc, offset);
                    } else {
                        offset = navigation::word_forward(doc, offset);
                    }
                    delta -= sign;
                }

                cursor = self.cursor_move_to_offset_internal(cursor, offset);
            }
        }

        cursor
    }

    /// Moves the cursor to the given offset.
    pub fn cursor_move_to_offset(&mut self, offset: usize) {
        unsafe { self.set_cursor(self.cursor_move_to_offset_internal(self.cursor, offset)) }
    }

    /// Moves the cursor to the given logical position.
    pub fn cursor_move_to_logical(&mut self, pos: Point) {
        unsafe { self.set_cursor(self.cursor_move_to_logical_internal(self.cursor, pos)) }
    }

    /// Moves the cursor to the given visual position.
    pub fn cursor_move_to_visual(&mut self, pos: Point) {
        unsafe { self.set_cursor(self.cursor_move_to_visual_internal(self.cursor, pos)) }
    }

    /// Moves the cursor by the given delta.
    pub fn cursor_move_delta(&mut self, granularity: CursorMovement, delta: CoordType) {
        unsafe { self.set_cursor(self.cursor_move_delta_internal(self.cursor, granularity, delta)) }
    }

    /// Sets the cursor to the given position, and clears the selection.
    ///
    /// # Safety
    ///
    /// This function performs no checks that the cursor is valid. "Valid" in this case means
    /// that the TextBuffer has not been modified since you received the cursor from this class.
    pub unsafe fn set_cursor(&mut self, cursor: Cursor) {
        self.clear_secondary_cursors();
        self.set_cursor_internal(cursor);
        self.last_history_type = HistoryType::Other;
        self.set_selection(None);
    }

    fn set_cursor_for_selection(&mut self, cursor: Cursor) {
        self.clear_secondary_cursors();
        let beg = match self.selection {
            Some(TextBufferSelection { beg, .. }) => beg,
            None => self.cursor.logical_pos,
        };

        self.set_cursor_internal(cursor);
        self.last_history_type = HistoryType::Other;

        let end = self.cursor.logical_pos;
        self.set_selection(if beg == end { None } else { Some(TextBufferSelection { beg, end }) });
    }

    fn set_cursor_internal(&mut self, cursor: Cursor) {
        debug_assert!(
            cursor.offset <= self.text_length()
                && cursor.logical_pos.x >= 0
                && cursor.logical_pos.y >= 0
                && cursor.logical_pos.y <= self.stats.logical_lines
                && cursor.visual_pos.x >= 0
                && (self.word_wrap_column <= 0 || cursor.visual_pos.x <= self.word_wrap_column)
                && cursor.visual_pos.y >= 0
                && cursor.visual_pos.y <= self.stats.visual_lines,
            "cursor {cursor:?}, len {}, lines {}/{}, wrap {}",
            self.text_length(),
            self.stats.logical_lines,
            self.stats.visual_lines,
            self.word_wrap_column
        );
        self.cursor = cursor;
    }

    /// Extracts a rectangular region of the text buffer and writes it to the framebuffer.
    /// The `destination` rect is framebuffer coordinates. The extracted region within this
    /// text buffer has the given `origin` and the same size as the `destination` rect.
    pub fn render(
        &mut self,
        origin: Point,
        destination: Rect,
        focused: bool,
        fb: &mut Framebuffer,
    ) -> Option<RenderResult> {
        if destination.is_empty() {
            return None;
        }

        let width = destination.width();
        let height = destination.height();
        let line_number_width = self.margin_width.max(3) as usize - 3;
        let text_width = width - self.margin_width;
        let mut visualizer_buf = [0xE2, 0x90, 0x80]; // U+2400 in UTF8
        let mut visual_pos_x_max = 0;

        // Pick the cursor closer to the `origin.y`.
        let mut cursor = {
            let a = self.cursor;
            let b = self.cursor_for_rendering.unwrap_or_default();
            let da = (a.visual_pos.y - origin.y).abs();
            let db = (b.visual_pos.y - origin.y).abs();
            if da < db { a } else { b }
        };

        let [selection_beg, selection_end] = match self.selection {
            None => [Point::MIN, Point::MIN],
            Some(TextBufferSelection { beg, end }) => minmax(beg, end),
        };

        // Compute selection byte range for auto-highlight
        let selection_byte_range = if let Some((beg_cursor, end_cursor)) = self.selection_range() {
            beg_cursor.offset..end_cursor.offset
        } else {
            0..0
        };

        // Update auto-highlight matches
        self.selection_highlights.update(
            if selection_byte_range.is_empty() { None } else { Some(selection_byte_range.clone()) },
            self.selection_generation,
            &self.buffer,
        );

        let syntax_spans = if self.syntax_enabled {
            if let Some(mut syntax) = self.syntax.take() {
                let beg = self.cursor_move_to_visual_internal(cursor, origin).offset;
                let end = self
                    .cursor_move_to_visual_internal(
                        cursor,
                        Point { x: CoordType::MAX, y: origin.y + height },
                    )
                    .offset;
                let spans = syntax.paint(
                    &*self.buffer,
                    self.content_revision,
                    beg..end,
                    self.cursor.offset,
                );
                self.syntax = Some(syntax);
                spans
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        for y in 0..height {
            let scratch = scratch_arena(None);
            let mut line = ArenaString::new_in(&scratch);
            line.reserve(width as usize * 2);

            let visual_line = origin.y + y;
            let mut cursor_beg =
                self.cursor_move_to_visual_internal(cursor, Point { x: origin.x, y: visual_line });
            let cursor_end = self.cursor_move_to_visual_internal(
                cursor_beg,
                Point { x: origin.x + text_width, y: visual_line },
            );

            // Accelerate the next render pass by remembering where we started off.
            if y == 0 {
                self.cursor_for_rendering = Some(cursor_beg);
            }

            if line_number_width != 0 {
                if visual_line >= self.stats.visual_lines {
                    // Past the end of the buffer? Place "    | " in the margin.
                    // Since we know that we won't see line numbers greater than i64::MAX (9223372036854775807)
                    // any time soon, we can use a static string as the template (`MARGIN`) and slice it,
                    // because `line_number_width` can't possibly be larger than 19.
                    let off = 19 - line_number_width;
                    unsafe { std::hint::assert_unchecked(off < MARGIN_TEMPLATE.len()) };
                    line.push_str(&MARGIN_TEMPLATE[off..]);
                } else if self.word_wrap_column <= 0 || cursor_beg.logical_pos.x == 0 {
                    // Regular line? Place "123 | " in the margin.
                    _ = write!(line, "{:1$} │ ", cursor_beg.logical_pos.y + 1, line_number_width);
                } else {
                    // Wrapped line? Place " ... | " in the margin.
                    let number_width = (cursor_beg.logical_pos.y + 1).ilog10() as usize + 1;
                    _ = write!(
                        line,
                        "{0:1$}{0:∙<2$} │ ",
                        "",
                        line_number_width - number_width,
                        number_width
                    );
                    // Blending in the background color will "dim" the indicator dots.
                    let left = destination.left;
                    let top = destination.top + y;
                    fb.blend_fg(
                        Rect {
                            left,
                            top,
                            right: left + line_number_width as CoordType,
                            bottom: top + 1,
                        },
                        fb.indexed_alpha(IndexedColor::Background, 1, 2),
                    );
                }
            }

            let mut selection_off = 0..0;

            // Colorize only visible text; selections below retain their contrast.
            for span in &syntax_spans {
                let start = span.range.start.max(cursor_beg.offset);
                let end = span.range.end.min(cursor_end.offset);
                if start >= end || cursor_beg.visual_pos.y != visual_line {
                    continue;
                }
                let a = self.cursor_move_to_offset_internal(cursor_beg, start);
                let b = self.cursor_move_to_offset_internal(a, end);
                let left = destination.left + self.margin_width - origin.x;
                let rect = Rect {
                    left: (left + a.visual_pos.x).max(destination.left + self.margin_width),
                    top: destination.top + y,
                    right: if b.visual_pos.y > a.visual_pos.y {
                        destination.right
                    } else {
                        (left + b.visual_pos.x).min(destination.right)
                    },
                    bottom: destination.top + y + 1,
                };
                if let Some(color) = span.foreground {
                    fb.blend_fg(rect, fb.indexed(color));
                }
                if let Some(attributes) = span.attributes {
                    fb.replace_attr(rect, attributes, attributes);
                }
                if span.bracket {
                    fb.blend_bg(rect, fb.indexed_alpha(IndexedColor::Cyan, 1, 2));
                }
            }

            // Figure out the selection range on this line, if any.
            if cursor_beg.visual_pos.y == visual_line
                && selection_beg <= cursor_end.logical_pos
                && selection_end >= cursor_beg.logical_pos
            {
                let mut cursor = cursor_beg;

                // By default, we assume the entire line is selected.
                let mut selection_pos_beg = 0;
                let mut selection_pos_end = COORD_TYPE_SAFE_MAX;
                selection_off.start = cursor_beg.offset;
                selection_off.end = cursor_end.offset;

                // The start of the selection is within this line. We need to update selection_beg.
                if selection_beg <= cursor_end.logical_pos
                    && selection_beg >= cursor_beg.logical_pos
                {
                    cursor = self.cursor_move_to_logical_internal(cursor, selection_beg);
                    selection_off.start = cursor.offset;
                    selection_pos_beg = cursor.visual_pos.x;
                }

                // The end of the selection is within this line. We need to update selection_end.
                if selection_end <= cursor_end.logical_pos
                    && selection_end >= cursor_beg.logical_pos
                {
                    cursor = self.cursor_move_to_logical_internal(cursor, selection_end);
                    selection_off.end = cursor.offset;
                    selection_pos_end = cursor.visual_pos.x;
                }

                let left = destination.left + self.margin_width - origin.x;
                let top = destination.top + y;
                let rect = Rect {
                    left: left + selection_pos_beg.max(origin.x),
                    top,
                    right: left + selection_pos_end.min(origin.x + text_width),
                    bottom: top + 1,
                };

                let mut bg = fb.indexed(IndexedColor::Foreground).oklab_blend(fb.indexed_alpha(
                    IndexedColor::BrightBlue,
                    1,
                    2,
                ));
                if !focused {
                    bg = bg.oklab_blend(fb.indexed_alpha(IndexedColor::Background, 1, 2));
                };
                let fg = fb.contrasted(bg);
                fb.blend_bg(rect, bg);
                fb.blend_fg(rect, fg);
            }

            // Highlight auto-highlight matches on this line
            if cursor_beg.visual_pos.y == visual_line
                && !self.selection_highlights.matches.is_empty()
            {
                for match_range in &self.selection_highlights.matches {
                    // Check if this match overlaps with the visible line range
                    if match_range.end <= cursor_beg.offset
                        || match_range.start >= cursor_end.offset
                    {
                        continue;
                    }

                    let mut cursor = cursor_beg;

                    // Find start position of this match on screen
                    let match_start_offset = match_range.start.max(cursor_beg.offset);
                    let match_end_offset = match_range.end.min(cursor_end.offset);

                    // Navigate to match start
                    if match_start_offset > cursor.offset {
                        cursor = self.cursor_move_to_offset_internal(cursor, match_start_offset);
                    }
                    let match_pos_beg = cursor.visual_pos.x;

                    // Navigate to match end
                    cursor = self.cursor_move_to_offset_internal(cursor, match_end_offset);
                    let match_pos_end = cursor.visual_pos.x;

                    let left = destination.left + self.margin_width - origin.x;
                    let top = destination.top + y;
                    let rect = Rect {
                        left: left + match_pos_beg.max(origin.x),
                        top,
                        right: left + match_pos_end.min(origin.x + text_width),
                        bottom: top + 1,
                    };

                    // Use a subtle yellow/orange background for auto-highlights
                    let bg = fb.indexed(IndexedColor::Yellow).oklab_blend(fb.indexed_alpha(
                        IndexedColor::Background,
                        1,
                        2,
                    ));
                    fb.blend_bg(rect, bg);
                }
            }

            for &(anchor, head) in &self.secondary_selections {
                let start = anchor.min(head);
                let end = anchor.max(head);
                if start == end {
                    if head < cursor_beg.offset || head > cursor_end.offset {
                        continue;
                    }
                    let caret = self.cursor_move_to_offset_internal(cursor_beg, head);
                    if focused && caret.visual_pos.y == visual_line {
                        let left =
                            destination.left + self.margin_width + caret.visual_pos.x - origin.x;
                        if left >= destination.left + self.margin_width && left < destination.right
                        {
                            // A wide grapheme must share one attribute run or the
                            // terminal cannot draw it underlined. Tabs are expanded
                            // into individual spaces, so mark only their first cell.
                            let next =
                                self.cursor_move_delta_internal(caret, CursorMovement::Grapheme, 1);
                            let caret_width = if next.visual_pos.y == caret.visual_pos.y
                                && self.read_forward(head).first() != Some(&b'\t')
                            {
                                (next.visual_pos.x - caret.visual_pos.x).max(1)
                            } else {
                                1
                            };
                            let rect = Rect {
                                left,
                                top: destination.top + y,
                                right: (left + caret_width).min(destination.right),
                                bottom: destination.top + y + 1,
                            };
                            // Terminals expose one hardware cursor. Mark additional
                            // insertion points with an underline, preserving the
                            // character's colors so it never looks selected.
                            fb.replace_attr(rect, Attributes::Underlined, Attributes::Underlined);
                        }
                    }
                } else if start <= cursor_end.offset
                    && end > cursor_beg.offset
                    && cursor_beg.visual_pos.y == visual_line
                {
                    let a = self
                        .cursor_move_to_offset_internal(cursor_beg, start.max(cursor_beg.offset));
                    let b = self.cursor_move_to_offset_internal(a, end.min(cursor_end.offset));
                    let left = destination.left + self.margin_width - origin.x;
                    let rect = Rect {
                        left: (left + a.visual_pos.x).max(destination.left + self.margin_width),
                        top: destination.top + y,
                        right: if end > cursor_end.offset {
                            destination.right
                        } else {
                            (left + b.visual_pos.x).min(destination.right)
                        },
                        bottom: destination.top + y + 1,
                    };
                    let mut bg = fb
                        .indexed(IndexedColor::Foreground)
                        .oklab_blend(fb.indexed_alpha(IndexedColor::BrightBlue, 1, 2));
                    if !focused {
                        bg = bg.oklab_blend(fb.indexed_alpha(IndexedColor::Background, 1, 2));
                    }
                    fb.blend_bg(rect, bg);
                    fb.blend_fg(rect, fb.contrasted(bg));
                }
            }

            // Nothing to do if the entire line is empty.
            if cursor_beg.offset != cursor_end.offset {
                // If we couldn't reach the left edge, we may have stopped short due to a wide glyph.
                // In that case we'll try to find the next character and then compute by how many
                // columns it overlaps the left edge (can be anything between 1 and 7).
                if cursor_beg.visual_pos.x < origin.x {
                    let cursor_next = self.cursor_move_to_logical_internal(
                        cursor_beg,
                        Point { x: cursor_beg.logical_pos.x + 1, y: cursor_beg.logical_pos.y },
                    );

                    if cursor_next.visual_pos.x > origin.x {
                        let overlap = cursor_next.visual_pos.x - origin.x;
                        debug_assert!((1..=7).contains(&overlap));
                        line.push_str(&TAB_WHITESPACE[..overlap as usize]);
                        cursor_beg = cursor_next;
                    }
                }

                let mut global_off = cursor_beg.offset;
                let mut cursor_line = cursor_beg;

                while global_off < cursor_end.offset {
                    let chunk = self.read_forward(global_off);
                    let chunk = &chunk[..chunk.len().min(cursor_end.offset - global_off)];
                    let mut it = Utf8Chars::new(chunk, 0);

                    // TODO: Looping char-by-char is bad for performance.
                    // >25% of the total rendering time is spent here.
                    loop {
                        let chunk_off = it.offset();
                        let global_off = global_off + chunk_off;
                        let Some(ch) = it.next() else {
                            break;
                        };

                        if ch == ' ' || ch == '\t' {
                            let is_tab = ch == '\t';
                            let visualize = selection_off.contains(&global_off);
                            let mut whitespace = TAB_WHITESPACE;
                            let mut prefix_add = 0;

                            if is_tab || visualize {
                                // We need the character's visual position in order to either compute the tab size,
                                // or set the foreground color of the visualizer, respectively.
                                // TODO: Doing this char-by-char is of course also bad for performance.
                                cursor_line =
                                    self.cursor_move_to_offset_internal(cursor_line, global_off);
                            }

                            let tab_size =
                                if is_tab { self.tab_size_eval(cursor_line.column) } else { 1 };

                            if visualize {
                                // If the whitespace is part of the selection,
                                // we replace " " with "･" and "\t" with "￫".
                                (whitespace, prefix_add) = if is_tab {
                                    (VISUAL_TAB, VISUAL_TAB_PREFIX_ADD)
                                } else {
                                    (VISUAL_SPACE, VISUAL_SPACE_PREFIX_ADD)
                                };

                                // Make the visualized characters slightly gray.
                                let visualizer_rect = {
                                    let left = destination.left
                                        + self.margin_width
                                        + cursor_line.visual_pos.x
                                        - origin.x;
                                    let top = destination.top + cursor_line.visual_pos.y - origin.y;
                                    Rect { left, top, right: left + 1, bottom: top + 1 }
                                };
                                fb.blend_fg(
                                    visualizer_rect,
                                    fb.indexed_alpha(IndexedColor::Foreground, 1, 2),
                                );
                            }

                            line.push_str(&whitespace[..prefix_add + tab_size as usize]);
                        } else if ch <= '\x1f' || ('\u{7f}'..='\u{9f}').contains(&ch) {
                            // Append a Unicode representation of the C0 or C1 control character.
                            visualizer_buf[2] = if ch <= '\x1f' {
                                0x80 | ch as u8 // U+2400..=U+241F
                            } else if ch == '\x7f' {
                                0xA1 // U+2421
                            } else {
                                0xA6 // U+2426, because there are no pictures for C1 control characters.
                            };

                            // Our manually constructed UTF8 is never going to be invalid. Trust.
                            line.push_str(unsafe { str::from_utf8_unchecked(&visualizer_buf) });

                            // Highlight the control character yellow.
                            cursor_line =
                                self.cursor_move_to_offset_internal(cursor_line, global_off);
                            let visualizer_rect = {
                                let left =
                                    destination.left + self.margin_width + cursor_line.visual_pos.x
                                        - origin.x;
                                let top = destination.top + cursor_line.visual_pos.y - origin.y;
                                Rect { left, top, right: left + 1, bottom: top + 1 }
                            };
                            let bg = fb.indexed(IndexedColor::Yellow);
                            let fg = fb.contrasted(bg);
                            fb.blend_bg(visualizer_rect, bg);
                            fb.blend_fg(visualizer_rect, fg);
                        } else {
                            line.push(ch);
                        }
                    }

                    global_off += chunk.len();
                }

                visual_pos_x_max = visual_pos_x_max.max(cursor_end.visual_pos.x);
            }

            fb.replace_text(destination.top + y, destination.left, destination.right, &line);

            cursor = cursor_end;
        }

        // Colorize the margin that we wrote above.
        if self.margin_width > 0 {
            let margin = Rect {
                left: destination.left,
                top: destination.top,
                right: destination.left + self.margin_width,
                bottom: destination.bottom,
            };
            fb.blend_fg(margin, StraightRgba::from_le(0x7f7f7f7f));
        }

        if self.ruler > 0 {
            let left = destination.left + self.margin_width + (self.ruler - origin.x).max(0);
            let right = destination.right;
            if left < right {
                fb.blend_bg(
                    Rect { left, top: destination.top, right, bottom: destination.bottom },
                    fb.indexed_alpha(IndexedColor::BrightRed, 1, 4),
                );
            }
        }

        if focused {
            let mut x = self.cursor.visual_pos.x;
            let mut y = self.cursor.visual_pos.y;

            if self.word_wrap_column > 0 && x >= self.word_wrap_column {
                // The line the cursor is on wraps exactly on the word wrap column which
                // means the cursor is invisible. We need to move it to the next line.
                x = 0;
                y += 1;
            }

            // Move the cursor into screen space.
            x += destination.left - origin.x + self.margin_width;
            y += destination.top - origin.y;

            let cursor = Point { x, y };
            let text = Rect {
                left: destination.left + self.margin_width,
                top: destination.top,
                right: destination.right,
                bottom: destination.bottom,
            };

            if text.contains(cursor) {
                fb.set_cursor(cursor, self.overtype);

                if self.line_highlight_enabled && selection_beg >= selection_end {
                    fb.blend_bg(
                        Rect {
                            left: destination.left,
                            top: cursor.y,
                            right: destination.right,
                            bottom: cursor.y + 1,
                        },
                        StraightRgba::from_le(0x7f7f7f7f),
                    );
                }
            }
        }

        Some(RenderResult { visual_pos_x_max })
    }

    pub fn cut(&mut self, clipboard: &mut Clipboard) {
        self.cut_copy(clipboard, true);
    }

    pub fn copy(&mut self, clipboard: &mut Clipboard) {
        self.cut_copy(clipboard, false);
    }

    fn cut_copy(&mut self, clipboard: &mut Clipboard, cut: bool) {
        if !self.secondary_selections.is_empty() {
            let original_ranges = self.selection_ranges();
            if cut {
                self.edit_begin_grouping();
            }
            let mut ranges = self.selection_ranges();
            if ranges.iter().all(|&(a, b)| a == b) {
                for (a, b) in &mut ranges {
                    let cursor = self.cursor_move_to_offset_internal(self.cursor, *a);
                    *a = self.goto_line_start(cursor, cursor.logical_pos.y).offset;
                    *b = self.goto_line_start(cursor, cursor.logical_pos.y + 1).offset;
                }
                self.set_selection_ranges(&ranges);
                ranges = self.selection_ranges();
            }
            let fragments =
                ranges.iter().map(|&(a, b)| self.extract_bytes(a.min(b)..a.max(b))).collect();
            clipboard
                .write_fragments(fragments, if self.newlines_are_crlf { b"\r\n" } else { b"\n" });
            if cut {
                self.write_multiple(b"", true);
                self.edit_end_grouping();
            } else {
                self.set_selection_ranges(&original_ranges);
            }
            return;
        }
        let line_copy = !self.has_selection();
        let selection = self.extract_selection(cut);
        clipboard.write(selection);
        clipboard.write_was_line_copy(line_copy);
    }

    pub fn paste(&mut self, clipboard: &Clipboard) {
        let data = clipboard.read();
        if data.is_empty() {
            return;
        }

        if !self.secondary_selections.is_empty() {
            if let Some(fragments) = clipboard.fragments()
                && fragments.len() == self.selection_count()
            {
                self.write_multiple_fragments(fragments);
            } else {
                // External rectangular text naturally distributes one row per
                // cursor. Other text is repeated at every cursor.
                let rows: Vec<Vec<u8>> = data
                    .split(|&b| b == b'\n')
                    .map(|row| row.strip_suffix(b"\r").unwrap_or(row).to_vec())
                    .collect();
                if rows.len() == self.selection_count() {
                    self.write_multiple_fragments(&rows);
                } else {
                    self.write_multiple(data, true);
                }
            }
            return;
        }

        let pos = self.cursor_logical_pos();
        let at = if clipboard.is_line_copy() {
            self.goto_line_start(self.cursor, pos.y)
        } else {
            self.cursor
        };

        self.write(data, at, true);

        if clipboard.is_line_copy() {
            self.cursor_move_to_logical(Point { x: pos.x, y: pos.y + 1 });
        }
    }

    /// Paste only the first line into a path, search, or other single-line field.
    pub fn paste_single_line(&mut self, clipboard: &Clipboard) {
        let data = clipboard.read();
        let end = memchr2(b'\r', b'\n', data, 0);
        if end > 0 {
            self.write(&data[..end], self.cursor, true);
        }
    }

    /// Duplicate whole lines or an exact partial selection, selecting the copy.
    /// Keep the original bytes (including mixed line endings) and make the
    /// insertion one undo step without temporarily changing the wrap layout.
    pub fn duplicate_line_or_selection(&mut self) {
        let selected = self.selection_range();
        let line_mode = selected.is_none_or(|(beg, end)| {
            beg.logical_pos.x == 0
                && (end.logical_pos.x == 0
                    || end.offset
                        == self
                            .cursor_move_to_logical_internal(
                                end,
                                Point { x: CoordType::MAX, y: end.logical_pos.y },
                            )
                            .offset)
        });

        let (at, mut bytes, selection_start) = if line_mode {
            let first_line =
                selected.map_or(self.cursor.logical_pos.y, |(beg, _)| beg.logical_pos.y);
            let last_line = selected.map_or(first_line, |(_, end)| {
                end.logical_pos.y
                    - CoordType::from(end.logical_pos.x == 0 && end.logical_pos.y > first_line)
            });
            let beg = self.goto_line_start(self.cursor, first_line);
            let end = self
                .cursor_move_to_logical_internal(beg, Point { x: CoordType::MAX, y: last_line });
            let next = self.cursor_move_to_logical_internal(end, Point { x: 0, y: last_line + 1 });
            let mut bytes = Vec::new();
            // Prefer this line's terminator, preserving mixed-newline files.
            // At EOF there is no terminator; use the document's chosen style.
            if next.offset > end.offset {
                self.buffer.extract_raw(end.offset..next.offset, &mut bytes, 0);
            } else {
                bytes.extend_from_slice(if self.newlines_are_crlf { b"\r\n" } else { b"\n" });
            }
            let selection_start = end.offset + bytes.len();
            self.buffer.extract_raw(beg.offset..end.offset, &mut bytes, usize::MAX);
            (end.offset, bytes, selection_start)
        } else {
            let (beg, end) = selected.unwrap();
            (end.offset, self.extract_bytes(beg.offset..end.offset), end.offset)
        };
        let selection_end = at + bytes.len();
        if self.insert_final_newline && at == self.text_length() && !bytes.ends_with(b"\n") {
            bytes.extend_from_slice(if self.newlines_are_crlf { b"\r\n" } else { b"\n" });
        }
        if self.apply_byte_edits(&[(at..at, bytes)]).is_err() {
            self.edit_capacity_error = true;
            return;
        }
        self.set_selection_ranges(&[(selection_start, selection_end)]);
    }

    /// Inserts the user input `text` at the current cursor position.
    /// Replaces tabs with whitespace if needed, etc.
    pub fn write_canon(&mut self, text: &[u8]) {
        if !self.secondary_selections.is_empty() {
            self.write_multiple(text, false);
            return;
        }
        self.write(text, self.cursor, false);
    }

    /// Inserts `text` as-is at the current cursor position.
    /// The only transformation applied is that newlines are normalized.
    pub fn write_raw(&mut self, text: &[u8]) {
        if !self.secondary_selections.is_empty() {
            self.write_multiple(text, true);
            return;
        }
        self.write(text, self.cursor, true);
    }

    fn write(&mut self, text: &[u8], at: Cursor, raw: bool) {
        let start = self.selection_range().map_or(at, |(beg, _)| beg);
        let capacity = self.write_capacity_bound(text, raw, start);
        let deleted = self.selection_range().map_or(0, |(beg, end)| end.offset - beg.offset);
        if !self.buffer.reserve_insert(capacity.saturating_sub(deleted)) {
            self.edit_capacity_error = true;
            return;
        }
        let history_type = if raw { HistoryType::Other } else { HistoryType::Write };
        let mut edit_begun = false;

        // If we have an active selection, writing an empty `text`
        // will still delete the selection. As such, we check this first.
        if let Some((beg, end)) = self.selection_range_internal(false) {
            self.edit_begin(history_type, beg);
            self.edit_delete(end);
            self.set_selection(None);
            edit_begun = true;
        }

        // If the text is empty the remaining code won't do anything,
        // allowing us to exit early.
        if text.is_empty() {
            // ...we still need to end any active edit session though.
            if edit_begun {
                self.edit_end();
            }
            return;
        }

        if !edit_begun {
            self.edit_begin(history_type, at);
        }

        let mut offset = 0;
        let scratch = scratch_arena(None);
        let mut newline_buffer = ArenaString::new_in(&scratch);

        loop {
            // Can't use `unicode::newlines_forward` because bracketed paste uses CR instead of LF/CRLF.
            let offset_next = memchr2(b'\r', b'\n', text, offset);
            let line = &text[offset..offset_next];
            let column_before = self.cursor.logical_pos.x;

            // Write the contents of the line into the buffer.
            let mut line_off = 0;
            while line_off < line.len() {
                // Split the line into chunks of non-tabs and tabs.
                let mut plain = line;
                if !raw && !self.indent_with_tabs {
                    let end = memchr2(b'\t', b'\t', line, line_off);
                    plain = &line[line_off..end];
                }

                // Non-tabs are written as-is, because the outer loop already handles newline translation.
                self.edit_write(plain);
                line_off += plain.len();

                // Now replace tabs with spaces.
                while line_off < line.len() && line[line_off] == b'\t' {
                    let spaces = self.tab_size_eval(self.cursor.column);
                    let spaces = &TAB_WHITESPACE.as_bytes()[..spaces as usize];
                    self.edit_write(spaces);
                    line_off += 1;
                }
            }

            if !raw && self.overtype {
                let delete = self.cursor.logical_pos.x - column_before;
                let end = self.cursor_move_to_logical_internal(
                    self.cursor,
                    Point { x: self.cursor.logical_pos.x + delete, y: self.cursor.logical_pos.y },
                );
                self.edit_delete(end);
            }

            offset += line.len();
            if offset >= text.len() {
                break;
            }

            // First, write the newline.
            newline_buffer.clear();
            newline_buffer.push_str(if self.newlines_are_crlf { "\r\n" } else { "\n" });

            if !raw {
                // We'll give the next line the same indentation as the previous one.
                // This block figures out how much that is. We can't reuse that value,
                // because "  a\n  a\n" should give the 3rd line a total indentation of 4.
                // Assuming your terminal has bracketed paste, this won't be a concern though.
                // (If it doesn't, use a different terminal.)
                let line_beg = self.goto_line_start(self.cursor, self.cursor.logical_pos.y);
                let limit = self.cursor.offset;
                let mut off = line_beg.offset;
                let mut newline_indentation = 0;

                'outer: while off < limit {
                    let chunk = self.read_forward(off);
                    let chunk = &chunk[..chunk.len().min(limit - off)];

                    for &c in chunk {
                        if c == b' ' {
                            newline_indentation += 1;
                        } else if c == b'\t' {
                            newline_indentation += self.tab_size_eval(newline_indentation);
                        } else {
                            break 'outer;
                        }
                    }

                    off += chunk.len();
                }

                // If tabs are enabled, add as many tabs as we can.
                if self.indent_with_tabs {
                    let tab_count = newline_indentation / self.tab_size;
                    newline_buffer.push_repeat('\t', tab_count as usize);
                    newline_indentation -= tab_count * self.tab_size;
                }

                // If tabs are disabled, or if the indentation wasn't a multiple of the tab size,
                // add spaces to make up the difference.
                newline_buffer.push_repeat(' ', newline_indentation as usize);
            }

            self.edit_write(newline_buffer.as_bytes());

            // Skip one CR/LF/CRLF.
            if offset >= text.len() {
                break;
            }
            if text[offset] == b'\r' {
                offset += 1;
            }
            if offset >= text.len() {
                break;
            }
            if text[offset] == b'\n' {
                offset += 1;
            }
            if offset >= text.len() {
                break;
            }
        }

        // POSIX mandates that all valid lines end in a newline.
        // This isn't all that common on Windows and so we have
        // `self.final_newline` to control this.
        //
        // In order to not annoy people with this, we only add a
        // newline if you just edited the very end of the buffer.
        if self.insert_final_newline
            && self.cursor.offset > 0
            && self.cursor.offset == self.text_length()
            && self.cursor.logical_pos.x > 0
        {
            let cursor = self.cursor;
            self.edit_write(if self.newlines_are_crlf { b"\r\n" } else { b"\n" });
            // The visual line count is finalized by edit_end below.
            self.cursor = cursor;
        }

        self.edit_end();
    }

    /// Deletes 1 grapheme cluster from the buffer.
    /// `cursor_movements` is expected to be -1 for backspace and 1 for delete.
    /// If there's a current selection, it will be deleted and `cursor_movements` ignored.
    /// The selection is cleared after the call.
    /// Deletes characters from the buffer based on a delta from the cursor.
    pub fn delete(&mut self, granularity: CursorMovement, delta: CoordType) {
        if delta == 0 {
            return;
        }

        if !self.secondary_selections.is_empty() {
            self.edit_begin_grouping();
            let ranges: Vec<_> = self
                .selection_ranges()
                .into_iter()
                .map(|(a, b)| {
                    if a != b {
                        return (a, b);
                    }
                    let cursor = self.cursor_move_to_offset_internal(self.cursor, a);
                    let next = self.cursor_move_delta_internal(cursor, granularity, delta);
                    (a, next.offset)
                })
                .collect();
            self.set_selection_ranges(&ranges);
            self.write_multiple(b"", true);
            self.edit_end_grouping();
            return;
        }

        let mut beg;
        let mut end;

        if let Some(r) = self.selection_range_internal(false) {
            (beg, end) = r;
        } else {
            if (delta < 0 && self.cursor.offset == 0)
                || (delta > 0 && self.cursor.offset >= self.text_length())
            {
                // Nothing to delete.
                return;
            }

            beg = self.cursor;
            end = self.cursor_move_delta_internal(beg, granularity, delta);
            if beg.offset == end.offset {
                return;
            }
            if beg.offset > end.offset {
                mem::swap(&mut beg, &mut end);
            }
        }

        self.edit_begin(HistoryType::Delete, beg);
        self.edit_delete(end);
        self.edit_end();

        self.set_selection(None);
    }

    /// Returns the logical position of the first character on this line.
    /// Return `.x == 0` if there are no non-whitespace characters.
    pub fn indent_end_logical_pos(&self) -> Point {
        let cursor = self.goto_line_start(self.cursor, self.cursor.logical_pos.y);
        let (chars, _) = self.measure_indent_internal(cursor.offset, CoordType::MAX);
        Point { x: chars, y: cursor.logical_pos.y }
    }

    /// Indents/unindents the current selection or line.
    pub fn indent_change(&mut self, direction: CoordType) {
        if !self.secondary_selections.is_empty() {
            if direction >= 0 && self.selection_ranges().iter().all(|&(a, b)| a == b) {
                self.write_multiple(b"\t", false);
                return;
            }
            let ranges = self.selection_ranges();
            let mut lines = std::collections::BTreeSet::new();
            for &(a, b) in &ranges {
                let beg = self.cursor_move_to_offset_internal(self.cursor, a.min(b));
                let end = self.cursor_move_to_offset_internal(beg, a.max(b));
                let last = end.logical_pos.y
                    - if end.logical_pos.x == 0 && end.offset > beg.offset { 1 } else { 0 };
                lines.extend(beg.logical_pos.y..=last);
            }
            let mut edits = Vec::new();
            for y in lines {
                let start = self.goto_line_start(self.cursor, y);
                let (chars, columns) = self.measure_indent_internal(start.offset, CoordType::MAX);
                if direction < 0 {
                    if columns == 0 {
                        continue;
                    }
                    let (prev_chars, _) = self
                        .measure_indent_internal(start.offset, self.tab_size_prev_column(columns));
                    let beg = self
                        .cursor_move_to_logical_internal(start, Point { x: prev_chars, y })
                        .offset;
                    let end =
                        self.cursor_move_to_logical_internal(start, Point { x: chars, y }).offset;
                    edits.push((beg..end, Vec::new()));
                } else {
                    let end =
                        self.cursor_move_to_logical_internal(start, Point { x: chars, y }).offset;
                    let text = if self.indent_with_tabs {
                        vec![b'\t']
                    } else {
                        vec![b' '; self.tab_size_eval(columns) as usize]
                    };
                    edits.push((end..end, text));
                }
            }
            if edits.is_empty() {
                return;
            }
            let map = |offset: usize| {
                let mut mapped = offset;
                for (range, text) in edits.iter().rev() {
                    if offset >= range.end {
                        mapped = mapped - range.len() + text.len();
                    } else if offset > range.start {
                        mapped = range.start + text.len();
                    }
                }
                mapped
            };
            let moved: Vec<_> = ranges.iter().map(|&(a, b)| (map(a), map(b))).collect();
            if self.apply_byte_edits(&edits).is_ok() {
                self.set_selection_ranges(&moved);
            } else {
                self.edit_capacity_error = true;
            }
            return;
        }
        let selection = self.selection;
        let mut selection_beg = self.cursor.logical_pos;
        let mut selection_end = selection_beg;

        if let Some(TextBufferSelection { beg, end }) = &selection {
            selection_beg = *beg;
            selection_end = *end;
        }

        if direction >= 0 && self.selection.is_none_or(|sel| sel.beg.y == sel.end.y) {
            self.write_canon(b"\t");
            return;
        }

        self.edit_begin_grouping();

        let [first, last] = minmax(selection_beg, selection_end);
        // A selection ending at column zero excludes that final line.
        let last_y = if last.x == 0 && last.y > first.y { last.y - 1 } else { last.y };
        for y in first.y..=last_y {
            self.cursor_move_to_logical(Point { x: 0, y });

            let line_start_offset = self.cursor.offset;
            let (curr_chars, curr_columns) =
                self.measure_indent_internal(line_start_offset, CoordType::MAX);

            self.cursor_move_to_logical(Point { x: curr_chars, y: self.cursor.logical_pos.y });

            let delta;

            if direction < 0 {
                // Unindent the line. If there's no indentation, skip.
                if curr_columns <= 0 {
                    continue;
                }

                let (prev_chars, _) = self.measure_indent_internal(
                    line_start_offset,
                    self.tab_size_prev_column(curr_columns),
                );

                delta = prev_chars - curr_chars;
                self.delete(CursorMovement::Grapheme, delta);
            } else {
                // Indent the line. `self.cursor` is already at the level of indentation.
                delta = self.tab_size_eval(curr_columns);
                self.write_canon(b"\t");
            }

            // As the lines get unindented, the selection should shift with them.
            if y == selection_beg.y {
                selection_beg.x += delta;
            }
            if y == selection_end.y {
                selection_end.x += delta;
            }
        }
        self.edit_end_grouping();

        // Move the cursor to the new end of the selection.
        self.set_cursor_internal(self.cursor_move_to_logical_internal(self.cursor, selection_end));

        // NOTE: If the selection was previously `None`,
        // it should continue to be `None` after this.
        self.set_selection(
            selection.map(|_| TextBufferSelection { beg: selection_beg, end: selection_end }),
        );
    }

    fn measure_indent_internal(
        &self,
        mut offset: usize,
        max_columns: CoordType,
    ) -> (CoordType, CoordType) {
        let mut chars = 0;
        let mut columns = 0;

        'outer: while columns < max_columns {
            let chunk = self.read_forward(offset);
            if chunk.is_empty() {
                break;
            }

            for &c in chunk {
                let next = match c {
                    b' ' => columns + 1,
                    b'\t' => columns + self.tab_size_eval(columns),
                    _ => break 'outer,
                };
                if next > max_columns {
                    break 'outer;
                }
                chars += 1;
                columns = next;
            }

            offset += chunk.len();
        }

        (chars, columns)
    }

    /// Backspace to the previous tab stop while inside leading indentation.
    /// Ctrl+Backspace removes indentation back to the start of the line.
    pub fn backspace_with_auto_unindent(&mut self, granularity: CursorMovement) {
        if !self.secondary_selections.is_empty() {
            let ranges: Vec<_> = self
                .selection_ranges()
                .into_iter()
                .map(|(a, b)| {
                    if a != b {
                        return (a, b);
                    }
                    let cursor = self.cursor_move_to_offset_internal(self.cursor, a);
                    (a, self.backspace_target(cursor, granularity).offset)
                })
                .collect();
            self.edit_begin_grouping();
            self.set_selection_ranges(&ranges);
            self.write_multiple(b"", true);
            self.edit_end_grouping();
            return;
        }
        if self.has_selection() {
            self.delete(granularity, -1);
            return;
        }
        let to = self.cursor;
        let from = self.backspace_target(to, granularity);
        if from.offset != to.offset {
            self.edit_begin(HistoryType::Delete, from);
            self.edit_delete(to);
            self.edit_end();
        }
    }

    fn backspace_target(&self, cursor: Cursor, granularity: CursorMovement) -> Cursor {
        if cursor.logical_pos.x > 0 {
            let start = self.goto_line_start(cursor, cursor.logical_pos.y);
            let previous_column = if granularity == CursorMovement::Grapheme {
                self.tab_size_prev_column(cursor.column)
            } else {
                0
            };
            let (from_pos, from_col) = self.measure_indent_internal(start.offset, previous_column);
            let (delta, _) = self.measure_indent_internal(
                start.offset + from_pos as usize,
                cursor.column - from_col,
            );
            if delta + from_pos >= cursor.logical_pos.x {
                return self.cursor_move_to_logical_internal(
                    start,
                    Point { x: from_pos, y: start.logical_pos.y },
                );
            }
        }
        self.cursor_move_delta_internal(cursor, granularity, -1)
    }

    /// Displaces the current, cursor or the selection, line(s) in the given direction.
    pub fn move_selected_lines(&mut self, direction: MoveLineDirection) {
        self.clear_secondary_cursors();
        let selection = self.selection;
        let cursor = self.cursor;

        // If there's no selection, we move the line the cursor is on instead.
        let [beg, end] = match self.selection {
            Some(s) => minmax(s.beg.y, s.end.y),
            None => [cursor.logical_pos.y, cursor.logical_pos.y],
        };

        // Check if this would be a no-op.
        if match direction {
            MoveLineDirection::Up => beg <= 0,
            MoveLineDirection::Down => end >= self.stats.logical_lines - 1,
        } {
            return;
        }

        let delta = match direction {
            MoveLineDirection::Up => -1,
            MoveLineDirection::Down => 1,
        };
        let (cut, paste) = match direction {
            MoveLineDirection::Up => (beg - 1, end),
            MoveLineDirection::Down => (end + 1, beg),
        };

        self.edit_begin_grouping();
        {
            // Let's say this is `MoveLineDirection::Up`.
            // In that case, we'll cut (remove) the line above the selection here...
            self.cursor_move_to_logical(Point { x: 0, y: cut });
            let line = self.extract_selection(true);

            // ...and paste it below the selection. This will then
            // appear to the user as if the selection was moved up.
            self.cursor_move_to_logical(Point { x: 0, y: paste });
            self.edit_begin(HistoryType::Write, self.cursor);
            // The `extract_selection` call can return an empty `Vec`),
            // if the `cut` line was at the end of the file. Since we want to
            // paste the line somewhere it needs a trailing newline at the minimum.
            //
            // Similarly, if the `paste` line is at the end of the file
            // and there's no trailing newline, we'll have failed to reach
            // that end in which case `logical_pos.y != past`.
            if line.is_empty() || self.cursor.logical_pos.y != paste {
                self.write_canon(b"\n");
            }
            if !line.is_empty() {
                self.write_raw(&line);
            }
            self.edit_end();
        }
        self.edit_end_grouping();

        // Shift the cursor and selection together with the moved lines.
        self.cursor_move_to_logical(Point {
            x: cursor.logical_pos.x,
            y: cursor.logical_pos.y + delta,
        });
        self.set_selection(selection.map(|mut s| {
            s.beg.y += delta;
            s.end.y += delta;
            s
        }));
    }

    /// Extracts the contents of the current selection.
    /// May optionally delete it, if requested. This is meant to be used for Ctrl+X.
    fn extract_selection(&mut self, delete: bool) -> Vec<u8> {
        let line_copy = !self.has_selection();
        let Some((beg, end)) = self.selection_range_internal(true) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        self.buffer.extract_raw(beg.offset..end.offset, &mut out, 0);

        if delete && !out.is_empty() {
            self.edit_begin(HistoryType::Delete, beg);
            self.edit_delete(end);
            self.edit_end();
            self.set_selection(None);
        }

        // Line copies (= Ctrl+C when there's no selection) always end with a newline.
        if line_copy && !out.ends_with(b"\n") {
            out.replace_range(out.len().., if self.newlines_are_crlf { b"\r\n" } else { b"\n" });
        }

        out
    }

    /// Extracts the contents of the current selection the user made.
    /// This differs from [`TextBuffer::extract_selection()`] in that
    /// it does nothing if the selection was made by searching.
    pub fn extract_user_selection(&mut self, delete: bool) -> Option<Vec<u8>> {
        if !self.has_selection() {
            return None;
        }

        if let Some(search) = &self.search {
            let search = unsafe { &*search.get() };
            if search.selection_generation == self.selection_generation {
                return None;
            }
        }

        Some(self.extract_selection(delete))
    }

    /// Returns the current selection anchors, or `None` if there
    /// is no selection. The returned logical positions are sorted.
    pub fn selection_range(&self) -> Option<(Cursor, Cursor)> {
        self.selection_range_internal(false)
    }

    /// Returns the current selection anchors.
    ///
    /// If there's no selection and `line_fallback` is `true`,
    /// the start/end of the current line are returned.
    /// This is meant to be used for Ctrl+C / Ctrl+X.
    fn selection_range_internal(&self, line_fallback: bool) -> Option<(Cursor, Cursor)> {
        let [beg, end] = match self.selection {
            None if !line_fallback => return None,
            None => [
                Point { x: 0, y: self.cursor.logical_pos.y },
                Point { x: 0, y: self.cursor.logical_pos.y + 1 },
            ],
            Some(TextBufferSelection { beg, end }) => minmax(beg, end),
        };

        let beg = self.cursor_move_to_logical_internal(self.cursor, beg);
        let end = self.cursor_move_to_logical_internal(beg, end);

        if beg.offset < end.offset { Some((beg, end)) } else { None }
    }

    fn edit_begin_grouping(&mut self) {
        self.edit_group_depth += 1;
        if self.edit_group_depth > 1 {
            return;
        }
        self.last_history_type = HistoryType::Other;
        self.active_edit_group = Some(ActiveEditGroupInfo {
            redo_before: mem::take(&mut self.redo_stack),
            cursor_before: self.cursor.logical_pos,
            selection_before: self.selection,
            secondary_before: self.secondary_selections.clone(),
            newlines_before: self.newlines_are_crlf,
            stats_before: self.stats,
            layout_before: self.layout_configuration(),
            generation_before: self.buffer.generation(),
        });
    }

    fn edit_end_grouping(&mut self) {
        self.edit_group_depth = self.edit_group_depth.saturating_sub(1);
        if self.edit_group_depth != 0 {
            return;
        }
        if let Some(info) = self.active_edit_group.take()
            && self.buffer.generation() == info.generation_before
        {
            self.redo_stack = info.redo_before;
        }
        self.last_history_type = HistoryType::Other;
        self.trim_history();
    }

    // Never split an undo transaction. A single large macro may exceed the
    // entry limit; retain it intact and discard it as a whole when superseded.
    fn trim_history(&mut self) {
        if self.active_edit_group.is_some() {
            return;
        }
        while self.undo_stack.len() > 1000 {
            let oldest = self.undo_stack.front().unwrap().borrow().generation_before;
            let newest = self.undo_stack.back().unwrap().borrow().generation_before;
            if oldest == newest {
                break;
            }
            while self
                .undo_stack
                .front()
                .is_some_and(|entry| entry.borrow().generation_before == oldest)
            {
                self.undo_stack.pop_front();
            }
        }
    }

    /// Starts a new edit operation.
    /// This is used for tracking the undo/redo history.
    fn edit_begin(&mut self, history_type: HistoryType, cursor: Cursor) {
        self.active_edit_depth += 1;
        if self.active_edit_depth > 1 {
            return;
        }

        let cursor_before = self.cursor;
        self.set_cursor_internal(cursor);
        self.redo_stack.clear();
        self.active_edit_needs_reflow = false;

        // Adjacent edits can share their byte replacement. Nonadjacent typed
        // edits (for example before an automatic final newline) need separate
        // replacements but still belong to the same user-visible undo group.
        let (coalesces, previous_group) = if history_type != HistoryType::Other
            && history_type == self.last_history_type
            && let Some(entry) = self.undo_stack.back()
        {
            let entry = entry.borrow();
            let coalesces = match history_type {
                HistoryType::Other => false,
                HistoryType::Write => cursor.offset == entry.offset + entry.added.len(),
                HistoryType::Delete => entry.added.is_empty() && cursor.offset == entry.offset,
            };
            (
                coalesces,
                (!coalesces).then(|| ActiveEditGroupInfo {
                    redo_before: VecDeque::new(),
                    cursor_before: entry.cursor_before,
                    selection_before: entry.selection_before,
                    secondary_before: entry.secondary_before.clone(),
                    newlines_before: entry.newlines_before,
                    stats_before: entry.stats_before,
                    layout_before: entry.layout_before,
                    generation_before: entry.generation_before,
                }),
            )
        } else {
            (false, None)
        };
        if !coalesces {
            self.last_history_type = history_type;
            self.undo_stack.push_back(SemiRefCell::new(HistoryEntry {
                cursor_before: cursor_before.logical_pos,
                selection_before: self.selection,
                secondary_before: self.secondary_selections.clone(),
                newlines_before: self.newlines_are_crlf,
                stats_before: self.stats,
                layout_before: self.layout_configuration(),
                generation_before: self.buffer.generation(),
                offset: cursor.offset,
                deleted: Vec::new(),
                added: Vec::new(),
            }));

            if let Some(info) = self.active_edit_group.as_ref().or(previous_group.as_ref())
                && let Some(entry) = self.undo_stack.back()
            {
                let mut entry = entry.borrow_mut();
                entry.cursor_before = info.cursor_before;
                entry.selection_before = info.selection_before;
                entry.secondary_before.clone_from(&info.secondary_before);
                entry.newlines_before = info.newlines_before;
                entry.stats_before = info.stats_before;
                entry.layout_before = info.layout_before;
                entry.generation_before = info.generation_before;
            }
        }

        self.active_edit_off = cursor.offset;

        // If word-wrap is enabled, the visual layout of all logical lines affected by the write
        // may have changed. This includes even text before the insertion point up to the line
        // start, because this write may have joined with a word before the initial cursor.
        // See other uses of `word_wrap_cursor_next_line` in this function.
        if self.word_wrap_column > 0 {
            let safe_start =
                self.measurement_config().goto_logical(Point { x: 0, y: cursor.logical_pos.y });
            let next_line = self
                .measurement_config()
                .with_cursor(safe_start)
                .goto_logical(Point { x: 0, y: cursor.logical_pos.y + 1 });
            self.active_edit_line_info = Some(ActiveEditLineInfo {
                safe_start,
                line_height_in_rows: next_line.visual_pos.y - safe_start.visual_pos.y,
                distance_next_line_start: next_line.offset - cursor.offset,
            });
        }
    }

    /// Writes `text` into the buffer at the current cursor position.
    /// It records the change in the undo stack.
    fn edit_write(&mut self, text: &[u8]) {
        if text.is_empty() {
            return;
        }
        if !self.buffer.reserve_insert(text.len()) {
            self.edit_capacity_error = true;
            return;
        }
        let anchor = self.edit_anchor(self.active_edit_off, text);

        // Copy the written portion into the undo entry.
        {
            let mut undo = self.undo_stack.back_mut().unwrap().borrow_mut();
            undo.added.extend_from_slice(text);
        }

        // Write!
        self.buffer.replace(self.active_edit_off..self.active_edit_off, text);

        // Move self.cursor to the end of the newly written text. Can't use `self.set_cursor_internal`,
        // because we're still in the progress of recalculating the line stats.
        self.active_edit_off += text.len();
        self.cursor = self.cursor_move_to_offset_internal(anchor, self.active_edit_off);
        self.stats.logical_lines += text.iter().filter(|&&b| b == b'\n').count() as CoordType;
    }

    /// Deletes the text between the current cursor position and `to`.
    /// It records the change in the undo stack.
    fn edit_delete(&mut self, to: Cursor) {
        debug_assert!(to.offset >= self.active_edit_off);
        if to.offset == self.active_edit_off {
            return;
        }
        let anchor = self.edit_anchor(to.offset, b"");
        let off = self.active_edit_off;
        let removed_lines = {
            let mut undo = self.undo_stack.back_mut().unwrap().borrow_mut();
            let start = undo.deleted.len();
            self.buffer.extract_raw(off..to.offset, &mut undo.deleted, usize::MAX);
            undo.deleted[start..].iter().filter(|&&b| b == b'\n').count() as CoordType
        };

        // Delete the portion from the buffer by enlarging the gap.
        let count = to.offset - off;
        self.buffer.allocate_gap(off, 0, count);

        self.stats.logical_lines -= removed_lines;
        self.cursor = self.cursor_move_to_offset_internal(anchor, self.active_edit_off);
    }

    /// Re-measure from before an edit when its neighboring codepoints can join
    /// into a grapheme. The gap and undo history may sit inside that grapheme.
    fn edit_anchor(&mut self, end: usize, text: &[u8]) -> Cursor {
        let left = self.read_backward(self.active_edit_off);
        let right = self.read_forward(end);
        let joins = if text.is_empty() {
            crate::unicode::graphemes_may_join(left, right)
        } else {
            crate::unicode::graphemes_may_join(left, text)
                || crate::unicode::graphemes_may_join(text, right)
        };
        if joins || self.active_edit_off != self.cursor.offset {
            self.active_edit_needs_reflow = true;
            // The rounded cursor can already be on the next line when CRLF
            // formed at the exact insertion offset. Start from the prior line.
            self.measurement_config()
                .goto_logical(Point { x: 0, y: (self.cursor.logical_pos.y - 1).max(0) })
        } else {
            self.cursor
        }
    }

    /// Finalizes the current edit operation
    /// and recalculates the line statistics.
    fn edit_end(&mut self) {
        self.active_edit_depth -= 1;
        debug_assert!(self.active_edit_depth >= 0);
        if self.active_edit_depth > 0 {
            return;
        }

        if self.active_edit_needs_reflow {
            self.active_edit_line_info = None;
            self.reflow();
            self.recalc_after_content_changed();
            self.trim_history();
            return;
        }

        #[cfg(debug_assertions)]
        {
            let entry = self.undo_stack.back_mut().unwrap().borrow_mut();
            debug_assert!(
                !entry.deleted.is_empty()
                    || !entry.added.is_empty()
                    || entry.newlines_before != self.newlines_are_crlf
            );
        }

        if let Some(info) = self.active_edit_line_info.take() {
            let deleted_count = self.undo_stack.back_mut().unwrap().borrow_mut().deleted.len();
            let target = self.cursor.logical_pos;

            // After the edit, info.safe_start has stale visual_pos values because the buffer
            // content has changed. We need to recalculate the safe_start position for the
            // NEW buffer content by navigating from scratch. Start at the
            // original edited line, not the final cursor line: an insertion may
            // have introduced several new logical lines before the cursor.
            let safe_start = self.measurement_config().goto_logical(info.safe_start.logical_pos);

            // From our safe position we can measure the actual visual position of the cursor.
            let new_cursor = self.cursor_move_to_logical_internal(safe_start, target);

            // If content is added at the insertion position, that's not a problem:
            // We can just remeasure the height of this one line and calculate the delta.
            // `deleted_count` is 0 in this case.
            //
            // The problem is when content is deleted, because it may affect lines
            // beyond the end of the `next_line`. In that case we have to measure
            // the entire buffer contents until the end to compute `self.stats.visual_lines`.
            //
            // NOTE: We must update stats.visual_lines BEFORE calling set_cursor_internal,
            // because set_cursor_internal has a debug assertion that checks
            // cursor.visual_pos.y <= self.stats.visual_lines.
            if deleted_count < info.distance_next_line_start {
                // Now we can measure how many more visual rows this logical line spans.
                let next_line = self
                    .measurement_config()
                    .with_cursor(safe_start)
                    .goto_logical(Point { x: 0, y: target.y + 1 });
                let lines_before = info.line_height_in_rows;
                let lines_after = next_line.visual_pos.y - safe_start.visual_pos.y;
                self.stats.visual_lines += lines_after - lines_before;
            } else {
                let end = self.cursor_move_to_logical_internal(new_cursor, Point::MAX);
                self.stats.visual_lines = end.visual_pos.y + 1;
            }

            // Now set the cursor - stats.visual_lines has been updated so the assertion will pass.
            self.set_cursor_internal(new_cursor);
        } else {
            // If word-wrap is disabled the visual line count always matches the logical one.
            self.stats.visual_lines = self.stats.logical_lines;
        }

        self.recalc_after_content_changed();
        self.trim_history();
    }

    /// Undo the last edit operation.
    pub fn undo(&mut self) {
        self.undo_redo(true);
    }

    /// Redo the last undo operation.
    pub fn redo(&mut self) {
        self.undo_redo(false);
    }

    fn layout_configuration(&self) -> (CoordType, CoordType, bool, bool) {
        (self.width, self.tab_size, self.word_wrap_enabled, self.margin_enabled)
    }

    fn undo_redo(&mut self, undo: bool) {
        assert_eq!(self.active_edit_depth, 0, "cannot undo/redo during an edit session");
        self.last_history_type = HistoryType::Other;
        let layout = self.layout_configuration();
        let mut remeasure_layout = false;
        let buffer_generation = self.buffer.generation();
        let mut entry_buffer_generation = None;

        loop {
            // Transfer the last entry from the undo stack to the redo stack or vice versa.
            {
                let (from, to) = if undo {
                    (&mut self.undo_stack, &mut self.redo_stack)
                } else {
                    (&mut self.redo_stack, &mut self.undo_stack)
                };

                if let Some(g) = entry_buffer_generation
                    && from.back().is_none_or(|c| c.borrow().generation_before != g)
                {
                    break;
                }

                let Some(entry) = from.pop_back() else {
                    break;
                };

                to.push_back(entry);
            }

            let change = {
                let to = if undo { &self.redo_stack } else { &self.undo_stack };
                to.back().unwrap()
            };

            // Remember the buffer generation of the change so we can stop popping undos/redos.
            // Also, move to the point where the modification took place.
            let offset = {
                let change = change.borrow();
                entry_buffer_generation = Some(change.generation_before);
                change.offset
            };

            {
                let mut change = change.borrow_mut();
                let change = &mut *change;

                // Undo: Whatever was deleted is now added and vice versa.
                mem::swap(&mut change.deleted, &mut change.added);

                // Delete the inserted portion.
                self.buffer.allocate_gap(offset, 0, change.deleted.len());

                // Undo restores the exact bytes, including mixed line endings.
                self.buffer.replace(offset..offset, &change.added);

                // Restore the previous line statistics.
                mem::swap(&mut self.stats, &mut change.stats_before);
                remeasure_layout |= change.layout_before != layout;
                change.layout_before = layout;

                // Restore the previous selection.
                mem::swap(&mut self.selection, &mut change.selection_before);
                mem::swap(&mut self.secondary_selections, &mut change.secondary_before);
                mem::swap(&mut self.newlines_are_crlf, &mut change.newlines_before);

                // Pretend as if the buffer was never modified.
                self.buffer.set_generation(change.generation_before);
                change.generation_before = buffer_generation;

                // Restore the previous cursor.
                let cursor_before = self.measurement_config().goto_logical(change.cursor_before);
                change.cursor_before = self.cursor.logical_pos;
                // Can't use `set_cursor_internal` here, because we haven't updated the line stats yet.
                self.cursor = cursor_before;

                if self.undo_stack.is_empty() {
                    self.last_history_type = HistoryType::Other;
                }
            }
        }

        if entry_buffer_generation.is_some() {
            self.rectangle = None;
            self.last_history_type = HistoryType::Other;
            self.recalc_after_content_changed();
            // Historical visual counts belong to the viewport/tab settings at
            // the time of the edit. Logical positions survive a resize; visual
            // positions must be measured under the current settings.
            if remeasure_layout {
                self.reflow();
            }
        }
    }

    /// For interfacing with ICU.
    pub(crate) fn read_backward(&self, off: usize) -> &[u8] {
        self.buffer.read_backward(off)
    }

    /// For interfacing with ICU.
    pub fn read_forward(&self, off: usize) -> &[u8] {
        self.buffer.read_forward(off)
    }

    /// Extract bytes from the buffer in the given range.
    /// This is useful for logging selection content without modifying the buffer.
    pub fn extract_bytes(&self, range: std::ops::Range<usize>) -> Vec<u8> {
        let mut out = Vec::new();
        self.buffer.extract_raw(range, &mut out, 0);
        out
    }

    /// Extract the content of a specific line (by logical line number, 0-indexed).
    /// Returns the line content without the trailing newline, and the byte offset of the line start.
    /// Returns None if the line number is out of bounds.
    pub fn extract_line_content(&self, line_number: CoordType) -> Option<(Vec<u8>, usize)> {
        if line_number < 0 || line_number >= self.stats.logical_lines {
            return None;
        }

        let beg = self.goto_line_start(self.cursor, line_number);
        let end =
            self.cursor_move_to_logical_internal(beg, Point { x: CoordType::MAX, y: line_number });
        Some((self.extract_bytes(beg.offset..end.offset), beg.offset))
    }

    /// Find literal occurrences, including matches spanning the gap.
    pub fn find_line_matches(&self, pattern: &[u8]) -> Vec<(CoordType, usize)> {
        if pattern.is_empty() {
            return Vec::new();
        }
        let mut offset = 0;
        let mut line = 0;
        find_all_matches(&self.buffer, pattern, usize::MAX)
            .into_iter()
            .map(|range| {
                // Matches are ordered. Count intervening newlines only once,
                // instead of rescanning the document prefix for every match.
                while offset < range.start {
                    let chunk = self.read_forward(offset);
                    let count = chunk.len().min(range.start - offset);
                    line +=
                        chunk[..count].iter().filter(|&&byte| byte == b'\n').count() as CoordType;
                    offset += count;
                }
                (line, range.start)
            })
            .collect()
    }

    /// Find the nearest complete line for reload cursor restoration. A matching
    /// substring in a longer line is not the same source line. Scan once without
    /// allocating a match list, even in files containing millions of duplicates.
    pub fn nearest_matching_line(&self, pattern: &[u8], preferred: CoordType) -> Option<CoordType> {
        if pattern.is_empty() || pattern.contains(&b'\n') {
            return None;
        }
        let mut nearest = None;
        let mut distance = usize::MAX;
        let mut line: CoordType = 0;
        let mut length = 0;
        let mut last = 0;
        let mut equal = true;
        let mut offset = 0;
        while offset < self.text_length() {
            let chunk = self.read_forward(offset);
            for &byte in chunk {
                if byte == b'\n' {
                    let content_length = length - usize::from(last == b'\r');
                    let candidate_distance = line.abs_diff(preferred);
                    if equal && content_length == pattern.len() && candidate_distance < distance {
                        nearest = Some(line);
                        distance = candidate_distance;
                    }
                    line += 1;
                    length = 0;
                    last = 0;
                    equal = true;
                } else {
                    equal &= pattern
                        .get(length)
                        .map_or(length == pattern.len() && byte == b'\r', |&expected| {
                            byte == expected
                        });
                    length += 1;
                    last = byte;
                }
            }
            offset += chunk.len();
        }
        // An EOF CR is content, not a CRLF terminator.
        if equal && length == pattern.len() && line.abs_diff(preferred) < distance {
            nearest = Some(line);
        }
        nearest
    }

    /// Convert a byte offset to a logical line number.
    #[cfg(test)]
    fn offset_to_line(&self, target_offset: usize) -> CoordType {
        let mut offset = 0;
        let mut line = 0;

        while offset < target_offset {
            let chunk = self.read_forward(offset);
            if chunk.is_empty() {
                break;
            }

            // Count newlines in this chunk up to the target offset
            let chunk_end = (target_offset - offset).min(chunk.len());
            for (i, &b) in chunk[..chunk_end].iter().enumerate() {
                if b == b'\n' {
                    line += 1;
                }
                if offset + i + 1 >= target_offset {
                    break;
                }
            }

            offset += chunk.len();
        }

        line
    }
}

pub enum Bom {
    None,
    UTF8,
    UTF16LE,
    UTF16BE,
    UTF32LE,
    UTF32BE,
    GB18030,
}

const BOM_MAX_LEN: usize = 4;

fn detect_bom(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 4 {
        if bytes.starts_with(b"\xFF\xFE\x00\x00") {
            return Some("UTF-32LE");
        }
        if bytes.starts_with(b"\x00\x00\xFE\xFF") {
            return Some("UTF-32BE");
        }
        if bytes.starts_with(b"\x84\x31\x95\x33") {
            return Some("GB18030");
        }
    }
    if bytes.len() >= 3 && bytes.starts_with(b"\xEF\xBB\xBF") {
        return Some("UTF-8");
    }
    if bytes.len() >= 2 {
        if bytes.starts_with(b"\xFF\xFE") {
            return Some("UTF-16LE");
        }
        if bytes.starts_with(b"\xFE\xFF") {
            return Some("UTF-16BE");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multi_buffer(text: &str) -> TextBuffer {
        let mut buffer = TextBuffer::new(true).unwrap();
        buffer.set_crlf(false);
        buffer.write_raw(text.as_bytes());
        buffer.cursor_move_to_offset(0);
        buffer
    }

    fn contents(buffer: &TextBuffer) -> Vec<u8> {
        buffer.extract_bytes(0..buffer.text_length())
    }

    #[test]
    #[ignore = "requires dynamically loaded ICU"]
    fn stale_search_miss_is_invalidated_after_edit_and_undo_branch() {
        icu::init().unwrap();
        let mut b = multi_buffer("nothing");
        b.find_and_select("word", SearchOptions::default()).unwrap();
        assert!(!b.has_selection());
        b.cursor_move_to_offset(b.text_length());
        b.write_raw(b" word word");
        b.find_and_select("word", SearchOptions::default()).unwrap();
        let first = b.selection_range().unwrap().0.offset;
        b.find_and_select("word", SearchOptions::default()).unwrap();
        assert_ne!(b.selection_range().unwrap().0.offset, first);
        b.undo();
        b.find_and_select("word", SearchOptions::default()).unwrap();
        assert!(!b.has_selection());
        b.cursor_move_to_offset(b.text_length());
        b.write_raw(b" word");
        b.find_and_select("word", SearchOptions::default()).unwrap();
        assert!(b.has_selection());
    }

    #[test]
    fn multi_reversed_overlapping_ranges_replace_once_and_undo() {
        let mut b = multi_buffer("alpha beta gamma");
        b.set_selection_ranges(&[(5, 0), (2, 7), (11, 16)]);
        let selections = b.selection_ranges();
        assert_eq!(b.selection_count(), 2);
        b.write_raw(b"X");
        assert_eq!(contents(&b), b"Xeta X");
        assert_eq!(b.selection_count(), 2);
        b.undo();
        assert_eq!(contents(&b), b"alpha beta gamma");
        assert_eq!(b.selection_ranges(), selections);
        b.redo();
        assert_eq!(contents(&b), b"Xeta X");
        assert_eq!(b.selection_count(), 2);
    }

    #[test]
    fn multi_unicode_grapheme_delete_is_atomic() {
        let mut b = multi_buffer("e\u{301}🙂\nx🙂");
        let len = b.text_length();
        b.set_selection_ranges(&[(7, 7), (len, len)]);
        let selections = b.selection_ranges();
        b.delete(CursorMovement::Grapheme, -1);
        assert_eq!(contents(&b), "e\u{301}\nx".as_bytes());
        b.undo();
        assert_eq!(contents(&b), "e\u{301}🙂\nx🙂".as_bytes());
        assert_eq!(b.selection_ranges(), selections);
    }

    #[test]
    fn multi_crlf_delete_and_redo_preserve_exact_bytes() {
        let mut b = multi_buffer("a\nb\nc");
        b.normalize_newlines(true);
        b.set_selection_ranges(&[(1, 1), (4, 4)]);
        b.delete(CursorMovement::Grapheme, 1);
        assert_eq!(contents(&b), b"abc");
        b.undo();
        assert_eq!(contents(&b), b"a\r\nb\r\nc");
        b.redo();
        assert_eq!(contents(&b), b"abc");
    }

    #[test]
    fn multi_capacity_failure_preserves_contents_selection_and_history() {
        let mut b = multi_buffer("abc def");
        b.set_selection_ranges(&[(0, 3), (4, 7)]);
        let selections = b.selection_ranges();
        b.write_raw(&vec![b'x'; 70 * KIBI]);
        assert!(b.take_edit_capacity_error());
        assert_eq!(contents(&b), b"abc def");
        assert_eq!(b.selection_ranges(), selections);
        b.undo();
        assert!(contents(&b).is_empty());
    }

    #[test]
    fn multi_clipboard_keeps_distinct_fragments() {
        let mut b = multi_buffer("one two\n___ ___");
        let mut clipboard = Clipboard::default();
        b.set_selection_ranges(&[(0, 3), (4, 7)]);
        b.copy(&mut clipboard);
        b.set_selection_ranges(&[(8, 11), (12, 15)]);
        b.paste(&clipboard);
        assert_eq!(contents(&b), b"one two\none two");
        b.undo();
        assert_eq!(contents(&b), b"one two\n___ ___");
    }

    #[test]
    fn multi_rectangle_tabs_wide_characters_and_short_lines() {
        let mut b = multi_buffer("a\tb\n界xyz\nx");
        b.select_rectangle(Point { x: 1, y: 0 }, Point { x: 4, y: 2 });
        b.write_raw(b"!");
        assert_eq!(contents(&b), "a!b\n!z\nx!".as_bytes());
        b.undo();
        assert_eq!(contents(&b), "a\tb\n界xyz\nx".as_bytes());
    }

    #[test]
    fn multi_occurrences_cross_gap_and_stop_at_exhaustion() {
        let mut b = multi_buffer("word word word");
        b.cursor_move_to_offset(2);
        b.write_raw(b"X");
        b.delete(CursorMovement::Grapheme, -1);
        b.cursor_move_to_offset(0);
        assert!(b.add_next_occurrence());
        assert!(b.add_next_occurrence());
        assert!(b.add_next_occurrence());
        assert!(!b.add_next_occurrence());
        assert_eq!(b.selection_count(), 3);
        b.write_raw(b"changed");
        assert_eq!(contents(&b), b"changed changed changed");
    }

    #[test]
    fn multi_byte_edits_validate_before_mutation_and_group_nested_undo() {
        let mut b = multi_buffer("abc\ndef");
        assert!(b.apply_byte_edits(&[(0..2, b"a".to_vec()), (1..3, b"b".to_vec())]).is_err());
        assert_eq!(contents(&b), b"abc\ndef");
        b.begin_undo_group();
        b.apply_byte_edits(&[(0..1, b"A\nB".to_vec()), (4..7, b"D".to_vec())]).unwrap();
        b.cursor_move_to_offset(0);
        b.write_raw(b"!");
        b.end_undo_group();
        assert_eq!(contents(&b), b"!A\nBbc\nD");
        b.undo();
        assert_eq!(contents(&b), b"abc\ndef");
        b.redo();
        assert_eq!(contents(&b), b"!A\nBbc\nD");
    }

    #[test]
    fn multi_line_helpers_cross_gap() {
        let mut b = multi_buffer("alpha beta\nalpha beta");
        b.cursor_move_to_offset(4);
        b.write_raw(b"X");
        b.delete(CursorMovement::Grapheme, -1);
        assert_eq!(b.extract_line_content(0).unwrap().0, b"alpha beta");
        assert_eq!(b.find_line_matches(b"alpha"), vec![(0, 0), (1, 11)]);
    }

    #[test]
    fn multi_indent_and_unindent_keep_all_selections() {
        let mut b = multi_buffer("one\ntwo\nthree");
        b.set_selection_ranges(&[(0, 3), (4, 7)]);
        b.indent_change(1);
        assert_eq!(contents(&b), b"    one\n    two\nthree");
        assert_eq!(b.selection_count(), 2);
        b.indent_change(-1);
        assert_eq!(contents(&b), b"one\ntwo\nthree");
        b.undo();
        assert_eq!(contents(&b), b"    one\n    two\nthree");
    }

    #[test]
    fn multi_transaction_undo_redo_under_wrap_and_multiline_edits() {
        for width in [0, 4, 8, 17] {
            for seed in 0..40usize {
                let original = "alpha beta\ngamma delta\nepsilon zeta\nlast";
                let mut b = multi_buffer(original);
                if width > 0 {
                    b.set_word_wrap(true);
                    b.set_width(width);
                    assert!(b.word_wrap_column > 0);
                }
                let a = seed % 7;
                let c = 15 + seed % 5;
                let first = if seed % 2 == 0 { b"X\nY".to_vec() } else { Vec::new() };
                let second = if seed % 3 == 0 { b"\n\n".to_vec() } else { b"Q".to_vec() };
                let mut expected = original.as_bytes().to_vec();
                expected.splice(c..c + 2, second.clone());
                expected.splice(a..a + 2, first.clone());
                b.apply_byte_edits(&[(a..a + 2, first), (c..c + 2, second)]).unwrap();
                assert_eq!(contents(&b), expected, "seed {seed}, width {width}");
                b.undo();
                assert_eq!(contents(&b), original.as_bytes(), "undo seed {seed}, width {width}");
                b.redo();
                assert_eq!(contents(&b), expected, "redo seed {seed}, width {width}");
            }
        }
    }

    #[test]
    fn multi_reject_partial_grapheme_and_crlf_byte_edits() {
        let mut b = multi_buffer("e\u{301}🙂");
        assert!(b.apply_byte_edits(&[(1..2, b"x".to_vec())]).is_err());
        assert_eq!(contents(&b), "e\u{301}🙂".as_bytes());
        b.set_crlf(true);
        b.write_raw(b"\n");
        let text = contents(&b);
        let cr = text.iter().position(|&c| c == b'\r').unwrap();
        assert!(b.apply_byte_edits(&[(cr + 1..cr + 2, b"x".to_vec())]).is_err());
        assert_eq!(contents(&b), text);
    }

    #[test]
    fn multi_render_syntax_selection_and_secondary_carets() {
        let mut b = multi_buffer("fn main() {\n\tlet 界 = 1;\n}\n");
        b.set_syntax_path(std::path::Path::new("sample.rs"));
        b.set_width(30);
        b.set_selection_ranges(&[(3, 7), (24, 24)]);
        let mut fb = Framebuffer::new();
        fb.flip(Size { width: 30, height: 6 });
        assert!(
            b.render(
                Point::default(),
                Rect { left: 0, top: 0, right: 30, bottom: 6 },
                true,
                &mut fb
            )
            .is_some()
        );
        let arena = scratch_arena(None);
        let output = fb.render(&arena);
        assert!(output.contains("main"));
        assert!(output.contains("界"));
    }

    #[test]
    fn newline_conversion_preserves_prior_undo_and_mixed_endings() {
        let mut b = multi_buffer("");
        b.apply_byte_edits(&[(0..0, b"a\nb\r\nc\n".to_vec())]).unwrap();
        b.set_selection_ranges(&[(0, 1), (6, 7)]);
        let selections = b.selection_ranges();
        b.normalize_newlines(true);
        assert_eq!(contents(&b), b"a\r\nb\r\nc\r\n");
        assert!(b.is_crlf());
        b.undo();
        assert_eq!(contents(&b), b"a\nb\r\nc\n");
        assert!(!b.is_crlf());
        assert_eq!(b.selection_ranges(), selections);
        b.redo();
        assert_eq!(contents(&b), b"a\r\nb\r\nc\r\n");
        assert!(b.is_crlf());
        b.undo();
        b.undo();
        assert!(contents(&b).is_empty());
    }

    #[test]
    fn near_capacity_replacement_and_failed_newline_conversion_are_atomic() {
        let mut b = multi_buffer("");
        b.apply_byte_edits(&[(0..0, vec![b'\n'; 128 * KIBI - 8])]).unwrap();
        let before = contents(&b);
        b.normalize_newlines(true);
        assert!(b.take_edit_capacity_error());
        assert_eq!(contents(&b), before);
        assert!(!b.is_crlf());
        b.apply_byte_edits(&[(0..b.text_length(), vec![b'x'; 128 * KIBI - 8])]).unwrap();
        assert_eq!(b.text_length(), 128 * KIBI - 8);
        b.undo();
        assert_eq!(contents(&b), before);
    }

    struct StressRng(u64);

    impl StressRng {
        fn next(&mut self, limit: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 as usize % limit
        }
    }

    fn stress_assert_model(buffer: &TextBuffer, expected: &[u8], seed: u64, step: usize) {
        assert_eq!(contents(buffer), expected, "seed {seed:#x}, step {step}");
        assert_eq!(
            buffer.logical_line_count() as usize,
            expected.iter().filter(|&&b| b == b'\n').count() + 1,
            "line count, seed {seed:#x}, step {step}"
        );
        assert!(buffer.cursor_offset() <= expected.len(), "cursor seed {seed:#x}, step {step}");
        assert!(
            buffer.cursor.visual_pos.y < buffer.stats.visual_lines,
            "invalid cursor {:?}, lines {}, seed {seed:#x}, step {step}, text {:?}, layout {:?}, measured {:?}",
            buffer.cursor,
            buffer.stats.visual_lines,
            String::from_utf8_lossy(expected),
            buffer.layout_configuration(),
            buffer.measurement_config().goto_offset(usize::MAX)
        );
        let measured = buffer.measurement_config().goto_offset(usize::MAX);
        assert_eq!(
            buffer.visual_line_count(),
            measured.visual_pos.y + 1,
            "visual count, seed {seed:#x}, step {step}"
        );
        for (a, b) in buffer.selection_ranges() {
            assert!(a <= expected.len() && b <= expected.len());
        }
    }

    fn randomized_transaction_oracle(seed: u64, steps: usize) {
        let mut rng = StressRng(seed);
        let mut b = multi_buffer("alpha\r\n界e\u{301}🙂\nbeta\tomega\n");
        let mut model = contents(&b);
        let mut history = vec![model.clone()];
        let mut position = 0;
        // Baseline content is outside the modeled history.
        b.undo_stack.clear();
        b.redo_stack.clear();
        let fragments: &[&[u8]] =
            &[b"", b"x", "界".as_bytes(), "🙂".as_bytes(), b"one\ntwo", b"\r\n", b"\t"];
        for step in 0..steps {
            if std::env::var_os("OGEDIT_STRESS_TRACE").is_some() {
                eprintln!(
                    "seed {seed:#x} step {step} rng {:#x} model {:?}",
                    rng.0,
                    String::from_utf8_lossy(&model)
                );
                eprintln!(
                    "cursor {:?}, wrap {}, width {}, tab {}, nextop {}",
                    b.cursor,
                    b.word_wrap_column,
                    b.width,
                    b.tab_size,
                    StressRng(rng.0).next(10)
                );
            }
            match rng.next(10) {
                0 if position > 0 => {
                    b.undo();
                    position -= 1;
                    model = history[position].clone();
                }
                1 if position + 1 < history.len() => {
                    b.redo();
                    position += 1;
                    model = history[position].clone();
                }
                2 => {
                    b.set_word_wrap(rng.next(2) == 0);
                    b.set_width([1, 2, 4, 9, 25, 80][rng.next(6)]);
                    b.set_tab_size(1 + rng.next(8) as CoordType);
                }
                3 => {
                    let at = [0, model.len(), usize::MAX, rng.next(model.len() + 1)][rng.next(4)];
                    b.cursor_move_to_offset(at);
                    b.clear_selection();
                    b.clear_secondary_cursors();
                }
                _ => {
                    let before_generation = b.generation();
                    let mut boundaries = vec![0];
                    for off in 1..=model.len() {
                        if b.cursor_move_to_offset_internal(Cursor::default(), off).offset == off {
                            boundaries.push(off);
                        }
                    }
                    let a = boundaries[rng.next(boundaries.len())];
                    let c = boundaries[rng.next(boundaries.len())];
                    let range = a.min(c)..a.max(c);
                    let insertion = fragments[rng.next(fragments.len())];
                    b.begin_undo_group();
                    if rng.next(3) == 0 {
                        // Exercise normalization of overlapping, reversed and duplicate cursors,
                        // then independently splice the actual normalized selections in a Vec.
                        b.set_selection_ranges(&[
                            (a, c),
                            (c, a),
                            (0, 0),
                            (model.len(), model.len()),
                        ]);
                        let ranges = b.selection_ranges();
                        let fragment = fragments[rng.next(4)];
                        for &(a, c) in ranges.iter().rev() {
                            model.splice(a.min(c)..a.max(c), fragment.iter().copied());
                        }
                        b.write_raw(fragment);
                    } else {
                        b.apply_byte_edits(&[(range.clone(), insertion.to_vec())]).unwrap();
                        model.splice(range, insertion.iter().copied());
                    }
                    b.end_undo_group();
                    assert!(!b.take_edit_capacity_error());
                    if b.generation() != before_generation {
                        history.truncate(position + 1);
                        history.push(model.clone());
                        position += 1;
                    }
                }
            }
            stress_assert_model(&b, &model, seed, step);
        }
        // Undo/redo the entire surviving branch, checking every intermediate state.
        while position > 0 {
            b.undo();
            position -= 1;
            stress_assert_model(&b, &history[position], seed, steps + position);
        }
        while position + 1 < history.len() {
            b.redo();
            position += 1;
            stress_assert_model(&b, &history[position], seed, steps * 2 + position);
        }
    }

    #[test]
    fn stress_randomized_transactions_match_byte_vector_oracle() {
        for seed in [1, 7, 0xdead_beef, 0x5eed_cafe, 0x1234_5678, 0x7fff_ffff, 0xf00d, 0xace] {
            randomized_transaction_oracle(seed, 400);
        }
    }

    #[test]
    #[ignore = "long deterministic stress run"]
    fn stress_randomized_transactions_long() {
        for seed in 1..=128 {
            randomized_transaction_oracle(seed * 0x9e37_79b9, 1000);
        }
    }

    #[test]
    fn stress_malformed_bytes_render_navigate_replace_and_undo() {
        let mut rng = StressRng(0xcafe_d00d);
        let mut buffer = multi_buffer("");
        buffer.set_syntax_path(std::path::Path::new("stress.rs"));
        let mut fb = Framebuffer::new();
        fb.flip(Size { width: 20, height: 5 });
        for iteration in 0..256 {
            let before = contents(&buffer);
            let bytes: Vec<u8> = (0..rng.next(256)).map(|_| rng.next(256) as u8).collect();
            buffer.apply_byte_edits(&[(0..buffer.text_length(), bytes.clone())]).unwrap();
            buffer.set_word_wrap(iteration % 2 == 0);
            buffer.set_width(2 + rng.next(19) as CoordType);
            buffer.set_tab_size(1 + rng.next(8) as CoordType);
            for _ in 0..8 {
                buffer.cursor_move_to_offset(rng.next(bytes.len() + 3));
                assert!(buffer.cursor_offset() <= bytes.len());
            }
            buffer.render(
                Point::default(),
                Rect { left: 0, top: 0, right: 20, bottom: 5 },
                true,
                &mut fb,
            );
            stress_assert_model(&buffer, &bytes, 0xcafe_d00d, iteration);
            if before != bytes || !bytes.is_empty() {
                buffer.undo();
                assert_eq!(contents(&buffer), before);
                buffer.redo();
                assert_eq!(contents(&buffer), bytes);
            }
        }
    }

    #[test]
    fn narrow_wrapped_tabs_and_resize_undo_keep_cursor_in_view() {
        let mut buffer = multi_buffer("o界\tx界x\t");
        for width in 2..9 {
            for tab in 1..9 {
                buffer.set_word_wrap(true);
                buffer.set_width(width);
                buffer.set_tab_size(tab);
                for offset in 0..=buffer.text_length() {
                    buffer.cursor_move_to_offset(offset);
                    assert!(buffer.cursor_visual_pos().x <= width);
                    assert!(buffer.cursor_visual_pos().y < buffer.visual_line_count());
                }
            }
        }
        let before = contents(&buffer);
        buffer.apply_byte_edits(&[(0..1, b"one\ntwo\t".to_vec())]).unwrap();
        buffer.set_width(80);
        buffer.undo();
        stress_assert_model(&buffer, &before, 0, 0);
        buffer.redo();
        let after = contents(&buffer);
        stress_assert_model(&buffer, &after, 0, 1);
    }

    #[test]
    fn stress_invalid_edits_leave_content_history_and_selections_intact() {
        let mut b = multi_buffer("");
        b.apply_byte_edits(&[(0..0, "e\u{301}🙂\r\nlast".as_bytes().to_vec())]).unwrap();
        b.set_selection_ranges(&[(0, 3), (7, 7)]);
        let before = contents(&b);
        let selections = b.selection_ranges();
        let generation = b.generation();
        for ranges in [
            vec![(usize::MAX..usize::MAX, vec![b'x'])],
            vec![(0..usize::MAX, vec![])],
            vec![(Range { start: 3, end: 2 }, vec![b'x'])],
            vec![(1..2, vec![])],
            vec![(8..9, vec![])],
            vec![(0..3, vec![]), (0..0, vec![b'x'])],
        ] {
            assert!(b.apply_byte_edits(&ranges).is_err());
            assert_eq!(contents(&b), before);
            assert_eq!(b.selection_ranges(), selections);
            assert_eq!(b.generation(), generation);
        }
        b.apply_byte_edits(&[(0..before.len(), vec![b'x'; 256 * KIBI])]).unwrap_err();
        assert_eq!(contents(&b), before);
        assert_eq!(b.selection_ranges(), selections);
        b.undo();
        assert!(contents(&b).is_empty());
    }

    #[test]
    #[ignore = "requires dynamically loaded ICU"]
    fn stress_search_survives_moves_reloads_edits_and_drops() {
        icu::init().unwrap();
        for iteration in 0..128 {
            let mut original = multi_buffer("🙂 first needle\r\n界 second needle");
            original.find_and_select("needle", SearchOptions::default()).unwrap();
            let mut moved = vec![original];
            for _ in 0..5 {
                moved.push(multi_buffer("padding"));
            }
            let mut b = moved.remove(0);
            b.find_and_select("needle", SearchOptions::default()).unwrap();
            let (start, end) = b.selection_range().unwrap();
            assert_eq!(b.extract_bytes(start.offset..end.offset), b"needle", "move {iteration}");
            let replacement = multi_buffer("new needle");
            b.replace_loaded_content(replacement);
            b.find_and_select("needle", SearchOptions::default()).unwrap();
            let (start, end) = b.selection_range().unwrap();
            assert_eq!(b.extract_bytes(start.offset..end.offset), b"needle");
            b.select_all();
            b.write_raw(b"empty");
            b.find_and_select("needle", SearchOptions::default()).unwrap();
            assert!(!b.has_selection());
            b.undo();
            b.find_and_select("needle", SearchOptions::default()).unwrap();
            assert!(b.has_selection());
        }
    }

    // ===== SelectionHighlights::should_highlight Tests =====

    #[test]
    fn test_should_highlight_empty() {
        assert!(!SelectionHighlights::should_highlight(b""));
    }

    #[test]
    fn test_should_highlight_whitespace_only() {
        assert!(!SelectionHighlights::should_highlight(b" "));
        assert!(!SelectionHighlights::should_highlight(b"  "));
        assert!(!SelectionHighlights::should_highlight(b"\t"));
        assert!(!SelectionHighlights::should_highlight(b"\n"));
        assert!(!SelectionHighlights::should_highlight(b"\r\n"));
        assert!(!SelectionHighlights::should_highlight(b"   \t\n  "));
    }

    #[test]
    fn test_should_highlight_single_letter() {
        // Single ASCII letters should NOT be highlighted
        assert!(!SelectionHighlights::should_highlight(b"a"));
        assert!(!SelectionHighlights::should_highlight(b"z"));
        assert!(!SelectionHighlights::should_highlight(b"A"));
        assert!(!SelectionHighlights::should_highlight(b"Z"));
        assert!(!SelectionHighlights::should_highlight(b"m"));
    }

    #[test]
    fn test_should_highlight_single_non_letter() {
        // Single non-letter characters SHOULD be highlighted
        assert!(SelectionHighlights::should_highlight(b"0"));
        assert!(SelectionHighlights::should_highlight(b"9"));
        assert!(SelectionHighlights::should_highlight(b"="));
        assert!(SelectionHighlights::should_highlight(b"+"));
        assert!(SelectionHighlights::should_highlight(b"-"));
        assert!(SelectionHighlights::should_highlight(b"!"));
        assert!(SelectionHighlights::should_highlight(b"@"));
        assert!(SelectionHighlights::should_highlight(b"["));
        assert!(SelectionHighlights::should_highlight(b"{"));
    }

    #[test]
    fn test_should_highlight_multiple_chars() {
        // 2+ characters should always be highlighted
        assert!(SelectionHighlights::should_highlight(b"ab"));
        assert!(SelectionHighlights::should_highlight(b"foo"));
        assert!(SelectionHighlights::should_highlight(b"test"));
        assert!(SelectionHighlights::should_highlight(b"12"));
        assert!(SelectionHighlights::should_highlight(b"=="));
        assert!(SelectionHighlights::should_highlight(b"fn"));
        assert!(SelectionHighlights::should_highlight(b"hello world"));
    }

    #[test]
    fn test_should_highlight_mixed_content() {
        // Mixed content with whitespace and non-whitespace
        assert!(SelectionHighlights::should_highlight(b"a "));
        assert!(SelectionHighlights::should_highlight(b" b"));
        assert!(SelectionHighlights::should_highlight(b" x "));
    }

    // ===== extract_line_content Tests =====

    #[test]
    fn test_extract_line_content_simple() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false); // Force LF mode for consistent offsets
        buf.write_raw(b"line1\nline2\nline3");

        let (content, offset) = buf.extract_line_content(0).unwrap();
        assert_eq!(content, b"line1");
        assert_eq!(offset, 0);

        let (content, offset) = buf.extract_line_content(1).unwrap();
        assert_eq!(content, b"line2");
        assert_eq!(offset, 6);

        let (content, offset) = buf.extract_line_content(2).unwrap();
        assert_eq!(content, b"line3");
        assert_eq!(offset, 12);
    }

    #[test]
    fn test_extract_line_content_crlf() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(true);
        buf.write_raw(b"line1\r\nline2\r\n");

        let (content, _) = buf.extract_line_content(0).unwrap();
        assert_eq!(content, b"line1"); // Should strip \r

        let (content, _) = buf.extract_line_content(1).unwrap();
        assert_eq!(content, b"line2");
    }

    #[test]
    fn test_extract_line_content_out_of_bounds() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"line1\nline2");

        assert!(buf.extract_line_content(-1).is_none());
        assert!(buf.extract_line_content(2).is_none());
        assert!(buf.extract_line_content(100).is_none());
    }

    #[test]
    fn test_extract_line_content_empty_lines() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"\n\nline3\n");

        let (content, _) = buf.extract_line_content(0).unwrap();
        assert!(content.is_empty());

        let (content, _) = buf.extract_line_content(1).unwrap();
        assert!(content.is_empty());

        let (content, _) = buf.extract_line_content(2).unwrap();
        assert_eq!(content, b"line3");
    }

    #[test]
    fn test_extract_line_content_single_line() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"only one line");

        let (content, offset) = buf.extract_line_content(0).unwrap();
        assert_eq!(content, b"only one line");
        assert_eq!(offset, 0);

        assert!(buf.extract_line_content(1).is_none());
    }

    // ===== find_line_matches Tests =====

    #[test]
    fn test_find_line_matches_simple() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"foo bar foo\nbaz foo\nfoo");

        let matches = buf.find_line_matches(b"foo");
        assert_eq!(matches.len(), 4);

        // First "foo" on line 0
        assert_eq!(matches[0].0, 0); // line number

        // Second "foo" on line 0
        assert_eq!(matches[1].0, 0);

        // "foo" on line 1
        assert_eq!(matches[2].0, 1);

        // "foo" on line 2
        assert_eq!(matches[3].0, 2);
    }

    #[test]
    fn test_find_line_matches_no_matches() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"hello world");

        let matches = buf.find_line_matches(b"xyz");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_line_matches_empty_pattern() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"hello world");

        let matches = buf.find_line_matches(b"");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_line_matches_single_char() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"a b a\na");

        let matches = buf.find_line_matches(b"a");
        // "a b a\na" has 3 occurrences of 'a': positions 0, 4, 6
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].0, 0); // First 'a' on line 0
        assert_eq!(matches[1].0, 0); // Second 'a' on line 0
        assert_eq!(matches[2].0, 1); // Third 'a' on line 1
    }

    // ===== offset_to_line Tests =====

    #[test]
    fn test_offset_to_line_simple() {
        let mut buf = TextBuffer::new(false).unwrap();
        buf.set_crlf(false);
        buf.write_raw(b"line1\nline2\nline3");

        assert_eq!(buf.offset_to_line(0), 0); // Start of line 0
        assert_eq!(buf.offset_to_line(3), 0); // Middle of line 0
        assert_eq!(buf.offset_to_line(5), 0); // End of "line1"
        assert_eq!(buf.offset_to_line(6), 1); // Start of line 1 (after \n)
        assert_eq!(buf.offset_to_line(12), 2); // Start of line 2
    }

    fn assert_layout_consistent(buffer: &TextBuffer) {
        let measured = buffer.measurement_config().goto_logical(Point::MAX);
        assert_eq!(buffer.visual_line_count(), measured.visual_pos.y + 1);
        assert_eq!(buffer.logical_line_count(), measured.logical_pos.y + 1);
        let measured_cursor = buffer.measurement_config().goto_offset(buffer.cursor.offset);
        assert_eq!(buffer.cursor.logical_pos, measured_cursor.logical_pos);
        assert_eq!(buffer.cursor.visual_pos, measured_cursor.visual_pos);
    }

    #[test]
    fn legacy_duplicate_preserves_empty_lines_partial_selections_and_undo() {
        for (original, range, expected, copy) in [
            ("a\n\nlast", (0, 3), "a\n\na\n\nlast", "a\n"),
            ("first\nsecond\ntail", (0, 8), "first\nsefirst\nsecond\ntail", "first\nse"),
            ("é🙂\nnext", (0, 6), "é🙂\né🙂\nnext", "é🙂"),
        ] {
            let mut buffer = multi_buffer(original);
            buffer.set_selection_ranges(&[range]);
            let before = buffer.selection_ranges();
            buffer.duplicate_line_or_selection();
            assert_eq!(contents(&buffer), expected.as_bytes());
            let (a, b) = buffer.selection_ranges()[0];
            assert_eq!(buffer.extract_bytes(a.min(b)..a.max(b)), copy.as_bytes());
            buffer.undo();
            assert_eq!(contents(&buffer), original.as_bytes());
            assert_eq!(buffer.selection_ranges(), before);
            buffer.redo();
            assert_eq!(contents(&buffer), expected.as_bytes());
        }
    }

    #[test]
    fn legacy_duplicate_empty_eof_repetition_and_mixed_newlines() {
        let mut empty = multi_buffer("");
        empty.duplicate_line_or_selection();
        assert_eq!(contents(&empty), b"\n");
        empty.duplicate_line_or_selection();
        assert_eq!(contents(&empty), b"\n\n");
        empty.undo();
        assert_eq!(contents(&empty), b"\n");
        empty.undo();
        assert!(contents(&empty).is_empty());

        let mut final_newline = multi_buffer("last");
        final_newline.set_insert_final_newline(true);
        final_newline.set_crlf(true);
        final_newline.duplicate_line_or_selection();
        assert_eq!(contents(&final_newline), b"last\r\nlast\r\n");
        assert_eq!(final_newline.selection_ranges(), vec![(6, 10)]);
        final_newline.undo();
        assert_eq!(contents(&final_newline), b"last");

        let mut mixed = multi_buffer("");
        mixed.apply_byte_edits(&[(0..0, b"a\r\nb\nc".to_vec())]).unwrap();
        mixed.cursor_move_to_offset(0);
        mixed.duplicate_line_or_selection();
        assert_eq!(contents(&mixed), b"a\r\na\r\nb\nc");
        mixed.duplicate_line_or_selection();
        assert_eq!(contents(&mixed), b"a\r\na\r\na\r\nb\nc");
    }

    #[test]
    fn legacy_wrapped_multiline_paste_and_duplicate_keep_measured_layout() {
        for width in [4, 8, 17] {
            for crlf in [false, true] {
                let mut buffer = multi_buffer("header\nalpha beta gamma delta\nlast");
                buffer.set_crlf(crlf);
                buffer.set_word_wrap(true);
                buffer.set_width(width);
                assert!(buffer.word_wrap_column > 0);
                for (start, end) in [(8, 8), (8, 11), (0, 20)] {
                    buffer.set_selection_ranges(&[(start, end)]);
                    let before = contents(&buffer);
                    let selection_before = buffer.selection_ranges();
                    buffer.write_raw("new é🙂\nsecond long line\nthird".as_bytes());
                    assert_layout_consistent(&buffer);
                    let after = contents(&buffer);
                    buffer.undo();
                    assert_eq!(contents(&buffer), before);
                    assert_eq!(buffer.selection_ranges(), selection_before);
                    assert_layout_consistent(&buffer);
                    buffer.redo();
                    assert_eq!(contents(&buffer), after);
                    assert_layout_consistent(&buffer);
                    buffer.undo();
                }
                buffer.cursor_move_to_logical(Point { x: 6, y: 1 });
                buffer.duplicate_line_or_selection();
                assert_layout_consistent(&buffer);
                buffer.duplicate_line_or_selection();
                assert_layout_consistent(&buffer);
                buffer.undo();
                assert_layout_consistent(&buffer);
                buffer.redo();
                assert_layout_consistent(&buffer);
            }
        }
    }

    #[test]
    fn legacy_reload_matching_uses_complete_lines_nearest_tie_and_gap() {
        let mut buffer = multi_buffer("target suffix\nnot target\ntarget\nother\ntarget");
        buffer.cursor_move_to_offset(30);
        buffer.write_raw(b"X");
        buffer.delete(CursorMovement::Grapheme, -1);
        assert_eq!(buffer.nearest_matching_line(b"target", 0), Some(2));
        assert_eq!(buffer.nearest_matching_line(b"target", 3), Some(2));
        assert_eq!(buffer.nearest_matching_line(b"target", 5), Some(4));
        assert_eq!(buffer.nearest_matching_line(b"", 1), None);
        assert_eq!(buffer.nearest_matching_line(b"target\nother", 1), None);
        assert_eq!(buffer.nearest_matching_line(b"absent", 0), None);
        let mut crlf = multi_buffer("");
        crlf.apply_byte_edits(&[(0..0, "é🙂\r\nother\nlast\r".as_bytes().to_vec())]).unwrap();
        assert_eq!(crlf.nearest_matching_line("é🙂".as_bytes(), 0), Some(0));
        assert_eq!(crlf.nearest_matching_line(b"last", 2), None);
        assert_eq!(crlf.nearest_matching_line(b"last\r", 2), Some(2));
    }

    #[test]
    fn legacy_line_selection_is_inclusive_reversible_and_wrap_independent() {
        let mut buffer = multi_buffer("long first line with wraps\nsecond\nthird");
        buffer.set_word_wrap(true);
        buffer.set_width(7);
        assert_eq!(buffer.logical_line_at_visual_row(1), 0);
        buffer.select_lines(0, 1);
        assert_eq!(buffer.selection_ranges(), vec![(0, 34)]);
        buffer.select_lines(1, 0);
        assert_eq!(buffer.selection_ranges(), vec![(34, 0)]);
        assert_eq!(buffer.selection_anchor_line(), 1);
        buffer.select_lines(2, 2);
        assert_eq!(buffer.selection_ranges(), vec![(34, 39)]);
        buffer.select_lines(2, 100);
        assert_eq!(buffer.selection_ranges(), vec![(34, 39)]);
    }

    #[test]
    fn legacy_selection_highlights_classify_unicode_graphemes_and_whitespace() {
        for text in ["é", "א", "界", "e\u{301}", "\u{a0}", "\u{2003} "] {
            assert!(!SelectionHighlights::should_highlight(text.as_bytes()), "{text:?}");
        }
        for text in ["éé", "e\u{301}a", "🙂", "👨‍👩‍👧", "é ", "é\n"] {
            assert!(SelectionHighlights::should_highlight(text.as_bytes()), "{text:?}");
        }
    }
}
