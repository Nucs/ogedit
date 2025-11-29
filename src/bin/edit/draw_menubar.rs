// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ogedit::arena_format;
use ogedit::helpers::*;
use ogedit::input::{kbmod, vk};
use ogedit::tui::*;

use crate::draw_editor::draw_duplicate_line;
use crate::localization::*;
use crate::logging;
use crate::state::*;

pub fn draw_menubar(ctx: &mut Context, state: &mut State) {
    ctx.menubar_begin();
    ctx.attr_background_rgba(state.menubar_color_bg);
    ctx.attr_foreground_rgba(state.menubar_color_fg);
    {
        let contains_focus = ctx.contains_focus();

        if ctx.menubar_menu_begin(loc(LocId::File), 'F') {
            draw_menu_file(ctx, state);
        }
        if !contains_focus && ctx.consume_shortcut(vk::F10) {
            ctx.steal_focus();
        }
        if state.documents.active().is_some() {
            if ctx.menubar_menu_begin(loc(LocId::Edit), 'E') {
                draw_menu_edit(ctx, state);
            }
            if ctx.menubar_menu_begin(loc(LocId::View), 'V') {
                draw_menu_view(ctx, state);
            }
        }
        if ctx.menubar_menu_begin(loc(LocId::Help), 'H') {
            draw_menu_help(ctx, state);
        }
    }
    ctx.menubar_end();
}

fn draw_menu_file(ctx: &mut Context, state: &mut State) {
    // Check for file changes immediately when File menu is opened
    if let Some(doc) = state.documents.active() {
        if doc.has_file_changed_on_disk() {
            state.file_changed_cached = true;
        }
    }

    if ctx.menubar_menu_button(loc(LocId::FileNew), 'N', kbmod::CTRL | vk::N) {
        logging::log_menu_click("File->New");
        draw_add_untitled_document(ctx, state);
    }
    if ctx.menubar_menu_button(loc(LocId::FileOpen), 'O', kbmod::CTRL | vk::O) {
        logging::log_menu_click("File->Open");
        state.wants_file_picker = StateFilePicker::Open;
    }
    if state.documents.active().is_some() {
        if ctx.menubar_menu_button(loc(LocId::FileSave), 'S', kbmod::CTRL | vk::S) {
            logging::log_menu_click("File->Save");
            state.wants_save = true;
        }
        if ctx.menubar_menu_button(loc(LocId::FileSaveAs), 'A', vk::NULL) {
            logging::log_menu_click("File->Save As");
            state.wants_file_picker = StateFilePicker::SaveAs;
        }
        // Reload from disk - only show if document has a file path
        if state.documents.active().is_some_and(|d| d.path.is_some()) {
            let changed = state.file_changed_cached;

            // Show "[!]" prefix when file has changed on disk (menus don't support per-item colors)
            if changed {
                let label = arena_format!(ctx.arena(), "[!] {}", loc(LocId::FileReloadFromDisk));
                if ctx.menubar_menu_button(&label, 'R', vk::F5) {
                    logging::log_menu_click("File->Reload From Disk");
                    state.wants_reload = true;
                }
            } else if ctx.menubar_menu_button(loc(LocId::FileReloadFromDisk), 'R', vk::F5) {
                logging::log_menu_click("File->Reload From Disk");
                state.wants_reload = true;
            }
        }
        if ctx.menubar_menu_button(loc(LocId::FileClose), 'C', kbmod::CTRL | vk::W) {
            logging::log_menu_click("File->Close");
            state.wants_close = true;
        }
    }
    if ctx.menubar_menu_button(loc(LocId::FileExit), 'X', kbmod::CTRL | vk::Q) {
        logging::log_menu_click("File->Exit");
        state.wants_exit = true;
    }
    ctx.menubar_menu_end();
}

fn draw_menu_edit(ctx: &mut Context, state: &mut State) {
    {
        let doc = state.documents.active().unwrap();
        let mut tb = doc.buffer.borrow_mut();

        if ctx.menubar_menu_button(loc(LocId::EditUndo), 'U', kbmod::CTRL | vk::Z) {
            logging::log_menu_click("Edit->Undo");
            logging::log_undo();
            tb.undo();
            ctx.needs_rerender();
        }
        if ctx.menubar_menu_button(loc(LocId::EditRedo), 'R', kbmod::CTRL | vk::Y) {
            logging::log_menu_click("Edit->Redo");
            logging::log_redo();
            tb.redo();
            ctx.needs_rerender();
        }
        if ctx.menubar_menu_button(loc(LocId::EditCut), 'T', kbmod::CTRL | vk::X) {
            logging::log_menu_click("Edit->Cut");
            tb.cut(ctx.clipboard_mut());
            ctx.needs_rerender();
        }
        if ctx.menubar_menu_button(loc(LocId::EditCopy), 'C', kbmod::CTRL | vk::C) {
            logging::log_menu_click("Edit->Copy");
            tb.copy(ctx.clipboard_mut());
            ctx.needs_rerender();
        }
        if ctx.menubar_menu_button(loc(LocId::EditPaste), 'P', kbmod::CTRL | vk::V) {
            logging::log_menu_click("Edit->Paste");
            tb.paste(ctx.clipboard_ref());
            ctx.needs_rerender();
        }
    }

    if ctx.menubar_menu_button(loc(LocId::EditDuplicate), 'D', kbmod::CTRL | vk::D) {
        logging::log_menu_click("Edit->Duplicate");
        logging::log_duplicate_line();
        draw_duplicate_line(state);
        ctx.needs_rerender();
    }

    if state.wants_search.kind != StateSearchKind::Disabled {
        if ctx.menubar_menu_button(loc(LocId::EditFind), 'F', kbmod::CTRL | vk::F) {
            logging::log_menu_click("Edit->Find");
            state.wants_search.kind = StateSearchKind::Search;
            state.wants_search.focus = true;
        }
        if ctx.menubar_menu_button(loc(LocId::EditReplace), 'L', kbmod::CTRL | vk::R) {
            logging::log_menu_click("Edit->Replace");
            state.wants_search.kind = StateSearchKind::Replace;
            state.wants_search.focus = true;
        }
    }

    {
        let doc = state.documents.active().unwrap();
        let mut tb = doc.buffer.borrow_mut();
        if ctx.menubar_menu_button(loc(LocId::EditSelectAll), 'A', kbmod::CTRL | vk::A) {
            logging::log_menu_click("Edit->Select All");
            logging::log_select_all();
            tb.select_all();
            ctx.needs_rerender();
        }
    }

    ctx.menubar_menu_end();
}

fn draw_menu_view(ctx: &mut Context, state: &mut State) {
    if let Some(doc) = state.documents.active() {
        let mut tb = doc.buffer.borrow_mut();
        let word_wrap = tb.is_word_wrap_enabled();

        // All values on the statusbar are currently document specific.
        if ctx.menubar_menu_button(loc(LocId::ViewFocusStatusbar), 'S', vk::NULL) {
            logging::log_menu_click("View->Focus Statusbar");
            state.wants_statusbar_focus = true;
        }
        if ctx.menubar_menu_button(loc(LocId::ViewGoToFile), 'F', kbmod::CTRL | vk::P) {
            logging::log_menu_click("View->Go To File");
            state.wants_go_to_file = true;
        }
        if ctx.menubar_menu_button(loc(LocId::FileGoto), 'G', kbmod::CTRL | vk::G) {
            logging::log_menu_click("View->Go To Line");
            state.wants_goto = true;
        }
        if ctx.menubar_menu_checkbox(loc(LocId::ViewWordWrap), 'W', kbmod::ALT | vk::Z, word_wrap) {
            let new_state = !word_wrap;
            logging::log_menu_checkbox("View->Word Wrap", new_state);
            logging::log_word_wrap_toggle(new_state);
            tb.set_word_wrap(new_state);
            state.config.word_wrap = new_state;
            // Save config to disk
            let _ = state.config.save();
            ctx.needs_rerender();
        }
    }

    ctx.menubar_menu_end();
}

fn draw_menu_help(ctx: &mut Context, state: &mut State) {
    if ctx.menubar_menu_button(loc(LocId::HelpAbout), 'A', vk::NULL) {
        logging::log_menu_click("Help->About");
        state.wants_about = true;
    }
    ctx.menubar_menu_end();
}

pub fn draw_dialog_about(ctx: &mut Context, state: &mut State) {
    ctx.modal_begin("about", loc(LocId::AboutDialogTitle));
    {
        ctx.block_begin("content");
        ctx.inherit_focus();
        ctx.attr_padding(Rect::three(1, 2, 1));
        {
            ctx.label("description", "OGEdit");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.label(
                "version",
                &arena_format!(
                    ctx.arena(),
                    "{}{}",
                    loc(LocId::AboutDialogVersion),
                    env!("CARGO_PKG_VERSION")
                ),
            );
            ctx.attr_overflow(Overflow::TruncateHead);
            ctx.attr_position(Position::Center);

            ctx.label("maintainer", "Maintained by Nucs / Eli Belash");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.label("fork", "Fork of Microsoft Edit");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.label("copyright", "Copyright (c) Microsoft Corp 2025");
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_position(Position::Center);

            ctx.block_begin("choices");
            ctx.inherit_focus();
            ctx.attr_padding(Rect::three(1, 2, 0));
            ctx.attr_position(Position::Center);
            {
                if ctx.button("ok", loc(LocId::Ok), ButtonStyle::default()) {
                    logging::log_dialog_close("About", "Ok");
                    state.wants_about = false;
                }
                ctx.inherit_focus();
            }
            ctx.block_end();
        }
        ctx.block_end();
    }
    if ctx.modal_end() {
        logging::log_dialog_close("About", "Escape");
        state.wants_about = false;
    }
}
