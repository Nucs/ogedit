// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![feature(allocator_api)]

mod config;
mod data_lens;
mod documents;
mod draw_editor;
mod draw_filepicker;
mod draw_menubar;
mod draw_statusbar;
mod localization;
#[allow(dead_code)]
mod logging;
mod macros;
mod native_dialog;
mod native_open;
mod palette;
mod recovery;
mod safe_save;
mod settings;
mod shortcuts;
mod state;
#[cfg(test)]
mod test_runtime;
mod watch;
mod workbench;
mod workspace_search;

#[cfg(feature = "debug-latency")]
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, process};

use draw_editor::*;
use draw_filepicker::*;
use draw_menubar::*;
use draw_statusbar::*;
use localization::*;
use ogedit::arena::{self, Arena, ArenaString, scratch_arena};
use ogedit::framebuffer::{self, IndexedColor};
use ogedit::helpers::{CoordType, KIBI, MEBI, MetricFormatter, Point, Rect, Size};
use ogedit::oklab::StraightRgba;
use ogedit::tui::*;
use ogedit::vt::{self, Token};
use ogedit::{apperr, arena_format, base64, input, path, sys, unicode};
use state::{log_periodic_content_snapshot, log_state_changes, *};

#[cfg(target_pointer_width = "32")]
const SCRATCH_ARENA_CAPACITY: usize = 128 * MEBI;
#[cfg(target_pointer_width = "64")]
const SCRATCH_ARENA_CAPACITY: usize = 512 * MEBI;

fn main() -> process::ExitCode {
    if cfg!(debug_assertions) {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            drop(RestoreModes);
            drop(sys::Deinit);
            hook(info);
        }));
    }

    match run() {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(err) => {
            sys::write_stdout(&format!("{}\n", FormatApperr::from(err)));
            process::ExitCode::FAILURE
        }
    }
}

fn run() -> apperr::Result<()> {
    // Init logging FIRST so panics during other init are captured.
    // This only uses std library functions, no dependencies on sys/arena/loc.
    logging::init();
    logging::log_app_start();

    // Init `sys` first, as everything else may depend on its functionality (IO, function pointers, etc.).
    let _sys_deinit = sys::init();
    logging::log_action("INIT: sys initialized");

    // Next init `arena`, so that `scratch_arena` works. `loc` depends on it.
    arena::init(SCRATCH_ARENA_CAPACITY)?;
    logging::log_action("INIT: arena initialized");

    // Init the `loc` module, so that error messages are localized.
    localization::init();
    logging::log_action("INIT: localization initialized");

    let mut state = State::new()?;
    if handle_args(&mut state)? {
        return Ok(());
    }
    handle_stdin(&mut state)?;
    workbench::initialize(&mut state);

    // This will reopen stdin if it's redirected (which may fail) and switch
    // the terminal to raw mode which prevents the user from pressing Ctrl+C.
    // `handle_args` may want to print a help message (must not fail),
    // and reads files (may hang; should be cancelable with Ctrl+C).
    // As such, we call this after `handle_args`.
    logging::log_action("INIT: Switching terminal to raw mode");
    sys::switch_modes()?;

    let mut vt_parser = vt::Parser::new();
    let mut input_parser = input::Parser::new();
    let mut tui = Tui::new()?;
    if let Ok(size) = sys::get_window_size() {
        tui.set_size(size);
    }

    logging::log_action("INIT: Terminal setup complete");
    let _restore = setup_terminal(&mut tui, &mut state, &mut vt_parser);

    state.menubar_color_bg = tui.indexed(IndexedColor::Background).oklab_blend(tui.indexed_alpha(
        IndexedColor::BrightBlue,
        1,
        2,
    ));
    state.menubar_color_fg = tui.contrasted(state.menubar_color_bg);
    let floater_bg = tui
        .indexed_alpha(IndexedColor::Background, 2, 3)
        .oklab_blend(tui.indexed_alpha(IndexedColor::Foreground, 1, 3));
    let floater_fg = tui.contrasted(floater_bg);
    tui.setup_modifier_translations(ModifierTranslations {
        ctrl: loc(LocId::Ctrl),
        alt: loc(LocId::Alt),
        shift: loc(LocId::Shift),
    });
    tui.set_floater_default_bg(floater_bg);
    tui.set_floater_default_fg(floater_fg);
    tui.set_modal_default_bg(floater_bg);
    tui.set_modal_default_fg(floater_fg);

    #[cfg(feature = "debug-latency")]
    let mut last_latency_width = 0;

    // Track start time for periodic content snapshots
    let start_time = std::time::Instant::now();

    loop {
        #[cfg(feature = "debug-latency")]
        let time_beg;
        #[cfg(feature = "debug-latency")]
        let mut passes;

        // Process a batch of input.
        {
            let scratch = scratch_arena(None);
            let read_timeout =
                vt_parser.read_timeout().min(tui.read_timeout()).min(Duration::from_millis(100));
            let Some((resize, input)) = sys::read_stdin(&scratch, read_timeout) else {
                break;
            };

            #[cfg(feature = "debug-latency")]
            {
                time_beg = std::time::Instant::now();
                passes = 0usize;
            }

            let vt_iter = vt_parser.parse(&input);
            let mut input_iter =
                resize.map(input::Input::Resize).into_iter().chain(input_parser.parse(vt_iter));

            while {
                let mut input = input_iter.next();
                let more = input.is_some();

                // Log input events
                if let Some(ref inp) = input {
                    match inp {
                        input::Input::Text(text) => {
                            logging::log_text_input(text);
                        }
                        input::Input::Paste(data) => {
                            logging::log_paste(data);
                        }
                        input::Input::Resize(size) => {
                            logging::log_action(&format!(
                                "TERMINAL_RESIZE: {}x{}",
                                size.width, size.height
                            ));
                        }
                        input::Input::Mouse(mouse) => {
                            use input::InputMouseState;

                            use crate::state::ClickTarget;
                            let button = match mouse.state {
                                InputMouseState::Left => Some("left"),
                                InputMouseState::Right => Some("right"),
                                InputMouseState::Middle => Some("middle"),
                                _ => None,
                            };
                            if let Some(btn) = button {
                                // Determine click target based on position and UI state
                                let size = tui.size();
                                let target = if state.wants_about
                                    || state.wants_goto
                                    || state.wants_encoding_picker
                                    || state.wants_language_picker
                                    || state.wants_indentation_picker
                                {
                                    ClickTarget::Dialog
                                } else if !matches!(state.wants_file_picker, StateFilePicker::None)
                                {
                                    ClickTarget::FilePicker
                                } else if mouse.position.y == 0 {
                                    ClickTarget::Menubar
                                } else if mouse.position.y >= size.height - 1 {
                                    ClickTarget::Statusbar
                                } else {
                                    ClickTarget::Editor
                                };
                                logging::log_mouse_click(mouse.position, btn, target.as_str());
                                state.logging_tracker.last_input_was_click = true;
                                state.logging_tracker.last_click_target = target;
                            }
                        }
                        _ => {}
                    }
                }

                // Capture logical editor commands before dispatch changes the clipboard.
                if state.workbench.panel == workbench::Panel::None
                    && matches!(
                        &input,
                        Some(
                            input::Input::Text(_)
                                | input::Input::Paste(_)
                                | input::Input::Keyboard(_)
                        )
                    )
                {
                    state.workbench.message.clear();
                }
                let recording_editor = state.editor_focused
                    && state.workbench.panel == workbench::Panel::None
                    && state.macros.as_ref().is_some_and(|store| store.is_recording());
                let recordable_event =
                    input.as_ref().is_some_and(|event| !matches!(event, input::Input::Resize(_)));
                let macro_candidate = if recording_editor {
                    input.as_ref().and_then(|event| {
                        shortcuts::macro_command(&state.config.hotkeys, event, tui.clipboard_ref())
                    })
                } else {
                    None
                };
                let macro_document = state.documents.active().map(|doc| doc.buffer.clone());
                let stop_recording_key = matches!(&input, Some(input::Input::Keyboard(key))
                    if *key == (input::kbmod::CTRL_SHIFT | input::vk::M) || *key == input::vk::F4);
                let stopped_before_input = recording_editor
                    && recordable_event
                    && macro_candidate.is_none()
                    && !stop_recording_key;
                if stopped_before_input {
                    input = None;
                    state.workbench.panel = workbench::Panel::MacroName;
                    state.workbench.message = "Recording stopped before this action. Save or cancel, then repeat the action.".into();
                }

                let mut ctx = tui.create_context(input);

                workbench::tick(&mut state);
                draw(&mut ctx, &mut state);

                if recording_editor
                    && recordable_event
                    && !stop_recording_key
                    && !stopped_before_input
                {
                    let same_document = macro_document
                        .as_ref()
                        .zip(state.documents.active())
                        .is_some_and(|(before, after)| std::rc::Rc::ptr_eq(before, &after.buffer));
                    if state.editor_focused
                        && state.workbench.panel == workbench::Panel::None
                        && same_document
                    {
                        if let Some(command) = macro_candidate {
                            if let Some(store) = &mut state.macros {
                                store.record(command);
                            }
                        } else {
                            state.workbench.panel = workbench::Panel::MacroName;
                            state.workbench.message = "Recording stopped before an unsupported action. Save the recorded commands or cancel.".into();
                            ctx.needs_rerender();
                        }
                    } else if state.macros.as_ref().is_some_and(|store| store.is_recording()) {
                        state.workbench.panel = workbench::Panel::MacroName;
                        state.workbench.message = "Recording stopped when the editor lost focus. Save the recorded commands or cancel.".into();
                        ctx.needs_rerender();
                    }
                }

                #[cfg(feature = "debug-latency")]
                {
                    passes += 1;
                }

                more
            } {}
        }

        // Continue rendering until the layout has settled.
        // This can take >1 frame, if the input focus is tossed between different controls.
        while tui.needs_settling() {
            let mut ctx = tui.create_context(None);

            draw(&mut ctx, &mut state);

            #[cfg(feature = "debug-latency")]
            {
                passes += 1;
            }
        }

        if state.exit {
            logging::log_app_exit();
            break;
        }

        // Log state changes (cursor movement, etc.)
        log_state_changes(&mut state);

        // Log periodic content snapshot (1 second after last change)
        let elapsed_ms = start_time.elapsed().as_millis() as u64;
        log_periodic_content_snapshot(&mut state, elapsed_ms);

        // Poll file watcher for external modifications
        for event in state.file_watcher.poll_events() {
            state.documents.record_watch_event(&event);
            match event {
                watch::WatchEvent::Modified(path) => {
                    // Check if this path matches the active document
                    if let Some(doc) = state.documents.active()
                        && doc.path.as_ref() == Some(&path)
                    {
                        state.file_changed_cached = true;
                        logging::log_action(&format!(
                            "FILE_WATCH_EVENT: Modified {}",
                            path.display()
                        ));
                    }
                }
                watch::WatchEvent::Deleted(path) => {
                    logging::log_action(&format!("FILE_WATCH_EVENT: Deleted {}", path.display()));
                }
            }
        }
        state.file_changed_cached =
            state.documents.active().is_some_and(|doc| doc.has_file_changed_on_disk());

        // Render the UI and write it to the terminal.
        {
            let scratch = scratch_arena(None);
            let mut output = tui.render(&scratch);

            write_terminal_title(&scratch, &mut output, &mut state);

            if state.osc_clipboard_sync {
                write_osc_clipboard(&mut tui, &mut state, &mut output);
            }

            #[cfg(feature = "debug-latency")]
            {
                // Print the number of passes and latency in the top right corner.
                let time_end = std::time::Instant::now();
                let status = time_end - time_beg;

                let scratch_alt = scratch_arena(Some(&scratch));
                let status = arena_format!(
                    &scratch_alt,
                    "{}P {}B {:.3}μs",
                    passes,
                    output.len(),
                    status.as_nanos() as f64 / 1000.0
                );

                // "μs" is 3 bytes and 2 columns.
                let cols = status.len() as CoordType - 3 + 2;

                // Since the status may shrink and grow, we may have to overwrite the previous one with whitespace.
                let padding = (last_latency_width - cols).max(0);

                // If the `output` is already very large,
                // Rust may double the size during the write below.
                // Let's avoid that by reserving the needed size in advance.
                output.reserve_exact(128);

                // To avoid moving the cursor, push and pop it onto the VT cursor stack.
                _ = write!(
                    output,
                    "\x1b7\x1b[0;41;97m\x1b[1;{0}H{1:2$}{3}\x1b8",
                    tui.size().width - cols - padding + 1,
                    "",
                    padding as usize,
                    status
                );

                last_latency_width = cols;
            }

            sys::write_stdout(&output);
        }
    }

    Ok(())
}

// Returns true if the application should exit early.
fn handle_args(state: &mut State) -> apperr::Result<bool> {
    let scratch = scratch_arena(None);
    let mut paths: Vec<(PathBuf, Option<Point>), &Arena> = Vec::new_in(&*scratch);
    let cwd = env::current_dir()?;
    let mut dir = None;
    let mut parse_args = true;
    let mut goto_next = false;

    // The best CLI argument parser in the world.
    for arg in env::args_os().skip(1) {
        if parse_args {
            if arg == "--" {
                parse_args = false;
                goto_next = false;
                continue;
            }
            if arg == "-" {
                paths.clear();
                break;
            }
            if arg == "-g" || arg == "--goto" {
                goto_next = true;
                continue;
            }
            if arg == "-h" || arg == "--help" || (cfg!(windows) && arg == "/?") {
                print_help();
                return Ok(true);
            }
            if arg == "-v" || arg == "--version" {
                print_version();
                return Ok(true);
            }
        }

        #[cfg(windows)]
        let argument = path::from_user_input(Path::new(&arg));
        #[cfg(not(windows))]
        let argument = PathBuf::from(&arg);
        let (argument, goto) = if goto_next {
            goto_next = false;
            documents::DocumentManager::parse_filename_goto(&argument)
        } else {
            (argument.as_path(), None)
        };
        let p = cwd.join(argument);
        let p = path::normalize(&p);
        if p.is_dir() {
            state.wants_file_picker = StateFilePicker::Open;
            dir = Some(p);
        } else {
            paths.push((p, goto));
        }
    }

    for (p, goto) in &paths {
        logging::log_action(&format!("STARTUP_FILE: {}", p.to_string_lossy()));
        let doc = state.documents.add_file_path_literal(p, &state.config)?;
        if let Some(goto) = goto {
            doc.cursor_move_to_goto(*goto);
        }
        // Track in recent files
        if let Some(resolved) = &doc.path {
            state.config.add_recent_file(resolved);
            state.file_watcher.watch(resolved);
        }
    }

    // Determine the initial file picker directory:
    // 1. If a directory was explicitly passed as argument, use that
    // 2. Otherwise, check if we have a saved folder for this project
    // 3. Use the opened file's parent, then the current working directory.
    let initial_dir = initial_picker_directory(
        dir.as_deref(),
        paths.last().and_then(|(p, _)| p.parent()),
        state.config.get_project_folder(&state.startup_cwd).map(Path::new),
        &cwd,
    );

    state.file_picker_pending_dir = DisplayablePathBuf::from_path(initial_dir);
    Ok(false)
}

fn initial_picker_directory(
    explicit: Option<&Path>,
    file_parent: Option<&Path>,
    saved: Option<&Path>,
    cwd: &Path,
) -> PathBuf {
    explicit
        .or_else(|| saved.filter(|path| path.is_dir()))
        .or(file_parent)
        .unwrap_or(cwd)
        .to_owned()
}

/// Validate/reopen terminal input after help/version handling, before raw mode.
fn handle_stdin(state: &mut State) -> apperr::Result<()> {
    if let Some(mut file) = sys::reopen_stdin_if_redirected()? {
        logging::log_action("STARTUP_STDIN: Reading from redirected stdin");
        let doc = state.documents.add_untitled(&state.config)?;
        let mut tb = doc.buffer.borrow_mut();
        tb.read_file(&mut file, None)?;
        tb.mark_as_dirty();
    } else if state.documents.len() == 0 {
        state.documents.add_untitled(&state.config)?;
    }
    Ok(())
}

fn print_help() {
    sys::write_stdout(concat!(
        "Usage: ogedit [OPTIONS] [FILE]...\n",
        "Options:\n",
        "    -g, --goto <FILE:LINE[:CHARACTER]>    Open at the specified line and character\n",
        "    -h, --help                            Print this help message\n",
        "    -v, --version                         Print the version number\n",
        "\nNegative lines count from the end. --goto applies to the next file only.\n",
    ));
}

fn print_version() {
    sys::write_stdout(concat!("ogedit version ", env!("CARGO_PKG_VERSION"), "\n"));
}

fn draw(ctx: &mut Context, state: &mut State) {
    native_open::poll(ctx, state);
    if ctx
        .keyboard_input()
        .is_none_or(|key| shortcuts::resolve(&state.config.hotkeys, key).is_none())
    {
        workbench::shortcuts(ctx, state);
    }
    draw_menubar(ctx, state);
    draw_editor(ctx, state);
    draw_statusbar(ctx, state);

    if state.wants_close {
        draw_handle_wants_close(ctx, state);
    }
    if state.wants_exit {
        draw_handle_wants_exit(ctx, state);
    }
    if state.wants_goto {
        draw_goto_menu(ctx, state);
    }
    if state.wants_file_picker != StateFilePicker::None {
        draw_file_picker(ctx, state);
    }
    if state.wants_save {
        draw_handle_save(ctx, state);
    }
    if state.wants_encoding_change != StateEncodingChange::None {
        draw_dialog_encoding_change(ctx, state);
    }
    if state.wants_language_picker {
        draw_dialog_language_change(ctx, state);
    }
    if state.wants_go_to_file {
        state.wants_go_to_file = false;
        workbench::open(state, workbench::Panel::Files);
    }
    if state.wants_about {
        draw_dialog_about(ctx, state);
    }
    if state.wants_reload {
        draw_handle_wants_reload(ctx, state);
    }
    if ctx.clipboard_ref().wants_host_sync() {
        draw_handle_clipboard_change(ctx, state);
    }
    if state.error_log_count != 0 {
        draw_error_log(ctx, state);
    }
    workbench::draw(ctx, state);

    handle_shortcuts(ctx, state, false);
}

fn handle_shortcuts(ctx: &mut Context, state: &mut State, editor_focused: bool) {
    use shortcuts::Command::*;
    let Some(key) = ctx.keyboard_input() else {
        return;
    };
    let Some(command) = shortcuts::resolve(&state.config.hotkeys, key) else {
        return;
    };
    if command.edits_buffer() && !editor_focused {
        return;
    }
    match command {
        New => draw_add_untitled_document(ctx, state),
        Open => state.wants_file_picker = StateFilePicker::Open,
        Save => state.wants_save = true,
        SaveAs => state.wants_file_picker = StateFilePicker::SaveAs,
        Reload => {
            if state.documents.active().is_some_and(|doc| doc.path.is_some()) {
                state.wants_reload = true;
            }
        }
        Close => state.wants_close = true,
        Exit => state.wants_exit = true,
        Files => state.wants_go_to_file = true,
        Goto => state.wants_goto = true,
        Duplicate => draw_duplicate_line(state),
        Find | Replace => {
            if state.wants_search.kind != StateSearchKind::Disabled {
                state.wants_search.kind = if command == Find {
                    StateSearchKind::Search
                } else {
                    StateSearchKind::Replace
                };
                state.wants_search.focus = true;
            }
        }
        FindNext => search_execute(ctx, state, SearchAction::Search),
        Wrap => {
            if let Some(doc) = state.documents.active() {
                let mut tb = doc.buffer.borrow_mut();
                state.config.word_wrap = !tb.is_word_wrap_enabled();
                tb.set_word_wrap(state.config.word_wrap);
                if let Err(err) = state.config.save() {
                    drop(tb);
                    error_log_add(ctx, state, err);
                }
            }
        }
        Undo | Redo | Cut | Copy | Paste | SelectAll => {
            if let Some(doc) = state.documents.active() {
                let mut tb = doc.buffer.borrow_mut();
                match command {
                    Undo => {
                        tb.undo();
                        logging::log_undo();
                    }
                    Redo => {
                        tb.redo();
                        logging::log_redo();
                    }
                    Cut => {
                        tb.cut(ctx.clipboard_mut());
                        logging::log_cut(ctx.clipboard_ref().read());
                    }
                    Copy => {
                        tb.copy(ctx.clipboard_mut());
                        logging::log_copy(ctx.clipboard_ref().read());
                    }
                    Paste => {
                        logging::log_paste(ctx.clipboard_ref().read());
                        tb.paste(ctx.clipboard_ref());
                    }
                    SelectAll => {
                        tb.select_all();
                        logging::log_select_all();
                    }
                    _ => unreachable!(),
                }
                tb.make_cursor_visible();
            }
        }
    }
    logging::log_shortcut(key, &format!("{command:?}"));
    ctx.needs_rerender();
    ctx.set_input_consumed();
}

fn draw_handle_wants_exit(_ctx: &mut Context, state: &mut State) {
    while let Some(doc) = state.documents.active() {
        if doc.buffer.borrow().is_dirty() {
            state.wants_close = true;
            return;
        }
        // Stop watching this file before closing
        if let Some(path) = &doc.path {
            state.file_watcher.unwatch(path);
        }
        state.documents.remove_active();
    }

    if state.documents.len() == 0 {
        state.exit = true;
    }
}

fn write_terminal_title(arena: &Arena, output: &mut ArenaString, state: &mut State) {
    let (filename, dirty) = state
        .documents
        .active()
        .map_or(("", false), |d| (&d.filename, d.buffer.borrow().is_dirty()));

    if filename == state.osc_title_file_status.filename
        && dirty == state.osc_title_file_status.dirty
    {
        return;
    }

    output.push_str("\x1b]0;");
    if !filename.is_empty() {
        if dirty {
            output.push_str("● ");
        }
        let scratch = scratch_arena(Some(arena));
        output.push_str(&unicode::sanitize_control_chars(&scratch, filename));
        output.push_str(" - ");
    }
    output.push_str("edit\x1b\\");

    state.osc_title_file_status.filename = filename.to_string();
    state.osc_title_file_status.dirty = dirty;
}

const LARGE_CLIPBOARD_THRESHOLD: usize = 128 * KIBI;

fn draw_handle_clipboard_change(ctx: &mut Context, state: &mut State) {
    let data_len = ctx.clipboard_ref().read().len();

    if state.osc_clipboard_always_send || data_len < LARGE_CLIPBOARD_THRESHOLD {
        ctx.clipboard_mut().mark_as_synchronized();
        state.osc_clipboard_sync = true;
        return;
    }

    let over_limit = data_len >= SCRATCH_ARENA_CAPACITY / 4;
    let mut done = None;

    ctx.modal_begin("warning", loc(LocId::WarningDialogTitle));
    {
        ctx.block_begin("description");
        ctx.attr_padding(Rect::three(1, 2, 1));

        if over_limit {
            ctx.label("line1", loc(LocId::LargeClipboardWarningLine1));
            ctx.attr_position(Position::Center);
            ctx.label("line2", loc(LocId::SuperLargeClipboardWarning));
            ctx.attr_position(Position::Center);
        } else {
            let label2 = {
                let template = loc(LocId::LargeClipboardWarningLine2);
                let size = arena_format!(ctx.arena(), "{}", MetricFormatter(data_len));

                let mut label =
                    ArenaString::with_capacity_in(template.len() + size.len(), ctx.arena());
                label.push_str(template);
                label.replace_once_in_place("{size}", &size);
                label
            };

            ctx.label("line1", loc(LocId::LargeClipboardWarningLine1));
            ctx.attr_position(Position::Center);
            ctx.label("line2", &label2);
            ctx.attr_position(Position::Center);
            ctx.label("line3", loc(LocId::LargeClipboardWarningLine3));
            ctx.attr_position(Position::Center);
        }
        ctx.block_end();

        ctx.table_begin("choices");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(0, 2, 1));
        ctx.attr_position(Position::Center);
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        {
            ctx.table_next_row();
            ctx.inherit_focus();

            if over_limit {
                if ctx.button("ok", loc(LocId::Ok), ButtonStyle::default()) {
                    logging::log_dialog_close("Large Clipboard Warning", "Ok");
                    done = Some(true);
                }
                ctx.inherit_focus();
            } else {
                if ctx.button("always", loc(LocId::Always), ButtonStyle::default()) {
                    logging::log_dialog_close("Large Clipboard Warning", "Always");
                    state.osc_clipboard_always_send = true;
                    done = Some(true);
                }

                if ctx.button("yes", loc(LocId::Yes), ButtonStyle::default()) {
                    logging::log_dialog_close("Large Clipboard Warning", "Yes");
                    done = Some(true);
                }
                if data_len < 10 * LARGE_CLIPBOARD_THRESHOLD {
                    ctx.inherit_focus();
                }

                if ctx.button("no", loc(LocId::No), ButtonStyle::default()) {
                    logging::log_dialog_close("Large Clipboard Warning", "No");
                    done = Some(false);
                }
                if data_len >= 10 * LARGE_CLIPBOARD_THRESHOLD {
                    ctx.inherit_focus();
                }
            }
        }
        ctx.table_end();
    }
    if ctx.modal_end() {
        logging::log_dialog_close("Large Clipboard Warning", "Escape");
        done = Some(false);
    }

    if let Some(sync) = done {
        state.osc_clipboard_sync = sync;
        ctx.clipboard_mut().mark_as_synchronized();
        ctx.needs_rerender();
    }
}

#[cold]
fn write_osc_clipboard(tui: &mut Tui, state: &mut State, output: &mut ArenaString) {
    let clipboard = tui.clipboard_mut();
    let data = clipboard.read();

    if !data.is_empty() {
        // Rust doubles the size of a string when it needs to grow it.
        // If `data` is *really* large, this may then double
        // the size of the `output` from e.g. 100MB to 200MB. Not good.
        // We can avoid that by reserving the needed size in advance.
        output.reserve_exact(base64::encode_len(data.len()) + 16);
        output.push_str("\x1b]52;c;");
        base64::encode(output, data);
        output.push_str("\x1b\\");
    }

    state.osc_clipboard_sync = false;
}

struct RestoreModes;

impl Drop for RestoreModes {
    fn drop(&mut self) {
        // Same as in the beginning but in the reverse order.
        // It also includes DECSCUSR 0 to reset the cursor style and DECTCEM to show the cursor.
        // We specifically don't reset mode 1036, because most applications expect it to be set nowadays.
        sys::write_stdout("\x1b[0 q\x1b[?25h\x1b]0;\x07\x1b[?1002;1006;2004l\x1b[?1049l");
    }
}

fn setup_terminal(tui: &mut Tui, state: &mut State, vt_parser: &mut vt::Parser) -> RestoreModes {
    sys::write_stdout(concat!(
        // 1049: Alternative Screen Buffer
        //   I put the ASB switch in the beginning, just in case the terminal performs
        //   some additional state tracking beyond the modes we enable/disable.
        // 1002: Cell Motion Mouse Tracking
        // 1006: SGR Mouse Mode
        // 2004: Bracketed Paste Mode
        // 1036: Xterm: "meta sends escape" (Alt keypresses should be encoded with ESC + char)
        "\x1b[?1049h\x1b[?1002;1006;2004h\x1b[?1036h",
        // OSC 4 color table requests for indices 0 through 15 (base colors).
        "\x1b]4;0;?;1;?;2;?;3;?;4;?;5;?;6;?;7;?\x07",
        "\x1b]4;8;?;9;?;10;?;11;?;12;?;13;?;14;?;15;?\x07",
        // OSC 10 and 11 queries for the current foreground and background colors.
        "\x1b]10;?\x07\x1b]11;?\x07",
        // Test whether ambiguous width characters are two columns wide.
        // We use "…", because it's the most common ambiguous width character we use,
        // and the old Windows conhost doesn't actually use wcwidth, it measures the
        // actual display width of the character and assigns it columns accordingly.
        // We detect it by writing the character and asking for the cursor position.
        "\r…\x1b[6n",
        // CSI c reports the terminal capabilities.
        // It also helps us to detect the end of the responses, because not all
        // terminals support the OSC queries, but all of them support CSI c.
        "\x1b[c",
    ));

    let mut done = false;
    let mut osc_buffer = String::new();
    let mut indexed_colors = framebuffer::DEFAULT_THEME;
    let mut color_responses = 0;
    let mut ambiguous_width = 1;

    while !done {
        let scratch = scratch_arena(None);

        // We explicitly set a high read timeout, because we're not
        // waiting for user keyboard input. If we encounter a lone ESC,
        // it's unlikely to be from a ESC keypress, but rather from a VT sequence.
        let Some((resize, input)) = sys::read_stdin(&scratch, Duration::from_secs(3)) else {
            break;
        };
        if let Some(size) = resize {
            tui.set_size(size);
        }

        let mut vt_stream = vt_parser.parse(&input);
        while let Some(token) = vt_stream.next() {
            match token {
                Token::Csi(csi) => match csi.final_byte {
                    'c' => done = true,
                    // CPR (Cursor Position Report) response.
                    'R' => ambiguous_width = csi.params[1] as CoordType - 1,
                    _ => {}
                },
                Token::Osc { mut data, partial } => {
                    if partial {
                        osc_buffer.push_str(data);
                        continue;
                    }
                    if !osc_buffer.is_empty() {
                        osc_buffer.push_str(data);
                        data = &osc_buffer;
                    }

                    let mut splits = data.split_terminator(';');

                    let color = match splits.next().unwrap_or("") {
                        // The response is `4;<color>;rgb:<r>/<g>/<b>`.
                        "4" => match splits.next().unwrap_or("").parse::<usize>() {
                            Ok(val) if val < 16 => &mut indexed_colors[val],
                            _ => continue,
                        },
                        // The response is `10;rgb:<r>/<g>/<b>`.
                        "10" => &mut indexed_colors[IndexedColor::Foreground as usize],
                        // The response is `11;rgb:<r>/<g>/<b>`.
                        "11" => &mut indexed_colors[IndexedColor::Background as usize],
                        _ => continue,
                    };

                    let color_param = splits.next().unwrap_or("");
                    if !color_param.starts_with("rgb:") {
                        continue;
                    }

                    let mut iter = color_param[4..].split_terminator('/');
                    let rgb_parts = [(); 3].map(|_| iter.next().unwrap_or("0"));
                    let mut rgb = 0;

                    for part in rgb_parts {
                        if part.len() == 2 || part.len() == 4 {
                            let Ok(mut val) = usize::from_str_radix(part, 16) else {
                                continue;
                            };
                            if part.len() == 4 {
                                // Round from 16 bits to 8 bits.
                                val = (val * 0xff + 0x7fff) / 0xffff;
                            }
                            rgb = (rgb << 8) | (val as u32);
                        }
                    }

                    *color = StraightRgba::from_rgba(rgb << 8 | 0xff);
                    color_responses += 1;
                    osc_buffer.clear();
                }
                _ => {}
            }
        }
    }

    if ambiguous_width == 2 {
        unicode::setup_ambiguous_width(2);
        state.documents.reflow_all();
    }

    if color_responses == indexed_colors.len() {
        tui.setup_indexed_colors(indexed_colors);
    }

    RestoreModes
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn remembered_folder_is_not_shadowed_by_a_file_argument() {
        let root = std::env::temp_dir().join(format!("ogedit-folder-order-{}", process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file_parent = Path::new("opened-file-folder");
        let explicit = Path::new("explicit-folder");
        let cwd = Path::new("cwd");
        assert_eq!(initial_picker_directory(None, Some(file_parent), Some(&root), cwd), root);
        assert_eq!(
            initial_picker_directory(Some(explicit), Some(file_parent), Some(&root), cwd),
            explicit
        );
        assert_eq!(
            initial_picker_directory(None, Some(file_parent), Some(&root.join("missing")), cwd),
            file_parent
        );
        assert_eq!(initial_picker_directory(None, None, None, cwd), cwd);
        std::fs::remove_dir(&root).unwrap();
    }
}
