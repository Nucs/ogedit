// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::mem;
use std::path::{Path, PathBuf};

use ogedit::framebuffer::IndexedColor;
use ogedit::helpers::*;
use ogedit::oklab::StraightRgba;
use ogedit::tui::*;
use ogedit::{apperr, buffer, icu, sys};

use crate::config::Config;
use crate::documents::DocumentManager;
use crate::localization::*;
use crate::logging;
use crate::watch::FileWatcher;

#[repr(transparent)]
pub struct FormatApperr(apperr::Error);

impl From<apperr::Error> for FormatApperr {
    fn from(err: apperr::Error) -> Self {
        Self(err)
    }
}

impl std::fmt::Display for FormatApperr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            apperr::APP_ICU_MISSING => f.write_str(loc(LocId::ErrorIcuMissing)),
            apperr::Error::App(code) => write!(f, "Unknown app error code: {code}"),
            apperr::Error::Icu(code) => icu::apperr_format(f, code),
            apperr::Error::Sys(code) => sys::apperr_format(f, code),
        }
    }
}

pub struct DisplayablePathBuf {
    value: PathBuf,
    str: Cow<'static, str>,
}

impl DisplayablePathBuf {
    #[allow(dead_code, reason = "only used on Windows")]
    pub fn from_string(string: String) -> Self {
        let str = Cow::Borrowed(string.as_str());
        let str = unsafe { mem::transmute::<Cow<'_, str>, Cow<'_, str>>(str) };
        let value = PathBuf::from(string);
        Self { value, str }
    }

    pub fn from_path(value: PathBuf) -> Self {
        let str = value.to_string_lossy();
        let str = unsafe { mem::transmute::<Cow<'_, str>, Cow<'_, str>>(str) };
        Self { value, str }
    }

    pub fn as_path(&self) -> &Path {
        &self.value
    }

    pub fn as_str(&self) -> &str {
        &self.str
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.value.as_os_str().as_encoded_bytes()
    }
}

impl Default for DisplayablePathBuf {
    fn default() -> Self {
        Self { value: Default::default(), str: Cow::Borrowed("") }
    }
}

impl Clone for DisplayablePathBuf {
    fn clone(&self) -> Self {
        Self::from_path(self.value.clone())
    }
}

impl From<OsString> for DisplayablePathBuf {
    fn from(s: OsString) -> Self {
        Self::from_path(PathBuf::from(s))
    }
}

impl<T: ?Sized + AsRef<OsStr>> From<&T> for DisplayablePathBuf {
    fn from(s: &T) -> Self {
        Self::from_path(PathBuf::from(s))
    }
}

pub struct StateSearch {
    pub kind: StateSearchKind,
    pub focus: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StateSearchKind {
    Hidden,
    Disabled,
    Search,
    Replace,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StateFilePicker {
    None,
    Open,
    SaveAs,

    SaveAsShown, // Transitioned from SaveAs
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StateEncodingChange {
    None,
    Convert,
    Reopen,
}

#[derive(Default)]
pub struct OscTitleFileStatus {
    pub filename: String,
    pub dirty: bool,
}

/// Tracks selection state for logging purposes
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct SelectionState {
    pub has_selection: bool,
    pub start: Point,
    pub end: Point,
    pub start_offset: usize,
    pub end_offset: usize,
}

/// Describes where a mouse click landed
#[derive(Default, Clone, Copy)]
pub enum ClickTarget {
    #[default]
    Unknown,
    Editor,
    Menubar,
    Statusbar,
    Dialog,
    FilePicker,
}

impl ClickTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClickTarget::Unknown => "unknown",
            ClickTarget::Editor => "editor",
            ClickTarget::Menubar => "menubar",
            ClickTarget::Statusbar => "statusbar",
            ClickTarget::Dialog => "dialog",
            ClickTarget::FilePicker => "filepicker",
        }
    }
}

/// Tracks the previous state for logging purposes
#[derive(Default)]
pub struct LoggingTracker {
    pub cursor_pos: Point,
    pub cursor_offset: usize,
    pub text_generation: u32,
    pub selection: SelectionState,
    /// Set to true when a mouse click is processed, cleared after logging
    pub last_input_was_click: bool,
    /// Where the last click landed (set by UI handlers)
    pub last_click_target: ClickTarget,
    /// Last time we logged a content snapshot (in milliseconds since program start)
    pub last_snapshot_time_ms: u64,
    /// Buffer generation at the last change (to detect idle periods)
    pub last_change_generation: u32,
    /// Time of the last content change (in milliseconds since program start)
    pub last_change_time_ms: u64,
}

pub struct State {
    pub menubar_color_bg: StraightRgba,
    pub menubar_color_fg: StraightRgba,

    pub config: Config,
    pub documents: DocumentManager,

    /// The working directory when the editor was started (for per-project settings)
    pub startup_cwd: String,

    // A ring buffer of the last 10 errors.
    pub error_log: [String; 10],
    pub error_log_index: usize,
    pub error_log_count: usize,

    pub wants_file_picker: StateFilePicker,
    pub file_picker_pending_dir: DisplayablePathBuf,
    pub file_picker_pending_dir_revision: u64, // Bumped every time `file_picker_pending_dir` changes.
    pub file_picker_pending_name: PathBuf,
    pub file_picker_entries: Option<[Vec<DisplayablePathBuf>; 3]>, // ["..", directories, files]
    pub file_picker_overwrite_warning: Option<PathBuf>,            // The path the warning is about.
    pub file_picker_autocomplete: Vec<DisplayablePathBuf>,

    pub wants_search: StateSearch,
    pub search_needle: String,
    pub search_replacement: String,
    pub search_options: buffer::SearchOptions,
    pub search_success: bool,

    pub wants_encoding_picker: bool,
    pub wants_encoding_change: StateEncodingChange,
    pub encoding_picker_needle: String,
    pub encoding_picker_results: Option<Vec<icu::Encoding>>,

    pub wants_save: bool,
    pub wants_statusbar_focus: bool,
    pub wants_indentation_picker: bool,
    pub wants_go_to_file: bool,
    pub wants_about: bool,
    pub wants_close: bool,
    pub wants_exit: bool,
    pub wants_reload: bool,
    pub wants_goto: bool,
    pub goto_target: String,
    pub goto_invalid: bool,

    pub osc_title_file_status: OscTitleFileStatus,
    pub osc_clipboard_sync: bool,
    pub osc_clipboard_always_send: bool,
    pub exit: bool,

    pub logging_tracker: LoggingTracker,

    /// Cached flag for file changed on disk (set by file watcher)
    pub file_changed_cached: bool,

    /// File watcher for detecting external modifications
    pub file_watcher: FileWatcher,
}

impl State {
    pub fn new() -> apperr::Result<Self> {
        let startup_cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(Self {
            menubar_color_bg: StraightRgba::zero(),
            menubar_color_fg: StraightRgba::zero(),

            config: Config::load(),
            documents: Default::default(),

            startup_cwd,

            error_log: [const { String::new() }; 10],
            error_log_index: 0,
            error_log_count: 0,

            wants_file_picker: StateFilePicker::None,
            file_picker_pending_dir: Default::default(),
            file_picker_pending_dir_revision: 0,
            file_picker_pending_name: Default::default(),
            file_picker_entries: None,
            file_picker_overwrite_warning: None,
            file_picker_autocomplete: Vec::new(),

            wants_search: StateSearch { kind: StateSearchKind::Hidden, focus: false },
            search_needle: Default::default(),
            search_replacement: Default::default(),
            search_options: Default::default(),
            search_success: true,

            wants_encoding_picker: false,
            encoding_picker_needle: Default::default(),
            encoding_picker_results: Default::default(),

            wants_save: false,
            wants_statusbar_focus: false,
            wants_encoding_change: StateEncodingChange::None,
            wants_indentation_picker: false,
            wants_go_to_file: false,
            wants_about: false,
            wants_close: false,
            wants_exit: false,
            wants_reload: false,
            wants_goto: false,
            goto_target: Default::default(),
            goto_invalid: false,

            osc_title_file_status: Default::default(),
            osc_clipboard_sync: false,
            osc_clipboard_always_send: false,
            exit: false,

            logging_tracker: Default::default(),

            file_changed_cached: false,

            file_watcher: FileWatcher::new(),
        })
    }
}

pub fn draw_add_untitled_document(ctx: &mut Context, state: &mut State) {
    match state.documents.add_untitled(&state.config) {
        Ok(doc) => {
            logging::log_file_new(&doc.filename);
        }
        Err(err) => {
            error_log_add(ctx, state, err);
        }
    }
}

pub fn error_log_add(ctx: &mut Context, state: &mut State, err: apperr::Error) {
    let msg = format!("{}", FormatApperr::from(err));
    if !msg.is_empty() {
        logging::log_error(&msg);
        logging::log_dialog_open("Error");
        state.error_log[state.error_log_index] = msg;
        state.error_log_index = (state.error_log_index + 1) % state.error_log.len();
        state.error_log_count = state.error_log.len().min(state.error_log_count + 1);
        ctx.needs_rerender();
    }
}

pub fn draw_error_log(ctx: &mut Context, state: &mut State) {
    ctx.modal_begin("error", loc(LocId::ErrorDialogTitle));
    ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
    {
        ctx.block_begin("content");
        ctx.attr_padding(Rect::three(0, 2, 1));
        {
            let off = state.error_log_index + state.error_log.len() - state.error_log_count;

            for i in 0..state.error_log_count {
                let idx = (off + i) % state.error_log.len();
                let msg = &state.error_log[idx][..];

                if !msg.is_empty() {
                    ctx.next_block_id_mixin(i as u64);
                    ctx.label("error", msg);
                    ctx.attr_overflow(Overflow::TruncateTail);
                }
            }
        }
        ctx.block_end();

        if ctx.button("ok", loc(LocId::Ok), ButtonStyle::default()) {
            logging::log_dialog_close("Error", "Ok");
            state.error_log_count = 0;
        }
        ctx.attr_position(Position::Center);
        ctx.inherit_focus();
    }
    if ctx.modal_end() {
        logging::log_dialog_close("Error", "Escape");
        state.error_log_count = 0;
    }
}

/// Check for state changes and log them
/// Call this at the end of each frame
pub fn log_state_changes(state: &mut State) {
    let Some(doc) = state.documents.active() else {
        return;
    };

    let tb = doc.buffer.borrow();
    let cursor_pos = tb.cursor_logical_pos();
    let cursor_offset = tb.cursor_offset();
    let generation = tb.generation();

    // Get current selection state
    let current_selection = if let Some((beg, end)) = tb.selection_range() {
        SelectionState {
            has_selection: true,
            start: beg.logical_pos,
            end: end.logical_pos,
            start_offset: beg.offset,
            end_offset: end.offset,
        }
    } else {
        SelectionState::default()
    };

    let tracker = &mut state.logging_tracker;

    // Check for cursor movement (only when not editing text)
    if (cursor_pos != tracker.cursor_pos || cursor_offset != tracker.cursor_offset)
        && generation == tracker.text_generation
    {
        let method = if tracker.last_input_was_click {
            // Include target info for clicks
            match tracker.last_click_target {
                ClickTarget::Editor => "click:editor",
                ClickTarget::Menubar => "click:menubar",
                ClickTarget::Statusbar => "click:statusbar",
                ClickTarget::Dialog => "click:dialog",
                ClickTarget::FilePicker => "click:filepicker",
                ClickTarget::Unknown => "click",
            }
        } else {
            "navigation"
        };
        logging::log_cursor_move(
            tracker.cursor_pos,
            cursor_pos,
            tracker.cursor_offset,
            cursor_offset,
            method,
        );
    }

    // Clear the click flags after processing
    tracker.last_input_was_click = false;
    tracker.last_click_target = ClickTarget::Unknown;

    // Check for selection changes
    if current_selection != tracker.selection {
        if current_selection.has_selection {
            // Selection exists - get content and log it
            let content = tb.extract_bytes(current_selection.start_offset..current_selection.end_offset);
            logging::log_selection(
                current_selection.start,
                current_selection.end,
                current_selection.start_offset,
                current_selection.end_offset,
                Some(&content),
            );
        } else if tracker.selection.has_selection {
            // Selection was cleared
            logging::log_selection_clear();
        }
        tracker.selection = current_selection;
    }

    // Update tracker state
    if generation != tracker.text_generation {
        tracker.text_generation = generation;
    }
    tracker.cursor_pos = cursor_pos;
    tracker.cursor_offset = cursor_offset;
}

/// Check if we should log a content snapshot (1 second of idle after last change)
/// Call this at the end of each frame with the current timestamp in milliseconds
pub fn log_periodic_content_snapshot(state: &mut State, now_ms: u64) {
    let Some(doc) = state.documents.active_mut() else {
        return;
    };

    let generation = doc.buffer.borrow().generation();
    let tracker = &mut state.logging_tracker;

    // Detect content change
    if generation != tracker.last_change_generation {
        tracker.last_change_generation = generation;
        tracker.last_change_time_ms = now_ms;
    }

    // Check if 1 second has passed since the last change
    // and we haven't logged a snapshot since that change
    const SNAPSHOT_INTERVAL_MS: u64 = 1000;

    if tracker.last_change_time_ms > 0
        && now_ms >= tracker.last_change_time_ms + SNAPSHOT_INTERVAL_MS
        && tracker.last_snapshot_time_ms < tracker.last_change_time_ms + SNAPSHOT_INTERVAL_MS
    {
        // Time to log a snapshot
        let mut content = String::new();
        doc.buffer.borrow().copy_content(&mut content);

        logging::log_content_snapshot(&content, &doc.filename);
        tracker.last_snapshot_time_ms = now_ms;
    }
}

/// Reload the current document from disk
/// Used when file has been modified externally
pub fn reload_file_from_disk(state: &mut State) -> apperr::Result<()> {
    if let Some(doc) = state.documents.active_mut() {
        // Reload the file with current encoding
        doc.reread(None)?;
        logging::log_action(&format!("FILE_RELOADED: {}", doc.filename));

        // Re-watch the file to update the watcher's baseline timestamp
        if let Some(path) = &doc.path {
            state.file_watcher.unwatch(path);
            state.file_watcher.watch(path);
        }

        // Clear the cache so indicator disappears after reload
        state.file_changed_cached = false;
    }
    Ok(())
}
