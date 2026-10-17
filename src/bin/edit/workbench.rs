//! Project tools share the editor's buffers, guarded byte edits, and undo history.
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ogedit::buffer::{RcTextBuffer, TextBuffer};
use ogedit::helpers::{Point, Size};
use ogedit::input::{kbmod, vk};
use ogedit::tui::{ButtonStyle, Context, ListSelection, Overflow};

use crate::data_lens::{DataLens, LensKind};
use crate::palette::FileIndex;
use crate::state::{FormatApperr, State, StateFilePicker, StateSearchKind};
use crate::workspace_search::{
    OpenDocumentSnapshot, SearchHit, SearchOptions, SearchReport, SearchTask,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Panel {
    #[default]
    None,
    Commands,
    Files,
    Search,
    Lens,
    Cell,
    Diff,
    Recovery,
    MacroName,
    LogPattern,
}

#[derive(Clone)]
enum Destination {
    File(PathBuf),
    Untitled(usize),
    Command(&'static str),
    Macro(String),
}

#[derive(Default)]
pub struct Workbench {
    pub panel: Panel,
    pub message: String,
    pub syntax_disabled: bool,
    query: String,
    index: usize,
    files: Option<FileIndex>,
    root: String,
    needle: String,
    options: SearchOptions,
    search: Option<SearchTask>,
    report: Option<SearchReport>,
    lens: Option<DataLens>,
    lens_buffer: Option<RcTextBuffer>,
    column: usize,
    draft: Option<RcTextBuffer>,
    return_panel: Panel,
    diff: String,
    diff_rows: Vec<std::ops::Range<usize>>,
    diff_index: usize,
    macro_name: String,
    checkpoint_at: Option<Instant>,
    log_pattern: String,
    pending_hit: Option<SearchHit>,
}

const COMMANDS: &[(&str, &str)] = &[
    ("new", "New file"),
    ("open", "Open file dialog"),
    ("native-browse", "Browse File"),
    ("reload", "Reload from disk"),
    ("find", "Find in file"),
    ("replace", "Replace in file"),
    ("goto", "Go to line"),
    ("close", "Close file"),
    ("exit", "Exit"),
    ("files", "Open file                         Ctrl+P"),
    ("search", "Search workspace                  Ctrl+Shift+F"),
    ("lens", "Data view                         Ctrl+Shift+L"),
    ("csv", "CSV view"),
    ("tsv", "TSV view"),
    ("json", "JSON view"),
    ("log", "Log view"),
    ("compare", "Compare with disk"),
    ("recover", "Recover unsaved work"),
    ("record", "Record / stop macro               Ctrl+Shift+M"),
    ("replay", "Replay last macro                 Ctrl+Alt+M"),
    ("next", "Add next occurrence               Ctrl+Alt+D"),
    ("above", "Add cursor above                  Ctrl+Alt+Up"),
    ("below", "Add cursor below                  Ctrl+Alt+Down"),
    ("bracket", "Go to matching bracket            Ctrl+]"),
    ("syntax", "Toggle syntax colors"),
    ("save", "Save                              Ctrl+S"),
    ("saveas", "Save as                           Ctrl+Shift+S"),
    ("wrap", "Toggle word wrap                  Alt+Z"),
    ("undo", "Undo                              Ctrl+Z"),
    ("redo", "Redo                              Ctrl+Y"),
];

pub fn execute(state: &mut State, command: &str) {
    let mut work = std::mem::take(&mut state.workbench);
    work.command(state, command);
    state.workbench = work;
}

pub fn open(state: &mut State, panel: Panel) {
    state.workbench.close();
    state.workbench.panel = panel;
    state.workbench.query.clear();
    state.workbench.index = 0;
    state.workbench.message.clear();
    if panel == Panel::Files {
        match FileIndex::start(PathBuf::from(&state.startup_cwd)) {
            Ok(index) => state.workbench.files = Some(index),
            Err(err) => state.workbench.message = err,
        }
    }
    if state.workbench.root.is_empty() {
        state.workbench.root = state.startup_cwd.clone();
    }
}

pub fn shortcuts(ctx: &mut Context, state: &mut State) {
    if ctx.consume_shortcut(kbmod::CTRL_SHIFT | vk::P)
        || (state.workbench.panel == Panel::None && ctx.consume_shortcut(vk::F2))
    {
        open(state, Panel::Commands);
    } else if ctx.consume_shortcut(kbmod::CTRL_SHIFT | vk::F) {
        open(state, Panel::Search);
    } else if ctx.consume_shortcut(kbmod::CTRL_SHIFT | vk::L) {
        let mut work = std::mem::take(&mut state.workbench);
        work.open_lens(state, None);
        state.workbench = work;
    } else if ctx.consume_shortcut(kbmod::CTRL_SHIFT | vk::M)
        || (state.workbench.panel == Panel::None && ctx.consume_shortcut(vk::F4))
    {
        let mut work = std::mem::take(&mut state.workbench);
        work.command(state, "record");
        state.workbench = work;
    } else if ctx.consume_shortcut(kbmod::CTRL_ALT | vk::M) {
        let mut work = std::mem::take(&mut state.workbench);
        work.command(state, "replay");
        state.workbench = work;
    } else if state.workbench.panel == Panel::None
        && ctx.consume_shortcut(state.config.hotkeys.view_go_to_file)
    {
        open(state, Panel::Files);
    } else {
        return;
    }
    ctx.needs_rerender();
}

pub fn initialize(state: &mut State) {
    if let Err(err) = state.documents.init_recovery() {
        state.workbench.message = format!("Recovery: {err}");
    }
    let warnings = state.documents.take_recovery_warnings();
    if !warnings.is_empty() {
        state.workbench.message = warnings.join("; ");
    }
    if !state.documents.recoverable().is_empty() {
        state.workbench.panel = Panel::Recovery;
    }
}

pub fn tick(state: &mut State) {
    if state.workbench.checkpoint_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
        state.workbench.checkpoint_at = Some(Instant::now());
        if let Err(err) = state.documents.checkpoint_recovery() {
            state.workbench.message = format!("Recovery: {err}");
        }
        let warnings = state.documents.take_recovery_warnings();
        if !warnings.is_empty() {
            state.workbench.message = warnings.join("; ");
        }
    }
    if let Some(result) = state.workbench.search.as_mut().and_then(SearchTask::try_finish) {
        state.workbench.search = None;
        match result {
            Ok(report) => {
                state.workbench.message = format!(
                    "{} matches · {} files{}{}",
                    report.matches_found,
                    report.files_searched,
                    if report.truncated { " · limited results" } else { "" },
                    if report.cancelled { " · cancelled" } else { "" }
                );
                if report.skipped_files > 0 {
                    state
                        .workbench
                        .message
                        .push_str(&format!(" · {} skipped", report.skipped_files));
                }
                if let Some(warning) = report.warnings.first() {
                    state.workbench.message.push_str(&format!(" · {warning}"));
                }
                state.workbench.report = Some(report);
            }
            Err(err) => state.workbench.message = err,
        }
    }
}

pub fn draw(ctx: &mut Context, state: &mut State) {
    let mut work = std::mem::take(&mut state.workbench);
    match work.panel {
        Panel::None => {}
        Panel::Commands | Panel::Files => work.draw_palette(ctx, state),
        Panel::Search => work.draw_search(ctx, state),
        Panel::Lens => work.draw_lens(ctx, state),
        Panel::Cell => work.draw_cell(ctx, state),
        Panel::Diff => work.draw_diff(ctx, state),
        Panel::Recovery => work.draw_recovery(ctx, state),
        Panel::MacroName => work.draw_macro_name(ctx, state),
        Panel::LogPattern => work.draw_log_pattern(ctx, state),
    }
    state.workbench = work;
}

impl Workbench {
    fn close(&mut self) {
        self.panel = Panel::None;
        self.files = None;
        self.draft = None;
        self.search = None;
        self.pending_hit = None;
        self.report = None;
        self.lens = None;
        self.lens_buffer = None;
        self.diff = String::new();
        self.diff_rows = Vec::new();
    }
    fn dimensions(ctx: &Context) -> Size {
        Size {
            width: (ctx.size().width - 4).clamp(1, 100),
            height: (ctx.size().height - 11).clamp(1, 24),
        }
    }
    fn footer(&self, ctx: &mut Context) {
        if !self.message.is_empty() {
            ctx.label("message", &display(&self.message));
            ctx.attr_overflow(Overflow::TruncateTail);
        }
    }
    fn draw_palette(&mut self, ctx: &mut Context, state: &mut State) {
        let commands = self.panel == Panel::Commands;
        let size = Self::dimensions(ctx);
        ctx.modal_begin("palette", if commands { "Commands" } else { "Files" });
        if ctx.consume_shortcut(vk::DOWN) {
            self.index = self.index.saturating_add(1);
        }
        if ctx.consume_shortcut(vk::UP) {
            self.index = self.index.saturating_sub(1);
        }
        let mut activate = ctx.consume_shortcut(vk::RETURN);
        if ctx.editline("query", &mut self.query) {
            self.index = 0;
        }
        ctx.attr_intrinsic_size(Size { width: size.width, height: 1 });
        ctx.focus_on_first_present();
        // Exact destinations remain usable even when they exceed the fuzzy
        // query budget. Preserve oversized input so it can be edited/cleared.
        let direct = (!commands)
            .then(|| direct_file_target(std::path::Path::new(&state.startup_cwd), &self.query))
            .flatten();
        let query_too_long = self.query.len() > ogedit::fuzzy::MAX_QUERY_BYTES;
        let show_query_limit = query_too_long && direct.is_none();
        let mut entries: Vec<(String, Destination)> = Vec::new();
        if commands && !query_too_long {
            for &(command, label) in COMMANDS {
                if command == "native-browse"
                    && !crate::native_dialog::NativeFilePicker::supported()
                {
                    continue;
                }
                entries.push((command_label(state, command, label), Destination::Command(command)));
            }
            if let Some(macros) = &state.macros {
                for name in macros.names() {
                    entries
                        .push((format!("Run macro: {name}"), Destination::Macro(name.to_owned())));
                }
            }
        } else if !query_too_long {
            for doc in state.documents.iter() {
                let marker = if doc.buffer.borrow().is_dirty() { "* " } else { "  " };
                let target = doc
                    .path
                    .clone()
                    .map(Destination::File)
                    .unwrap_or(Destination::Untitled(doc.new_file_counter));
                entries.push((
                    format!(
                        "{marker}{}",
                        doc.path
                            .as_ref()
                            .map(|p| relative(p, &state.startup_cwd))
                            .unwrap_or_else(|| doc.filename.clone())
                    ),
                    target,
                ));
            }
            for recent in &state.config.recent_files {
                if recent.path.is_file() && !state.documents.is_path_open(&recent.path) {
                    entries.push((
                        relative(&recent.path, &state.startup_cwd),
                        Destination::File(recent.path.clone()),
                    ));
                }
            }
        }
        if !query_too_long && !self.query.trim().is_empty() {
            let arena = ogedit::arena::scratch_arena(None);
            let mut ranked: Vec<_> = entries
                .into_iter()
                .filter_map(|item| {
                    let (score, _) = ogedit::fuzzy::score_fuzzy(&arena, &item.0, &self.query, true);
                    (score > 0).then_some((score, item))
                })
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0));
            entries = ranked.into_iter().map(|(_, item)| item).collect();
        }
        if !query_too_long && let Some(files) = &mut self.files {
            let mut seen: HashSet<PathBuf> = entries
                .iter()
                .filter_map(
                    |(_, d)| {
                        if let Destination::File(p) = d { Some(p.clone()) } else { None }
                    },
                )
                .collect();
            for path in files.results(&self.query, 500) {
                if seen.insert(path.clone()) {
                    entries.push((relative(&path, &state.startup_cwd), Destination::File(path)));
                }
            }
        }
        if let Some(path) = direct {
            let label = relative(&path, &state.startup_cwd);
            entries.retain(|(_, destination)| !matches!(destination, Destination::File(existing) if existing == &path));
            // A pasted path is an explicit destination, including files outside
            // the current workspace. Its quote characters are not fuzzy terms.
            entries.insert(0, (label, Destination::File(path)));
        }
        self.index = self.index.min(entries.len().saturating_sub(1));
        ctx.next_block_id_mixin(self.index as u64);
        ctx.list_begin("matches");
        for (index, (label, _)) in entries
            .iter()
            .enumerate()
            .skip(window_start(self.index, entries.len(), size.height as usize))
            .take(size.height as usize)
        {
            ctx.next_block_id_mixin(index as u64);
            match ctx.list_item(index == self.index, &display(label)) {
                ListSelection::Selected if ctx.was_mouse_down() => self.index = index,
                ListSelection::Activated => {
                    self.index = index;
                    activate = true;
                }
                _ => {}
            }
            ctx.attr_overflow(Overflow::TruncateMiddle);
        }
        ctx.list_end();
        if entries.is_empty() {
            ctx.label("empty", "No matches");
        }
        if let Some(files) = &self.files
            && !files.status().is_empty()
        {
            ctx.label("scan", files.status());
        }
        if show_query_limit {
            ctx.label("query-limit", "Query is too long (16 KiB maximum).");
        }
        self.footer(ctx);
        let close = ctx.modal_end();
        if close {
            self.close();
        } else if activate {
            if let Some((_, destination)) = entries.get(self.index) {
                self.activate(state, destination.clone());
            }
            ctx.needs_rerender();
        }
    }

    fn activate(&mut self, state: &mut State, destination: Destination) {
        match destination {
            Destination::File(path) => match open_file(state, &path) {
                Ok(()) => self.close(),
                Err(err) => self.message = err,
            },
            Destination::Untitled(id) => {
                state
                    .documents
                    .update_active(|doc| doc.path.is_none() && doc.new_file_counter == id);
                self.close();
            }
            Destination::Command(command) => self.command(state, command),
            Destination::Macro(name) => {
                self.replay(state, &name);
                self.close();
            }
        }
    }
    fn command(&mut self, state: &mut State, command: &str) {
        self.message.clear();
        match command {
            "new" => match state.documents.add_untitled(&state.config) {
                Ok(_) => self.close(),
                Err(err) => self.message = FormatApperr::from(err).to_string(),
            },
            "open" => {
                self.close();
                state.wants_file_picker = StateFilePicker::Open;
            }
            "native-browse" => {
                self.close();
                state.wants_native_browse = true;
            }
            "reload" => {
                self.close();
                state.wants_reload = true;
            }
            "find" | "replace" => {
                self.close();
                state.wants_search.kind = if command == "find" {
                    StateSearchKind::Search
                } else {
                    StateSearchKind::Replace
                };
                state.wants_search.focus = true;
            }
            "goto" => {
                self.close();
                state.wants_goto = true;
            }
            "close" => {
                self.close();
                state.wants_close = true;
            }
            "exit" => {
                self.close();
                state.wants_exit = true;
            }
            "files" => {
                self.close();
                self.panel = Panel::Files;
                self.query.clear();
                self.index = 0;
                match FileIndex::start(PathBuf::from(&state.startup_cwd)) {
                    Ok(index) => self.files = Some(index),
                    Err(err) => self.message = err,
                }
            }
            "search" => {
                self.close();
                self.panel = Panel::Search;
                self.index = 0;
                if self.root.is_empty() {
                    self.root = state.startup_cwd.clone();
                }
            }
            "lens" => self.open_lens(state, None),
            "csv" => self.open_lens(state, Some(LensKind::Csv)),
            "tsv" => self.open_lens(state, Some(LensKind::Tsv)),
            "json" => self.open_lens(state, Some(LensKind::Json)),
            "log" => self.open_lens(
                state,
                Some(LensKind::Log { pattern: crate::data_lens::DEFAULT_LOG_PATTERN.into() }),
            ),
            "compare" => self.compare(state),
            "recover" => {
                self.close();
                self.panel = Panel::Recovery;
            }
            "save" => {
                self.close();
                state.wants_save = true;
            }
            "saveas" => {
                self.close();
                state.wants_file_picker = StateFilePicker::SaveAs;
            }
            "record" => {
                if let Some(macros) = &mut state.macros {
                    if macros.is_recording() {
                        self.panel = Panel::MacroName;
                        self.macro_name = "macro".into();
                    } else {
                        macros.start();
                        self.close();
                    }
                } else {
                    self.message = "Macros could not be loaded.".into();
                }
            }
            "replay" => {
                let name = state.last_macro.clone();
                if let Some(name) = name {
                    self.replay(state, &name);
                    self.close();
                } else {
                    self.message = "Record a macro first.".into();
                }
            }
            "syntax" => {
                self.syntax_disabled = !self.syntax_disabled;
                self.close();
            }
            other => {
                if let Some(doc) = state.documents.active() {
                    let mut buffer = doc.buffer.borrow_mut();
                    match other {
                        "next" => {
                            buffer.add_next_occurrence();
                        }
                        "above" => {
                            buffer.add_cursor_vertical(-1);
                        }
                        "below" => {
                            buffer.add_cursor_vertical(1);
                        }
                        "bracket" => {
                            buffer.jump_to_matching_bracket();
                        }
                        "undo" => buffer.undo(),
                        "redo" => buffer.redo(),
                        "wrap" => {
                            let enabled = !buffer.is_word_wrap_enabled();
                            buffer.set_word_wrap(enabled);
                        }
                        _ => {}
                    }
                }
                self.close();
            }
        }
    }

    fn start_search(&mut self, state: &State) {
        // Cancel before capturing another set of open-buffer snapshots.
        self.search = None;
        self.report = None;
        self.pending_hit = None;
        let mut snapshots = Vec::new();
        let mut bytes = 0usize;
        let entered_root = ogedit::path::join_user_input(
            std::path::Path::new(&state.startup_cwd),
            std::path::Path::new(&self.root),
        );
        let root = crate::safe_save::resolved_path(&entered_root);
        self.root = entered_root.to_string_lossy().into_owned();
        for doc in state.documents.iter() {
            if let Some(path) = &doc.path {
                if !crate::safe_save::resolved_path(path).starts_with(&root) {
                    continue;
                }
                let buffer = doc.buffer.borrow();
                if buffer.text_length() > self.options.max_file_bytes {
                    self.search = None;
                    self.report = None;
                    self.message = format!(
                        "{} is open and exceeds the {} MiB search limit. Narrow the folder or close that file.",
                        doc.filename,
                        self.options.max_file_bytes / (1024 * 1024),
                    );
                    return;
                }
                bytes = bytes.saturating_add(buffer.text_length());
                if bytes > self.options.max_snapshot_bytes {
                    self.search = None;
                    self.report = None;
                    self.message = "Open buffers exceed the search memory limit.".into();
                    return;
                }
                snapshots.push(OpenDocumentSnapshot {
                    path: path.clone(),
                    text: buffer_text(&buffer),
                    revision: buffer.content_revision(),
                });
            }
        }
        match SearchTask::start(root, self.needle.clone(), self.options.clone(), snapshots) {
            Ok(task) => {
                self.search = Some(task);
                self.report = None;
                self.index = 0;
                self.message = "Searching…".into();
            }
            Err(err) => self.message = err,
        }
    }
    fn draw_search(&mut self, ctx: &mut Context, state: &mut State) {
        let size = Self::dimensions(ctx);
        ctx.modal_begin("workspace-search", "Search");
        let count = self.report.as_ref().map_or(0, |report| report.hits.len());
        let mut moved = false;
        if count > 0
            && matches!(ctx.keyboard_input(), Some(vk::UP | vk::DOWN | vk::PRIOR | vk::NEXT))
        {
            moved = navigate(ctx, &mut self.index, count, size.height as usize);
        }
        let mut edit = (count > 0 && ctx.consume_shortcut(vk::F2)).then_some(self.index);
        let root_changed = ctx.editline("root", &mut self.root);
        ctx.attr_intrinsic_size(Size { width: size.width, height: 1 });
        let root_focused = ctx.is_focused();
        let mut changed = ctx.editline("needle", &mut self.needle) || root_changed;
        ctx.attr_intrinsic_size(Size { width: size.width, height: 1 });
        ctx.focus_on_first_present();
        let input_focused = root_focused || ctx.is_focused();
        let mut run = input_focused && ctx.consume_shortcut(vk::RETURN);
        if !input_focused {
            moved |= navigate(ctx, &mut self.index, count, size.height as usize);
        }
        if changed {
            self.index = 0;
        }
        ctx.table_begin("options");
        ctx.table_next_row();
        changed |= ctx.checkbox("regex", ".*", &mut self.options.regex);
        changed |= ctx.checkbox("case", "Aa", &mut self.options.case_sensitive);
        changed |= ctx.checkbox("hidden", "Hidden", &mut self.options.include_hidden);
        run |= ctx.button("find", "Find", ButtonStyle::default());
        if self.search.is_some()
            && ctx.button("cancel", "Stop", ButtonStyle::default())
            && let Some(task) = &self.search
        {
            task.cancel();
        }
        ctx.table_end();
        if changed {
            self.search = None;
            self.report = None;
            self.message.clear();
        }
        let mut source = None;
        if let Some(report) = &self.report {
            self.index = self.index.min(report.hits.len().saturating_sub(1));
            ctx.next_block_id_mixin(self.index as u64);
            ctx.list_begin("rows");
            ctx.attr_intrinsic_size(size);
            ctx.inherit_focus();
            for (index, hit) in report
                .hits
                .iter()
                .enumerate()
                .skip(window_start(self.index, report.hits.len(), size.height as usize))
                .take(size.height as usize)
            {
                ctx.next_block_id_mixin(index as u64);
                let label = format!(
                    "{}:{}  {}",
                    relative(&hit.source.path, &self.root),
                    hit.line_number,
                    display_limited(hit.line_text(), size.width as usize * 4 + 32)
                );
                match ctx.list_item(index == self.index, &label) {
                    ListSelection::Selected if ctx.was_mouse_down() => self.index = index,
                    ListSelection::Activated => {
                        self.index = index;
                        edit = Some(index);
                    }
                    _ => {}
                }
                if moved && index == self.index {
                    ctx.list_item_steal_focus();
                }
                ctx.attr_overflow(Overflow::TruncateTail);
            }
            ctx.list_end();
            if !report.hits.is_empty() {
                ctx.table_begin("actions");
                ctx.table_next_row();
                if ctx.button("edit", "Edit  F2", ButtonStyle::default()) {
                    edit = Some(self.index);
                }
                if ctx.button("source", "Source", ButtonStyle::default()) {
                    source = Some(self.index);
                }
                ctx.table_end();
            } else {
                ctx.label("empty", "No matches");
            }
        }
        self.footer(ctx);
        let close = ctx.modal_end();
        if close {
            self.close();
        } else if run {
            self.start_search(state);
            ctx.needs_rerender();
        } else if let Some(index) = edit {
            let hit = self.report.as_ref().unwrap().hits[index].clone();
            let text = hit.line_text().to_owned();
            self.pending_hit = Some(hit);
            self.begin_cell(&text, Panel::Search);
            ctx.needs_rerender();
        } else if let Some(index) = source {
            let hit = &self.report.as_ref().unwrap().hits[index];
            let path = hit.source.path.clone();
            let offset = hit.matches.first().map_or(hit.line_range.start, |range| range.start);
            match open_file(state, &path) {
                Ok(()) => {
                    if let Some(doc) = state.documents.active() {
                        let mut b = doc.buffer.borrow_mut();
                        if let Err(err) = hit.prepare_line_edit(
                            &buffer_text(&b),
                            Some(b.content_revision()),
                            hit.line_text(),
                        ) {
                            self.message = err;
                            return;
                        }
                        b.cursor_move_to_offset(offset);
                        b.make_cursor_visible();
                    }
                    self.close();
                }
                Err(err) => self.message = err,
            }
        }
    }

    fn open_lens(&mut self, state: &State, kind: Option<LensKind>) {
        let Some(doc) = state.documents.active() else {
            self.message = "Open a file first.".into();
            return;
        };
        let buffer = doc.buffer.borrow();
        if buffer.text_length() > 8 * 1024 * 1024 {
            self.message =
                "Data views are limited to 8 MiB. Use the source editor for larger files.".into();
            return;
        }
        let kind = kind.unwrap_or_else(|| LensKind::infer(doc.path.as_deref()));
        match DataLens::parse(&buffer_text(&buffer), buffer.content_revision(), kind) {
            Ok(lens) => {
                self.close();
                self.message = lens.warnings.join("; ");
                self.column = lens
                    .rows
                    .first()
                    .and_then(|row| row.cells.iter().position(|cell| cell.editable()))
                    .unwrap_or(0);
                self.lens = Some(lens);
                self.lens_buffer = Some(doc.buffer.clone());
                self.panel = Panel::Lens;
                self.index = 0;
            }
            Err(err) => {
                self.message = err;
                if self.panel == Panel::None {
                    self.panel = Panel::Commands;
                }
            }
        }
    }
    fn draw_lens(&mut self, ctx: &mut Context, state: &mut State) {
        let size = Self::dimensions(ctx);
        let mut edit = false;
        let mut refresh = false;
        let Some(lens) = &self.lens else {
            self.close();
            return;
        };
        let title = lens.kind.name();
        ctx.modal_begin("data-view", title);
        let mut moved = navigate(ctx, &mut self.index, lens.rows.len(), size.height as usize);
        edit |= ctx.consume_shortcut(vk::F2);
        if ctx.consume_shortcut(vk::LEFT) {
            self.column = self.column.saturating_sub(1);
            moved = true;
        }
        if ctx.consume_shortcut(vk::RIGHT) {
            self.column = self.column.saturating_add(1).min(lens.columns.len().saturating_sub(1));
            moved = true;
        }
        self.index = self.index.min(lens.rows.len().saturating_sub(1));
        let visible_columns = ((size.width - 8) / 12).clamp(1, 6) as usize;
        let visible_columns = visible_columns.min(lens.columns.len().max(1));
        let col_width = ((size.width - 8) / visible_columns as isize - 3).max(1) as usize;
        let first_column = self.column.saturating_sub(visible_columns - 1);
        ctx.label(
            "columns",
            &lens
                .columns
                .iter()
                .enumerate()
                .skip(first_column)
                .take(visible_columns)
                .map(|(i, c)| {
                    format!(
                        "{}{}",
                        if i == self.column { "▸" } else { " " },
                        clip(c, col_width - 1)
                    )
                })
                .collect::<Vec<_>>()
                .join(" │ "),
        );
        ctx.attr_overflow(Overflow::TruncateTail);
        ctx.next_block_id_mixin(self.index as u64);
        ctx.list_begin("rows");
        ctx.attr_intrinsic_size(size);
        ctx.inherit_focus();
        for (index, row) in lens
            .rows
            .iter()
            .enumerate()
            .skip(window_start(self.index, lens.rows.len(), size.height as usize))
            .take(size.height as usize)
        {
            ctx.next_block_id_mixin(index as u64);
            let text = format!(
                "{:>4}  {}",
                row.label,
                row.cells
                    .iter()
                    .skip(first_column)
                    .take(visible_columns)
                    .map(|cell| clip(&display_limited(&cell.value, col_width * 4 + 32), col_width))
                    .collect::<Vec<_>>()
                    .join(" │ ")
            );
            match ctx.list_item(index == self.index, &text) {
                ListSelection::Selected if ctx.was_mouse_down() => self.index = index,
                ListSelection::Activated => {
                    self.index = index;
                    edit = true;
                }
                _ => {}
            }
            if moved && index == self.index {
                ctx.list_item_steal_focus();
            }
            ctx.attr_overflow(Overflow::TruncateTail);
        }
        ctx.list_end();
        ctx.table_begin("actions");
        ctx.table_next_row();
        edit |= ctx.button("edit", "Edit  F2", ButtonStyle::default());
        refresh |= ctx.button("refresh", "Refresh", ButtonStyle::default());
        let source = ctx.button("source", "Source", ButtonStyle::default());
        let pattern = matches!(lens.kind, LensKind::Log { .. })
            && ctx.button("pattern", "Fields…", ButtonStyle::default());
        ctx.table_end();
        self.footer(ctx);
        let close = ctx.modal_end();
        if close || source {
            self.close();
        } else if pattern {
            if let Some(DataLens { kind: LensKind::Log { pattern }, .. }) = &self.lens {
                self.log_pattern = pattern.clone();
                self.message.clear();
                self.panel = Panel::LogPattern;
            }
        } else if refresh {
            let kind = self.lens.as_ref().unwrap().kind.clone();
            self.open_lens(state, Some(kind));
        } else if edit
            && let Some(cell) = self
                .lens
                .as_ref()
                .and_then(|l| l.rows.get(self.index))
                .and_then(|r| r.cells.get(self.column))
        {
            if cell.editable() {
                let value = cell.value.clone();
                self.begin_cell(&value, Panel::Lens);
            } else {
                self.message = "This cell is read-only.".into();
            }
        }
    }

    fn begin_cell(&mut self, text: &str, return_panel: Panel) {
        let create = || -> Result<RcTextBuffer, String> {
            let mut loaded =
                TextBuffer::new(false).map_err(|err| FormatApperr::from(err).to_string())?;
            if let Err(err) = loaded.apply_byte_edits(&[(0..0, text.as_bytes().to_vec())]) {
                return Err(err.into());
            }
            let buffer =
                TextBuffer::new_rc(false).map_err(|err| FormatApperr::from(err).to_string())?;
            buffer.borrow_mut().replace_loaded_content(loaded);
            Ok(buffer)
        };
        match create() {
            Ok(buffer) => {
                if buffer.borrow().text_length() != text.len() {
                    self.message = "The cell is too large to edit.".into();
                    return;
                }
                self.draft = Some(buffer);
                self.return_panel = return_panel;
                self.panel = Panel::Cell;
            }
            Err(err) => self.message = err,
        }
    }
    fn draw_cell(&mut self, ctx: &mut Context, state: &mut State) {
        ctx.modal_begin("cell", "Edit");
        let size = Self::dimensions(ctx);
        // A multiline textarea consumes Escape for selection clearing. Capture
        // the modal's dismissal before delegating input to that textarea.
        let mut cancel = ctx.consume_shortcut(vk::ESCAPE);
        let mut apply = ctx.consume_shortcut(kbmod::CTRL | vk::RETURN);
        if let Some(draft) = &self.draft {
            ctx.textarea("value", draft.clone());
            ctx.attr_intrinsic_size(Size { width: size.width, height: 6.min(size.height) });
            ctx.focus_on_first_present();
        }
        ctx.table_begin("actions");
        ctx.table_next_row();
        apply |= ctx.button("apply", "Apply  Ctrl+Enter", ButtonStyle::default());
        cancel |= ctx.button("cancel", "Cancel", ButtonStyle::default());
        ctx.table_end();
        self.footer(ctx);
        let close = ctx.modal_end();
        if close || cancel {
            self.panel = self.return_panel;
            self.draft = None;
            self.pending_hit = None;
        } else if apply {
            match self.apply_cell(state) {
                Ok(()) => {
                    self.panel = self.return_panel;
                    self.draft = None;
                    self.pending_hit = None;
                    self.message = "Applied · Ctrl+Z in source to undo".into();
                    if self.return_panel == Panel::Search {
                        self.start_search(state);
                    }
                }
                Err(err) => self.message = err,
            }
            ctx.needs_rerender();
        }
    }
    fn apply_cell(&mut self, state: &mut State) -> Result<(), String> {
        let value = buffer_text(&self.draft.as_ref().ok_or("No cell selected.")?.borrow());
        if self.return_panel == Panel::Search {
            let hit = self.pending_hit.clone().ok_or("No search result selected.")?;
            open_file(state, &hit.source.path)?;
            let doc = state.documents.active().ok_or("The file could not be opened.")?;
            let mut buffer = doc.buffer.borrow_mut();
            let current = buffer_text(&buffer);
            let edit = hit.prepare_line_edit(&current, Some(buffer.content_revision()), &value)?;
            buffer
                .apply_byte_edits(&[(edit.range, edit.replacement.into_bytes())])
                .map_err(str::to_owned)?;
        } else {
            let lens = self.lens.as_mut().ok_or("No view selected.")?;
            let source_buffer = self.lens_buffer.as_ref().ok_or("The source was closed.")?;
            if !state.documents.iter().any(|doc| std::rc::Rc::ptr_eq(&doc.buffer, source_buffer)) {
                return Err("The source was closed. Open the file and its data view again.".into());
            }
            let mut buffer = source_buffer.borrow_mut();
            let current = buffer_text(&buffer);
            let edit = lens.prepare_edit(
                &current,
                buffer.content_revision(),
                self.index,
                self.column,
                &value,
            )?;
            buffer
                .apply_byte_edits(&[(edit.range, edit.replacement.into_bytes())])
                .map_err(str::to_owned)?;
            lens.refresh(&buffer_text(&buffer), buffer.content_revision())?;
        }
        Ok(())
    }

    pub fn compare(&mut self, state: &State) {
        if let Some(doc) = state.documents.active() {
            match doc.external_diff() {
                Ok(diff) => {
                    self.close();
                    self.diff_rows = line_ranges(&diff);
                    self.diff_index = 0;
                    self.diff = diff;
                    self.panel = Panel::Diff;
                }
                Err(err) => self.message = FormatApperr::from(err).to_string(),
            }
        }
    }
    fn draw_diff(&mut self, ctx: &mut Context, state: &mut State) {
        ctx.modal_begin("disk-diff", "Disk ↔ Buffer");
        let size = Self::dimensions(ctx);
        let scroll_positions = self.diff_rows.len().saturating_sub(size.height as usize) + 1;
        navigate(ctx, &mut self.diff_index, scroll_positions, size.height as usize);
        let start = self.diff_index;
        ctx.block_begin("changes");
        ctx.attr_intrinsic_size(size);
        if self.diff.is_empty() {
            ctx.label("same", "No differences");
        }
        for (i, range) in self.diff_rows.iter().enumerate().skip(start).take(size.height as usize) {
            ctx.next_block_id_mixin(i as u64);
            ctx.label(
                "line",
                &display_limited(&self.diff[range.clone()], size.width as usize * 4 + 32),
            );
            ctx.attr_overflow(Overflow::TruncateTail);
        }
        ctx.block_end();
        if self.diff_rows.len() > size.height as usize {
            ctx.label(
                "position",
                &format!(
                    "{}–{} / {}",
                    start + 1,
                    (start + size.height as usize).min(self.diff_rows.len()),
                    self.diff_rows.len()
                ),
            );
        }
        ctx.table_begin("actions");
        ctx.table_next_row();
        if ctx.button("refresh", "Refresh", ButtonStyle::default()) {
            self.compare(state);
        }
        let reload = ctx.button("reload", "Reload disk…", ButtonStyle::default());
        let saveas = ctx.button("saveas", "Save as…", ButtonStyle::default());
        ctx.table_end();
        self.footer(ctx);
        if ctx.modal_end() {
            self.close();
        }
        if reload {
            self.close();
            state.wants_reload = true;
        }
        if saveas {
            self.close();
            state.wants_file_picker = StateFilePicker::SaveAs;
        }
    }
    fn draw_recovery(&mut self, ctx: &mut Context, state: &mut State) {
        ctx.modal_begin("recovery", "Recovery");
        let size = Self::dimensions(ctx);
        let mut restore = None;
        let mut discard = false;
        let count = state.documents.recoverable().len();
        let moved = navigate(ctx, &mut self.index, count, size.height as usize);
        ctx.next_block_id_mixin(self.index as u64);
        ctx.list_begin("list");
        ctx.attr_intrinsic_size(size);
        ctx.inherit_focus();
        for (index, entry) in state
            .documents
            .recoverable()
            .iter()
            .enumerate()
            .skip(window_start(self.index, count, size.height as usize))
            .take(size.height as usize)
        {
            ctx.next_block_id_mixin(index as u64);
            let label = format!(
                "{}  {}",
                entry.metadata.filename,
                entry
                    .metadata
                    .original_path
                    .as_ref()
                    .map(|path| path.to_string_lossy())
                    .unwrap_or_default()
            );
            match ctx.list_item(index == self.index, &display(&label)) {
                ListSelection::Selected if ctx.was_mouse_down() => self.index = index,
                ListSelection::Activated => restore = Some(index),
                _ => {}
            }
            if index == self.index {
                // Recovery can be the first modal at startup, with no prior
                // input control to delegate focus from. Make Enter work then.
                ctx.focus_on_first_present();
                if moved {
                    ctx.list_item_steal_focus();
                }
            }
            ctx.attr_overflow(Overflow::TruncateMiddle);
        }
        ctx.list_end();
        if state.documents.recoverable().is_empty() {
            ctx.label("empty", "No unsaved recovery copies");
        } else {
            ctx.table_begin("actions");
            ctx.table_next_row();
            if ctx.button("restore", "Restore copy", ButtonStyle::default()) {
                restore = Some(self.index);
            }
            discard = ctx.button("discard", "Discard", ButtonStyle::default());
            ctx.table_end();
        }
        self.footer(ctx);
        if ctx.modal_end() {
            self.close();
        }
        if let Some(index) = restore {
            match state.documents.restore_recovery(index, &state.config) {
                Ok(_) => self.close(),
                Err(err) => self.message = FormatApperr::from(err).to_string(),
            }
        }
        if discard {
            if let Err(err) = state.documents.discard_recovery(self.index) {
                self.message = err.to_string();
            }
            self.index = self.index.saturating_sub(1);
        }
    }
    fn draw_macro_name(&mut self, ctx: &mut Context, state: &mut State) {
        ctx.modal_begin("macro-name", "Save macro");
        ctx.editline("name", &mut self.macro_name);
        ctx.attr_intrinsic_size(Size { width: 40.min(Self::dimensions(ctx).width), height: 1 });
        ctx.focus_on_first_present();
        let save =
            ctx.consume_shortcut(vk::RETURN) || ctx.button("save", "Save", ButtonStyle::default());
        self.footer(ctx);
        if ctx.modal_end() {
            if let Some(macros) = &mut state.macros {
                macros.cancel();
            }
            self.close();
        } else if save && let Some(macros) = &mut state.macros {
            match macros.stop_and_save(&self.macro_name) {
                Ok(()) => {
                    state.last_macro = Some(self.macro_name.clone());
                    self.close();
                }
                Err(err) => self.message = err,
            }
        }
    }
    fn draw_log_pattern(&mut self, ctx: &mut Context, state: &mut State) {
        ctx.modal_begin("log-pattern", "Log fields");
        ctx.editline("pattern", &mut self.log_pattern);
        ctx.attr_intrinsic_size(Size { width: Self::dimensions(ctx).width, height: 1 });
        ctx.focus_on_first_present();
        let apply = ctx.consume_shortcut(vk::RETURN)
            || ctx.button("apply", "Apply", ButtonStyle::default());
        self.footer(ctx);
        if ctx.modal_end() {
            self.panel = Panel::Lens;
        } else if apply {
            self.open_lens(state, Some(LensKind::Log { pattern: self.log_pattern.clone() }));
        }
    }
    fn replay(&mut self, state: &mut State, name: &str) {
        if let (Some(macros), Some(doc)) = (&mut state.macros, state.documents.active()) {
            if let Err(err) = macros.replay(name, &mut doc.buffer.borrow_mut()) {
                self.message = err;
            } else {
                state.last_macro = Some(name.to_owned());
            }
        }
    }
}

pub fn buffer_text(buffer: &TextBuffer) -> String {
    String::from_utf8_lossy(&buffer.extract_bytes(0..buffer.text_length())).into_owned()
}
fn open_file(state: &mut State, path: &std::path::Path) -> Result<(), String> {
    if !state.documents.activate_path(path) {
        state
            .documents
            .add_file_path(path, &state.config)
            .map_err(|err| FormatApperr::from(err).to_string())?;
    }
    if let Some(path) = state.documents.active().and_then(|doc| doc.path.as_ref()) {
        state.file_watcher.watch(path);
        state.config.add_recent_file(path);
    }
    Ok(())
}
fn relative(path: &std::path::Path, root: &str) -> String {
    // Presentation must not canonicalize hundreds of filesystem paths per
    // frame. Strip the Windows verbatim prefix without touching the disk.
    let visible = |value: &std::path::Path| {
        let value = value.to_string_lossy();
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
        }
    };
    let path = visible(path);
    let root = visible(std::path::Path::new(root));
    std::path::Path::new(&path)
        .strip_prefix(root)
        .unwrap_or(std::path::Path::new(&path))
        .to_string_lossy()
        .into_owned()
}

fn direct_file_target(root: &std::path::Path, query: &str) -> Option<PathBuf> {
    let input = std::path::Path::new(query);
    let looks_like_path = input.is_absolute()
        || query.starts_with('"')
        || query.contains('/')
        || cfg!(windows) && (query.contains('\\') || query.as_bytes().get(1) == Some(&b':'));
    if !looks_like_path {
        return None;
    }
    let path = ogedit::path::join_user_input(root, input);
    let (file, _) = crate::documents::DocumentManager::resolve_filename_goto(&path);
    (path.is_file() || file.is_file()).then_some(path)
}

fn command_label(state: &State, command: &str, fallback: &str) -> String {
    let keys = &state.config.hotkeys;
    let key = match command {
        "new" => keys.file_new,
        "open" => keys.file_open,
        "reload" => keys.file_reload,
        "find" => keys.edit_find,
        "replace" => keys.edit_replace,
        "goto" => keys.view_go_to_line,
        "close" => keys.file_close,
        "exit" => keys.file_exit,
        "files" => keys.view_go_to_file,
        "save" => keys.file_save,
        "saveas" => keys.file_save_as,
        "wrap" => keys.view_word_wrap,
        "undo" => keys.edit_undo,
        "redo" => keys.edit_redo,
        _ => return fallback.to_owned(),
    };
    let label = fallback.split("  ").next().unwrap_or(fallback);
    format!("{label:<28} {}", crate::config::Hotkeys::hotkey_to_string(key))
}

fn window_start(index: usize, count: usize, height: usize) -> usize {
    index.saturating_sub(height / 2).min(count.saturating_sub(height))
}

fn line_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut offset = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            let content = line.strip_suffix('\n').unwrap_or(line);
            let content = content.strip_suffix('\r').unwrap_or(content);
            start..start + content.len()
        })
        .collect()
}

fn navigate(ctx: &mut Context, index: &mut usize, count: usize, height: usize) -> bool {
    let previous = *index;
    if ctx.consume_shortcut(vk::DOWN) {
        *index = index.saturating_add(1);
    } else if ctx.consume_shortcut(vk::UP) {
        *index = index.saturating_sub(1);
    } else if ctx.consume_shortcut(vk::NEXT) {
        *index = index.saturating_add(height);
    } else if ctx.consume_shortcut(vk::PRIOR) {
        *index = index.saturating_sub(height);
    } else if ctx.consume_shortcut(vk::HOME) {
        *index = 0;
    } else if ctx.consume_shortcut(vk::END) {
        *index = count.saturating_sub(1);
    }
    *index = (*index).min(count.saturating_sub(1));
    if previous != *index {
        ctx.needs_rerender();
    }
    previous != *index
}
fn display(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => '↵',
            '\r' => ' ',
            '\t' => '→',
            c if c.is_control() => '�',
            c => c,
        })
        .collect()
}
fn clip(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let bytes = text.as_bytes();
    let measure = |columns: usize| {
        ogedit::unicode::MeasurementConfig::new(&bytes)
            .goto_visual(Point { x: columns as isize, y: 0 })
    };
    let end = measure(width);
    if end.offset == text.len() && end.visual_pos.x <= width as isize {
        return format!("{text}{}", " ".repeat(width.saturating_sub(end.visual_pos.x as usize)));
    }
    let mut end = measure(width - 1);
    if end.visual_pos.x > (width - 1) as isize {
        end = measure(width.saturating_sub(2));
    }
    let mut clipped = text[..end.offset].to_owned();
    clipped.push('…');
    clipped.push_str(&" ".repeat(width.saturating_sub(end.visual_pos.x as usize + 1)));
    clipped
}

fn display_limited(text: &str, limit: usize) -> String {
    display(&text.chars().take(limit).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_panels_releases_document_drafts_search_snapshots_and_diff_capacity() {
        let _runtime = crate::test_runtime::lock();
        let buffer = TextBuffer::new_rc(true).unwrap();
        let source = std::sync::Arc::new(crate::workspace_search::SourceSnapshot {
            path: PathBuf::from("temporary.txt"),
            text: "match".into(),
            revision: None,
        });
        let weak_source = std::sync::Arc::downgrade(&source);
        let weak_buffer = std::rc::Rc::downgrade(&buffer);
        let hit = SearchHit {
            source,
            line_number: 1,
            line_range: 0..5,
            matches: std::iter::once(0..5).collect(),
        };
        let mut work = Workbench {
            panel: Panel::Cell,
            lens_buffer: Some(buffer),
            pending_hit: Some(hit.clone()),
            report: Some(SearchReport { hits: vec![hit], ..Default::default() }),
            diff: "large diff\n".repeat(10_000),
            diff_rows: vec![0..10; 10_000],
            ..Default::default()
        };
        work.begin_cell("temporary draft", Panel::Lens);
        let weak_draft = std::rc::Rc::downgrade(work.draft.as_ref().unwrap());
        work.close();
        assert!(weak_buffer.upgrade().is_none());
        assert!(weak_source.upgrade().is_none());
        assert!(weak_draft.upgrade().is_none());
        assert_eq!(work.diff.capacity(), 0);
        assert_eq!(work.diff_rows.capacity(), 0);
    }

    #[test]
    fn virtual_windows_keep_selected_rows_visible_at_both_ends() {
        assert_eq!(window_start(0, 20_000, 24), 0);
        assert_eq!(window_start(19_999, 20_000, 24), 19_976);
        assert_eq!(window_start(3, 4, 24), 0);
        for selected in [0, 1, 12, 500, 19_999] {
            let start = window_start(selected, 20_000, 24);
            assert!((start..start + 24).contains(&selected));
        }
    }

    #[test]
    fn pasted_quoted_palette_path_can_open_outside_the_workspace() {
        let root = std::env::temp_dir().join(format!("ogedit-palette-path-{}", std::process::id()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let file = root.join("outside file.txt");
        std::fs::write(&file, "data").unwrap();
        let query = format!("\"{}\"", file.to_string_lossy());
        assert_eq!(direct_file_target(&workspace, &query), Some(file.clone()));
        assert_eq!(direct_file_target(&workspace, "\"../outside file.txt\""), Some(file.clone()));
        assert!(direct_file_target(&workspace, "\"../missing file.txt\"").is_none());
        let query = format!("\"{}\":2:3", file.to_string_lossy());
        assert!(direct_file_target(&workspace, &query).is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diff_line_index_keeps_utf8_byte_ranges_and_empty_lines() {
        let text = " café\r\n\n+שלום\n";
        let ranges = line_ranges(text);
        assert_eq!(
            ranges.iter().map(|range| &text[range.clone()]).collect::<Vec<_>>(),
            [" café", "", "+שלום"]
        );
        assert!(line_ranges("").is_empty());
        assert_eq!(line_ranges("last"), vec![0..4]);
    }

    #[test]
    fn data_columns_respect_graphemes_and_terminal_width() {
        let _runtime = crate::test_runtime::lock();
        assert_eq!(clip("東京hello", 4), "東… ");
        assert_eq!(clip("e\u{301}cole", 4), "e\u{301}co…");
        assert_eq!(clip("東京", 5), "東京 ");
    }

    #[test]
    fn cell_initial_value_is_multiline_and_not_an_undo_step() {
        let _runtime = crate::test_runtime::lock();
        let mut work = Workbench::default();
        let original = "café\r\nשלום";
        work.begin_cell(original, Panel::Lens);
        let draft = work.draft.as_ref().unwrap();
        draft.borrow_mut().undo();
        assert_eq!(buffer_text(&draft.borrow()), original);
    }

    #[test]
    #[cfg(windows)]
    fn windows_verbatim_paths_are_compact_in_results() {
        assert_eq!(
            relative(
                std::path::Path::new(r"\\?\K:\source\ogedit\src\main.rs"),
                r"K:\source\ogedit"
            ),
            r"src\main.rs"
        );
        assert_eq!(
            relative(std::path::Path::new(r"\\?\UNC\server\share\a.txt"), r"\\server\share"),
            "a.txt"
        );
    }
}
