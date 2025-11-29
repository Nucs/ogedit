// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::ParseIntError;

use ogedit::framebuffer::IndexedColor;
use ogedit::helpers::*;
use ogedit::icu;
use ogedit::input::{kbmod, vk};
use ogedit::tui::*;

use crate::localization::*;
use crate::logging;
use crate::state::*;

pub fn draw_editor(ctx: &mut Context, state: &mut State) {
    if !matches!(state.wants_search.kind, StateSearchKind::Hidden | StateSearchKind::Disabled) {
        draw_search(ctx, state);
    }

    let size = ctx.size();
    // TODO: The layout code should be able to just figure out the height on its own.
    let height_reduction = match state.wants_search.kind {
        StateSearchKind::Search => 4,
        StateSearchKind::Replace => 5,
        _ => 2,
    };

    if let Some(doc) = state.documents.active() {
        ctx.textarea("textarea", doc.buffer.clone());
        ctx.inherit_focus();
    } else {
        ctx.block_begin("empty");
        ctx.block_end();
    }

    ctx.attr_intrinsic_size(Size { width: 0, height: size.height - height_reduction });
}

fn draw_search(ctx: &mut Context, state: &mut State) {
    if let Err(err) = icu::init() {
        error_log_add(ctx, state, err);
        state.wants_search.kind = StateSearchKind::Disabled;
        return;
    }

    let Some(doc) = state.documents.active() else {
        state.wants_search.kind = StateSearchKind::Hidden;
        return;
    };

    let mut action = None;
    let mut focus = StateSearchKind::Hidden;

    if state.wants_search.focus {
        state.wants_search.focus = false;
        focus = StateSearchKind::Search;

        // If the selection is empty, focus the search input field.
        // Otherwise, focus the replace input field, if it exists.
        if let Some(selection) = doc.buffer.borrow_mut().extract_user_selection(false) {
            state.search_needle = String::from_utf8_lossy_owned(selection);
            focus = state.wants_search.kind;
        }
    }

    ctx.block_begin("search");
    ctx.attr_focus_well();
    ctx.attr_background_rgba(ctx.indexed(IndexedColor::White));
    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::Black));
    {
        if ctx.contains_focus() && ctx.consume_shortcut(vk::ESCAPE) {
            logging::log_dialog_close("Find/Replace", "Escape");
            state.wants_search.kind = StateSearchKind::Hidden;
        }

        ctx.table_begin("needle");
        ctx.table_set_cell_gap(Size { width: 1, height: 0 });
        {
            {
                ctx.table_next_row();
                ctx.label("label", loc(LocId::SearchNeedleLabel));

                if ctx.editline("needle", &mut state.search_needle) {
                    action = Some(SearchAction::Search);
                }
                if !state.search_success {
                    ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
                    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
                }
                ctx.attr_intrinsic_size(Size { width: COORD_TYPE_SAFE_MAX, height: 1 });
                if focus == StateSearchKind::Search {
                    ctx.steal_focus();
                }
                if ctx.is_focused() && ctx.consume_shortcut(vk::RETURN) {
                    action = Some(SearchAction::Search);
                }
            }

            if state.wants_search.kind == StateSearchKind::Replace {
                ctx.table_next_row();
                ctx.label("label", loc(LocId::SearchReplacementLabel));

                ctx.editline("replacement", &mut state.search_replacement);
                ctx.attr_intrinsic_size(Size { width: COORD_TYPE_SAFE_MAX, height: 1 });
                if focus == StateSearchKind::Replace {
                    ctx.steal_focus();
                }
                if ctx.is_focused() {
                    if ctx.consume_shortcut(vk::RETURN) {
                        action = Some(SearchAction::Replace);
                    } else if ctx.consume_shortcut(kbmod::CTRL_ALT | vk::RETURN) {
                        action = Some(SearchAction::ReplaceAll);
                    }
                }
            }
        }
        ctx.table_end();

        ctx.table_begin("options");
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        {
            let mut change = false;
            let mut change_action = Some(SearchAction::Search);

            ctx.table_next_row();

            let old_match_case = state.search_options.match_case;
            change |= ctx.checkbox(
                "match-case",
                loc(LocId::SearchMatchCase),
                &mut state.search_options.match_case,
            );
            if old_match_case != state.search_options.match_case {
                logging::log_action(&format!("SEARCH_OPTION: match_case={}", state.search_options.match_case));
            }

            let old_whole_word = state.search_options.whole_word;
            change |= ctx.checkbox(
                "whole-word",
                loc(LocId::SearchWholeWord),
                &mut state.search_options.whole_word,
            );
            if old_whole_word != state.search_options.whole_word {
                logging::log_action(&format!("SEARCH_OPTION: whole_word={}", state.search_options.whole_word));
            }

            let old_use_regex = state.search_options.use_regex;
            change |= ctx.checkbox(
                "use-regex",
                loc(LocId::SearchUseRegex),
                &mut state.search_options.use_regex,
            );
            if old_use_regex != state.search_options.use_regex {
                logging::log_action(&format!("SEARCH_OPTION: use_regex={}", state.search_options.use_regex));
            }

            if state.wants_search.kind == StateSearchKind::Replace
                && ctx.button("replace-all", loc(LocId::SearchReplaceAll), ButtonStyle::default())
            {
                logging::log_action("BUTTON_CLICK: Replace All");
                change = true;
                change_action = Some(SearchAction::ReplaceAll);
            }
            if ctx.button("close", loc(LocId::SearchClose), ButtonStyle::default()) {
                logging::log_dialog_close("Find/Replace", "Close");
                state.wants_search.kind = StateSearchKind::Hidden;
            }

            if change {
                action = change_action;
                state.wants_search.focus = true;
                ctx.needs_rerender();
            }
        }
        ctx.table_end();
    }
    ctx.block_end();

    if let Some(action) = action {
        search_execute(ctx, state, action);
    }
}

pub enum SearchAction {
    Search,
    Replace,
    ReplaceAll,
}

pub fn search_execute(ctx: &mut Context, state: &mut State, action: SearchAction) {
    let Some(doc) = state.documents.active_mut() else {
        return;
    };

    state.search_success = match action {
        SearchAction::Search => {
            let result = doc.buffer.borrow_mut().find_and_select(&state.search_needle, state.search_options);
            logging::log_search(&state.search_needle, result.is_ok());
            result
        }
        SearchAction::Replace => {
            let result = doc.buffer.borrow_mut().find_and_replace(
                &state.search_needle,
                state.search_options,
                state.search_replacement.as_bytes(),
            );
            if result.is_ok() {
                logging::log_replace(&state.search_needle, &state.search_replacement, 1);
            }
            result
        }
        SearchAction::ReplaceAll => {
            let result = doc.buffer.borrow_mut().find_and_replace_all(
                &state.search_needle,
                state.search_options,
                state.search_replacement.as_bytes(),
            );
            // Note: We don't have the count here, but we log that replace all was executed
            logging::log_action(&format!("Replace all '{}' with '{}'", state.search_needle, state.search_replacement));
            result
        }
    }
    .is_ok();

    ctx.needs_rerender();
}

pub fn draw_handle_save(ctx: &mut Context, state: &mut State) {
    if let Some(doc) = state.documents.active_mut() {
        if doc.path.is_some() {
            let path_str = doc.path.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            if let Err(err) = doc.save(None) {
                logging::log_error(&format!("Failed to save: {}", path_str));
                error_log_add(ctx, state, err);
            } else {
                logging::log_file_save(&path_str);
                // Clear file changed indicator since we just saved
                state.file_changed_cached = false;
                state.file_check_counter = 0;
            }
        } else {
            // No path? Show the file picker.
            state.wants_file_picker = StateFilePicker::SaveAs;
            state.wants_save = false;
            ctx.needs_rerender();
        }
    }

    state.wants_save = false;
}

pub fn draw_handle_wants_close(ctx: &mut Context, state: &mut State) {
    let Some(doc) = state.documents.active() else {
        state.wants_close = false;
        return;
    };

    if !doc.buffer.borrow().is_dirty() {
        logging::log_file_close(&doc.filename, false);
        state.documents.remove_active();
        state.wants_close = false;
        ctx.needs_rerender();
        return;
    }

    enum Action {
        None,
        Save,
        Discard,
        Cancel,
    }
    let mut action = Action::None;

    ctx.modal_begin("unsaved-changes", loc(LocId::UnsavedChangesDialogTitle));
    ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
    ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
    {
        let contains_focus = ctx.contains_focus();

        ctx.label("description", loc(LocId::UnsavedChangesDialogDescription));
        ctx.attr_padding(Rect::three(1, 2, 1));

        ctx.table_begin("choices");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(0, 2, 1));
        ctx.attr_position(Position::Center);
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        {
            ctx.table_next_row();
            ctx.inherit_focus();

            if ctx.button(
                "yes",
                loc(LocId::UnsavedChangesDialogYes),
                ButtonStyle::default().accelerator('S'),
            ) {
                action = Action::Save;
            }
            ctx.inherit_focus();
            if ctx.button(
                "no",
                loc(LocId::UnsavedChangesDialogNo),
                ButtonStyle::default().accelerator('N'),
            ) {
                action = Action::Discard;
            }
            if ctx.button("cancel", loc(LocId::Cancel), ButtonStyle::default()) {
                action = Action::Cancel;
            }

            // Handle accelerator shortcuts
            if contains_focus {
                if ctx.consume_shortcut(vk::S) {
                    action = Action::Save;
                } else if ctx.consume_shortcut(vk::N) {
                    action = Action::Discard;
                }
            }
        }
        ctx.table_end();
    }
    if ctx.modal_end() {
        action = Action::Cancel;
    }

    match action {
        Action::None => return,
        Action::Save => {
            logging::log_dialog_close("Unsaved Changes", "Save");
            state.wants_save = true;
        }
        Action::Discard => {
            logging::log_dialog_close("Unsaved Changes", "Discard");
            if let Some(doc) = state.documents.active() {
                logging::log_file_close(&doc.filename, true);
            }
            state.documents.remove_active();
            state.wants_close = false;
        }
        Action::Cancel => {
            logging::log_dialog_close("Unsaved Changes", "Cancel");
            state.wants_exit = false;
            state.wants_close = false;
        }
    }

    ctx.needs_rerender();
}

pub fn draw_handle_wants_reload(ctx: &mut Context, state: &mut State) {
    let Some(doc) = state.documents.active() else {
        state.wants_reload = false;
        return;
    };

    let is_dirty = doc.buffer.borrow().is_dirty();
    let file_changed = state.file_changed_cached;

    logging::log_action(&format!(
        "RELOAD_HANDLER: is_dirty={}, file_changed={}",
        is_dirty, file_changed
    ));

    // If no unsaved changes AND no external changes, just reload silently
    if !is_dirty && !file_changed {
        logging::log_action("RELOAD_HANDLER: No changes, reloading directly");
        if let Err(err) = crate::state::reload_file_from_disk(state) {
            error_log_add(ctx, state, err);
        }
        state.wants_reload = false;
        ctx.needs_rerender();
        return;
    }

    logging::log_action("RELOAD_HANDLER: Showing confirmation dialog");

    // Determine which scenario we're in
    let (title, description) = match (is_dirty, file_changed) {
        (true, true) => (loc(LocId::ReloadConfirmTitle), loc(LocId::ReloadBothDescription)),
        (true, false) => (loc(LocId::ReloadConfirmTitle), loc(LocId::ReloadConfirmDescription)),
        (false, true) => (loc(LocId::ReloadExternalTitle), loc(LocId::ReloadExternalDescription)),
        (false, false) => unreachable!(), // Handled above
    };

    // Button labels depend on whether we have local changes to discard
    let reload_button_label = if is_dirty {
        loc(LocId::ReloadConfirmDiscard)
    } else {
        loc(LocId::ReloadButton)
    };

    enum Action {
        None,
        Reload,
        Cancel,
    }
    let mut action = Action::None;

    ctx.modal_begin("reload-confirm", title);
    if is_dirty {
        // Red background for destructive action (discarding local changes)
        ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
        ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
    }
    {
        let contains_focus = ctx.contains_focus();

        ctx.label("description", description);
        ctx.attr_padding(Rect::three(1, 2, 1));

        ctx.table_begin("choices");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(0, 2, 1));
        ctx.attr_position(Position::Center);
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        {
            ctx.table_next_row();
            ctx.inherit_focus();

            if ctx.button(
                "reload",
                reload_button_label,
                ButtonStyle::default().accelerator('R'),
            ) {
                action = Action::Reload;
            }
            ctx.inherit_focus();
            if ctx.button("cancel", loc(LocId::Cancel), ButtonStyle::default()) {
                action = Action::Cancel;
            }

            // Handle accelerator shortcuts
            if contains_focus && ctx.consume_shortcut(vk::R) {
                action = Action::Reload;
            }
        }
        ctx.table_end();
    }
    if ctx.modal_end() {
        action = Action::Cancel;
    }

    match action {
        Action::None => return,
        Action::Reload => {
            logging::log_dialog_close("Reload Confirm", "Reload");
            if let Err(err) = crate::state::reload_file_from_disk(state) {
                error_log_add(ctx, state, err);
            }
            state.wants_reload = false;
        }
        Action::Cancel => {
            logging::log_dialog_close("Reload Confirm", "Cancel");
            state.wants_reload = false;
        }
    }

    ctx.needs_rerender();
}

pub fn draw_goto_menu(ctx: &mut Context, state: &mut State) {
    let mut done = false;

    if let Some(doc) = state.documents.active_mut() {
        ctx.modal_begin("goto", loc(LocId::FileGoto));
        {
            if ctx.editline("goto-line", &mut state.goto_target) {
                state.goto_invalid = false;
            }
            if state.goto_invalid {
                ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
                ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
            }

            ctx.attr_intrinsic_size(Size { width: 24, height: 1 });
            ctx.steal_focus();

            if ctx.consume_shortcut(vk::RETURN) {
                match validate_goto_point(&state.goto_target) {
                    Ok(point) => {
                        logging::log_goto(point);
                        let mut buf = doc.buffer.borrow_mut();
                        buf.cursor_move_to_logical(point);
                        buf.make_cursor_visible();
                        done = true;
                    }
                    Err(_) => {
                        logging::log_error(&format!("Invalid goto target: {}", state.goto_target));
                        state.goto_invalid = true;
                    }
                }
                ctx.needs_rerender();
            }
        }
        if ctx.modal_end() {
            logging::log_dialog_close("Go to Line", "Escape");
            done = true;
        }
    } else {
        done = true;
    }

    if done {
        state.wants_goto = false;
        state.goto_target.clear();
        state.goto_invalid = false;
        ctx.needs_rerender();
    }
}

pub fn draw_duplicate_line(state: &mut State) {
    let Some(doc) = state.documents.active_mut() else {
        return;
    };

    doc.buffer.borrow_mut().duplicate_line_or_selection();
}

fn validate_goto_point(line: &str) -> Result<Point, ParseIntError> {
    let mut coords = [0; 2];
    let (y, x) = line.split_once(':').unwrap_or((line, "0"));
    // Using a loop here avoids 2 copies of the str->int code.
    // This makes the binary more compact.
    for (i, s) in [x, y].iter().enumerate() {
        coords[i] = s.parse::<CoordType>()?.saturating_sub(1);
    }
    Ok(Point { x: coords[0], y: coords[1] })
}
