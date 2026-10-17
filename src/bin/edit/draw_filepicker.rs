// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::path::{Path, PathBuf};

use ogedit::framebuffer::IndexedColor;
use ogedit::helpers::*;
use ogedit::input::{kbmod, vk};
use ogedit::tui::*;
use ogedit::{icu, path};

use crate::localization::*;
use crate::logging;
use crate::state::*;

const APP_SAVE_TARGET_OPEN: ogedit::apperr::Error = ogedit::apperr::Error::new_app(1005);

#[derive(Default)]
pub(crate) struct FilePickerPreview {
    // Keep relative input anchored while its preview changes the displayed folder.
    input_directory: Option<PathBuf>,
    selected: Option<PathBuf>,
}

impl FilePickerPreview {
    pub(crate) fn input_directory<'a>(&'a self, displayed: &'a Path) -> &'a Path {
        self.input_directory.as_deref().unwrap_or(displayed)
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    fn update(&mut self, displayed: &Path, input: &Path) -> Option<PathBuf> {
        self.selected = None;
        if input.as_os_str().is_empty() {
            self.reset();
            return None;
        }
        let base = self.input_directory.get_or_insert_with(|| displayed.to_owned());
        let entered = path::join_user_input(base, input);
        let (file, _) = crate::documents::DocumentManager::resolve_filename_goto(&entered);
        if !file.is_file() {
            return None;
        }
        self.selected = file.file_name().map(PathBuf::from);
        let parent = file.parent().map(Path::to_owned);
        // Absolute input does not need an anchor. A later basename replacement
        // should resolve in the folder the user now sees.
        if path::from_user_input(input).is_absolute() {
            self.input_directory = parent.clone();
        }
        parent
    }

    fn selects(&self, entry: &Path) -> bool {
        self.selected.as_deref().is_some_and(|selected| {
            if cfg!(windows) {
                selected.to_string_lossy().to_lowercase() == entry.to_string_lossy().to_lowercase()
            } else {
                selected == entry
            }
        })
    }
}

pub fn draw_file_picker(ctx: &mut Context, state: &mut State) {
    // The save dialog is pre-filled with the current document filename.
    if state.wants_file_picker == StateFilePicker::SaveAs {
        state.wants_file_picker = StateFilePicker::SaveAsShown;

        if state.file_picker_pending_name.as_os_str().is_empty() {
            state.file_picker_pending_name =
                state.documents.active().map_or("Untitled.txt", |doc| doc.filename.as_str()).into();
        }
    }

    let width = (ctx.size().width - 20).max(10);
    let height = (ctx.size().height - 10).max(10);
    let mut doit = None;
    let mut done = false;

    ctx.modal_begin(
        "file-picker",
        if state.wants_file_picker == StateFilePicker::Open {
            loc(LocId::FileOpen)
        } else {
            loc(LocId::FileSaveAs)
        },
    );
    ctx.attr_intrinsic_size(Size { width, height });
    {
        let contains_focus = ctx.contains_focus();
        let mut activated = false;
        let mut reveal_selection = false;

        ctx.table_begin("path");
        ctx.table_set_columns(&[0, COORD_TYPE_SAFE_MAX]);
        ctx.table_set_cell_gap(Size { width: 1, height: 0 });
        ctx.attr_padding(Rect::two(1, 1));
        ctx.inherit_focus();
        {
            ctx.table_next_row();

            ctx.label("dir-label", loc(LocId::SaveAsDialogPathLabel));
            ctx.label("dir", state.file_picker_pending_dir.as_str());
            ctx.attr_overflow(Overflow::TruncateMiddle);

            ctx.table_next_row();
            ctx.inherit_focus();

            ctx.label("name-label", loc(LocId::SaveAsDialogNameLabel));

            let name_changed = ctx.editline("name", &mut state.file_picker_pending_name);
            ctx.inherit_focus();

            if ctx.contains_focus() {
                if name_changed && ctx.is_focused() {
                    reveal_selection |= update_path_preview(ctx, state);
                    update_autocomplete_suggestions(state);
                }
            } else if !state.file_picker_autocomplete.is_empty() {
                state.file_picker_autocomplete.clear();
            }

            if !state.file_picker_autocomplete.is_empty() {
                let bg = ctx.indexed_alpha(IndexedColor::Background, 3, 4);
                let fg = ctx.contrasted(bg);
                let focus_list_beg = ctx.is_focused() && ctx.consume_shortcut(vk::DOWN);
                let focus_list_end = ctx.is_focused() && ctx.consume_shortcut(vk::UP);
                let mut autocomplete_done = ctx.consume_shortcut(vk::ESCAPE);
                let mut suggestion_changed = false;
                // Enter on the path input opens the entered path immediately;
                // an autocomplete popup must not swallow the first Enter.
                if ctx.is_focused() && ctx.consume_shortcut(vk::RETURN) {
                    activated = true;
                    autocomplete_done = true;
                }

                ctx.list_begin("suggestions");
                ctx.attr_float(FloatSpec {
                    anchor: Anchor::Last,
                    gravity_x: 0.0,
                    gravity_y: 0.0,
                    offset_x: 0.0,
                    offset_y: 1.0,
                });
                ctx.attr_border();
                ctx.attr_background_rgba(bg);
                ctx.attr_foreground_rgba(fg);
                {
                    for (idx, suggestion) in state.file_picker_autocomplete.iter().enumerate() {
                        let sel = ctx.list_item(false, suggestion.as_str());
                        if sel != ListSelection::Unchanged {
                            logging::log_action(&format!(
                                "AUTOCOMPLETE_SELECT: {}",
                                suggestion.as_str()
                            ));
                            state.file_picker_pending_name = suggestion.as_path().into();
                            suggestion_changed = true;
                        }
                        if sel == ListSelection::Activated {
                            logging::log_action(&format!(
                                "AUTOCOMPLETE_ACTIVATE: {}",
                                suggestion.as_str()
                            ));
                            autocomplete_done = true;
                        }

                        let is_first = idx == 0;
                        let is_last = idx == state.file_picker_autocomplete.len() - 1;
                        if (is_first && focus_list_beg) || (is_last && focus_list_end) {
                            ctx.list_item_steal_focus();
                        } else if ctx.is_focused()
                            && ((is_first && ctx.consume_shortcut(vk::UP))
                                || (is_last && ctx.consume_shortcut(vk::DOWN)))
                        {
                            ctx.toss_focus_up();
                        }
                    }
                }
                ctx.list_end();

                if suggestion_changed {
                    reveal_selection |= update_path_preview(ctx, state);
                }

                // If the user typed something, we want to put focus back into the editline.
                // TODO: The input should be processed by the editline and not simply get swallowed.
                if ctx.keyboard_input().is_some() {
                    ctx.set_input_consumed();
                    autocomplete_done = true;
                }

                if autocomplete_done {
                    state.file_picker_autocomplete.clear();
                }
            }

            if ctx.is_focused() && ctx.consume_shortcut(vk::RETURN) {
                activated = true;
            }
        }
        ctx.table_end();

        if state.wants_file_picker == StateFilePicker::Open
            && crate::native_dialog::NativeFilePicker::supported()
            && ctx.button("browse", "Browse", ButtonStyle::default().accelerator('B'))
        {
            state.wants_native_browse = true;
            ctx.needs_rerender();
        }

        if state.file_picker_entries.is_none() {
            draw_dialog_saveas_refresh_files(state);
        }

        ctx.scrollarea_begin(
            "directory",
            Size {
                width: 0,
                // -1 for the label (top)
                // -1 for the label (bottom)
                // -1 for the editline (bottom)
                height: height - 3,
            },
        );
        ctx.attr_background_rgba(ctx.indexed_alpha(IndexedColor::Black, 1, 4));
        if reveal_selection {
            let row = state
                .file_picker_entries
                .as_ref()
                .unwrap()
                .iter()
                .flatten()
                .position(|entry| state.file_picker_preview.selects(entry.as_path()));
            if let Some(row) = row {
                ctx.scrollarea_scroll_to(Point {
                    x: 0,
                    y: (row as CoordType - (height - 3) / 2).max(0),
                });
            }
        }
        {
            ctx.next_block_id_mixin(state.file_picker_pending_dir_revision);
            ctx.list_begin("files");
            ctx.inherit_focus();
            let list_focused = ctx.contains_focus();

            for entries in state.file_picker_entries.as_ref().unwrap() {
                for entry in entries {
                    let selected = state.file_picker_preview.selects(entry.as_path());
                    match ctx.list_item(selected, entry.as_str()) {
                        ListSelection::Unchanged => {}
                        ListSelection::Selected if ctx.is_focused() => {
                            logging::log_action(&format!("FILE_LIST_SELECT: {}", entry.as_str()));
                            state.file_picker_pending_name = entry.as_path().into();
                            state.file_picker_preview.reset();
                        }
                        ListSelection::Selected => {} // Preview selection preserves the typed path.
                        ListSelection::Activated => {
                            logging::log_action(&format!("FILE_LIST_ACTIVATE: {}", entry.as_str()));
                            state.file_picker_pending_name = entry.as_path().into();
                            state.file_picker_preview.reset();
                            activated = true;
                        }
                    }
                    if selected && !list_focused {
                        let bg = ctx.indexed(IndexedColor::Green);
                        ctx.attr_background_rgba(bg);
                        ctx.attr_foreground_rgba(ctx.contrasted(bg));
                    }
                    ctx.attr_overflow(Overflow::TruncateMiddle);
                }
            }

            // Show recent files section (only in Open dialog)
            if state.wants_file_picker == StateFilePicker::Open
                && !state.config.recent_files.is_empty()
            {
                // Filter recent files: must exist and not be currently open
                let recent_to_show: Vec<_> = state
                    .config
                    .recent_files
                    .iter()
                    .filter(|rf| rf.path.exists() && !state.documents.is_path_open(&rf.path))
                    .collect();

                if !recent_to_show.is_empty() {
                    // Separator
                    ctx.list_item(false, "── Recent Files ──");
                    ctx.attr_overflow(Overflow::TruncateTail);

                    for recent in recent_to_show {
                        let display = recent.path.to_string_lossy();
                        match ctx.list_item(false, &display) {
                            ListSelection::Unchanged => {}
                            ListSelection::Selected => {
                                logging::log_action(&format!("RECENT_FILE_SELECT: {}", display));
                                state.file_picker_pending_name = recent.path.clone();
                                state.file_picker_preview.reset();
                            }
                            ListSelection::Activated => {
                                logging::log_action(&format!("RECENT_FILE_ACTIVATE: {}", display));
                                state.file_picker_pending_name = recent.path.clone();
                                state.file_picker_preview.reset();
                                activated = true;
                            }
                        }
                        ctx.attr_overflow(Overflow::TruncateMiddle);
                    }
                }
            }

            ctx.list_end();
        }
        ctx.scrollarea_end();

        if contains_focus
            && (ctx.consume_shortcut(vk::BACK) || ctx.consume_shortcut(kbmod::ALT | vk::UP))
        {
            logging::log_action("NAVIGATE_BACK: ..");
            state.file_picker_pending_name = "..".into();
            state.file_picker_preview.reset();
            activated = true;
        }

        if activated {
            doit = draw_file_picker_update_path(state);

            if state.wants_file_picker != StateFilePicker::Open
                && let Some(path) = doit.as_deref()
                && save_target_is_open_elsewhere(&state.documents, path)
            {
                error_log_add(ctx, state, APP_SAVE_TARGET_OPEN);
                doit = None;
            }

            // Check if the file already exists and show an overwrite warning in that case.
            if state.wants_file_picker != StateFilePicker::Open
                && let Some(path) = doit.as_deref()
                && path.exists()
            {
                logging::log_dialog_open("Overwrite Warning");
                state.file_picker_overwrite_warning = doit.take();
            }
        }
    }
    if ctx.modal_end() {
        logging::log_dialog_close("File Picker", "Escape");
        done = true;
    }

    if state.file_picker_overwrite_warning.is_some() {
        let mut save;

        ctx.modal_begin("overwrite", loc(LocId::FileOverwriteWarning));
        ctx.attr_background_rgba(ctx.indexed(IndexedColor::Red));
        ctx.attr_foreground_rgba(ctx.indexed(IndexedColor::BrightWhite));
        {
            let contains_focus = ctx.contains_focus();

            ctx.label("description", loc(LocId::FileOverwriteWarningDescription));
            ctx.attr_overflow(Overflow::TruncateTail);
            ctx.attr_padding(Rect::three(1, 2, 1));

            ctx.table_begin("choices");
            ctx.inherit_focus();
            ctx.attr_padding(Rect::three(0, 2, 1));
            ctx.attr_position(Position::Center);
            ctx.table_set_cell_gap(Size { width: 2, height: 0 });
            {
                ctx.table_next_row();
                ctx.inherit_focus();

                save = ctx.button("yes", loc(LocId::Yes), ButtonStyle::default());
                ctx.inherit_focus();

                if ctx.button("no", loc(LocId::No), ButtonStyle::default()) {
                    logging::log_dialog_close("Overwrite Warning", "No");
                    state.file_picker_overwrite_warning = None;
                }
            }
            ctx.table_end();

            if contains_focus {
                save |= ctx.consume_shortcut(vk::Y);
                if ctx.consume_shortcut(vk::N) {
                    logging::log_dialog_close("Overwrite Warning", "No");
                    state.file_picker_overwrite_warning = None;
                }
            }
        }
        if ctx.modal_end() {
            logging::log_dialog_close("Overwrite Warning", "Cancel");
            state.file_picker_overwrite_warning = None;
        }

        if save {
            logging::log_dialog_close("Overwrite Warning", "Yes (overwrite)");
            doit = state.file_picker_overwrite_warning.take();
        }
    }

    if let Some(path) = doit {
        let path_str = path.to_string_lossy().to_string();
        let is_open = state.wants_file_picker == StateFilePicker::Open;
        let res = if is_open {
            state.documents.add_file_path(&path, &state.config).map(|_| ())
        } else if save_target_is_open_elsewhere(&state.documents, &path) {
            Err(APP_SAVE_TARGET_OPEN)
        } else if let Some(doc) = state.documents.active_mut() {
            save_as_and_watch(doc, &path, &state.file_watcher)
        } else {
            Ok(())
        };
        match res {
            Ok(..) => {
                if is_open {
                    logging::log_file_open(&path_str);
                    logging::log_dialog_close("File Picker", "file opened");
                    // Track in recent files
                    state.config.add_recent_file(&path);
                    // Start watching for external modifications
                    state.file_watcher.watch(&path);
                } else {
                    logging::log_file_save(&path_str);
                    logging::log_dialog_close("File Picker", "file saved");
                    if state.settings.is_settings_path(&path)
                        && let Err(error) = state.settings.reload()
                    {
                        error_log_add_message(ctx, state, error);
                    }
                    // Save the parent directory as the last-used save folder for this project
                    if let Some(parent) = path.parent() {
                        let parent_str = parent.to_string_lossy().to_string();
                        state.config.set_project_folder(&state.startup_cwd, &parent_str);
                    }
                }
                ctx.needs_rerender();
                done = true;
            }
            Err(err) => {
                logging::log_error(&format!(
                    "Failed to {}: {}",
                    if is_open { "open" } else { "save" },
                    path_str
                ));
                error_log_add(ctx, state, err);
            }
        }
    }

    if done {
        state.wants_file_picker = StateFilePicker::None;
        state.file_picker_pending_name = Default::default();
        state.file_picker_preview.reset();
        state.file_picker_entries = Default::default();
        state.file_picker_overwrite_warning = Default::default();
        state.file_picker_autocomplete = Default::default();
    }
}

fn save_as_and_watch(
    doc: &mut crate::documents::Document,
    path: &Path,
    watcher: &crate::watch::FileWatcher,
) -> ogedit::apperr::Result<()> {
    let previous = doc.path.clone();
    doc.save(Some(path.to_owned()))?;
    if let Some(previous) = previous {
        watcher.unwatch(&previous);
    }
    if let Some(saved) = &doc.path {
        watcher.watch(saved);
    }
    Ok(())
}

/// The active document is the manager's first entry. Saving onto another open
/// document would give two independent buffers authority over the same path.
fn save_target_is_open_elsewhere(
    documents: &crate::documents::DocumentManager,
    path: &Path,
) -> bool {
    let resolved = crate::safe_save::resolved_path(path);
    documents.iter().skip(1).any(|document| document.path.as_ref() == Some(&resolved))
}

// Returns Some(path) if the path refers to a file.
fn draw_file_picker_update_path(state: &mut State) -> Option<PathBuf> {
    let old_path =
        state.file_picker_preview.input_directory(state.file_picker_pending_dir.as_path());
    let path = path::join_user_input(old_path, &state.file_picker_pending_name);

    let (dir, name) = if path.is_dir() {
        // If the current path is C:\ and the user selects "..", we want to
        // navigate to the drive picker. Since `path::normalize` will turn C:\.. into C:\,
        // we can detect this by checking if the length of the path didn't change.
        let dir = if cfg!(windows)
            && state.file_picker_pending_name == Path::new("..")
            // It's unnecessary to check the contents of the paths.
            && old_path.as_os_str().len() == path.as_os_str().len()
        {
            Path::new("")
        } else {
            path.as_path()
        };
        (dir, PathBuf::new())
    } else {
        let dir = path.parent().unwrap_or(&path);
        let name = path.file_name().map_or(Default::default(), |s| s.into());
        (dir, name)
    };
    if dir != state.file_picker_pending_dir.as_path() {
        state.file_picker_pending_dir = DisplayablePathBuf::from_path(dir.to_path_buf());
        state.file_picker_pending_dir_revision =
            state.file_picker_pending_dir_revision.wrapping_add(1);
        state.file_picker_entries = None;
    }

    state.file_picker_pending_name = name;
    state.file_picker_preview.reset();
    if state.file_picker_pending_name.as_os_str().is_empty() { None } else { Some(path) }
}

// Preview only existing files; partial input never navigates into a nonexistent folder.
fn update_path_preview(ctx: &mut Context, state: &mut State) -> bool {
    if state.wants_file_picker != StateFilePicker::Open {
        return false;
    }
    let before = state.file_picker_preview.selected.clone();
    let directory = state
        .file_picker_preview
        .update(state.file_picker_pending_dir.as_path(), &state.file_picker_pending_name);
    let directory_changed =
        directory.as_deref().is_some_and(|dir| dir != state.file_picker_pending_dir.as_path());
    if directory_changed {
        state.file_picker_pending_dir = DisplayablePathBuf::from_path(directory.unwrap());
        state.file_picker_entries = None;
    }
    let changed = directory_changed || before != state.file_picker_preview.selected;
    if changed {
        if state.file_picker_preview.selected.is_some() {
            // The file may have been created since this folder was first listed.
            state.file_picker_entries = None;
        }
        // A new list identity discards its previous selection without moving input focus.
        state.file_picker_pending_dir_revision =
            state.file_picker_pending_dir_revision.wrapping_add(1);
        ctx.needs_rerender();
    }
    changed
}

fn draw_dialog_saveas_refresh_files(state: &mut State) {
    let dir = state.file_picker_pending_dir.as_path();
    // ["..", directories, files]
    let mut dirs_files = [Vec::new(), Vec::new(), Vec::new()];

    #[cfg(windows)]
    if dir.as_os_str().is_empty() {
        // If the path is empty, we are at the drive picker.
        // Add all drives as entries.
        for drive in ogedit::sys::drives() {
            dirs_files[1].push(DisplayablePathBuf::from_string(format!("{drive}:\\")));
        }

        state.file_picker_entries = Some(dirs_files);
        return;
    }

    if cfg!(windows) || dir.parent().is_some() {
        dirs_files[0].push(DisplayablePathBuf::from(".."));
    }

    if let Ok(iter) = fs::read_dir(dir) {
        for entry in iter.flatten() {
            if let Ok(metadata) = entry.metadata() {
                let mut name = entry.file_name();
                let dir = metadata.is_dir()
                    || (metadata.is_symlink()
                        && fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()));
                let idx = if dir { 1 } else { 2 };

                if dir {
                    name.push("/");
                }

                dirs_files[idx].push(DisplayablePathBuf::from(name));
            }
        }
    }

    for entries in &mut dirs_files[1..] {
        entries.sort_unstable_by(|a, b| icu::compare_strings(a.as_bytes(), b.as_bytes()));
    }

    state.file_picker_entries = Some(dirs_files);
}

#[inline(never)]
fn update_autocomplete_suggestions(state: &mut State) {
    if state.file_picker_preview.selected.is_some() {
        state.file_picker_autocomplete.clear();
        return;
    }
    let directory =
        state.file_picker_preview.input_directory(state.file_picker_pending_dir.as_path());
    let cached = if directory == state.file_picker_pending_dir.as_path() {
        state.file_picker_entries.as_ref()
    } else {
        None
    };
    state.file_picker_autocomplete =
        complete_entered_path(directory, &state.file_picker_pending_name, cached);
}

fn complete_entered_path(
    directory: &Path,
    input: &Path,
    cached: Option<&[Vec<DisplayablePathBuf>; 3]>,
) -> Vec<DisplayablePathBuf> {
    use std::ffi::{OsStr, OsString};

    if input.as_os_str().is_empty() {
        return Vec::new();
    }
    let bytes = input.as_os_str().as_encoded_bytes();
    let quoted = bytes.first() == Some(&b'"');
    let mut entered = path::from_user_input(input);
    // Look inside an opening quote for suggestions while it is being typed.
    // Commit parsing remains strict and never removes an unmatched quote.
    if quoted
        && entered.as_os_str().as_encoded_bytes().first() == Some(&b'"')
        && !bytes[1..].contains(&b'"')
    {
        let interior = unsafe { OsStr::from_encoded_bytes_unchecked(&bytes[1..]) };
        entered = path::from_user_input(Path::new(interior));
    }
    #[cfg(unix)]
    let quoted = if directory.join(input).exists() {
        entered = input.to_owned();
        false
    } else {
        quoted
    };

    let trailing_separator = entered
        .as_os_str()
        .as_encoded_bytes()
        .last()
        .is_some_and(|&byte| byte == b'/' || cfg!(windows) && byte == b'\\');
    let (prefix, needle) = if trailing_separator {
        (entered.as_path(), OsStr::new(""))
    } else {
        (entered.parent().unwrap_or_else(|| Path::new("")), entered.file_name().unwrap_or_default())
    };
    let scan_dir = path::normalize(&directory.join(prefix));
    let needle = needle.to_string_lossy();
    let needle = if cfg!(windows) { needle.to_lowercase() } else { needle.into_owned() };
    let mut matches: Vec<(bool, String, OsString)> = Vec::new();
    let mut consider = |name: OsString, is_dir: bool| {
        let text = name.to_string_lossy();
        let matches_prefix = if cfg!(windows) {
            text.to_lowercase().starts_with(&needle)
        } else {
            text.starts_with(&needle)
        };
        if !matches_prefix {
            return;
        }
        let candidate = (!is_dir, text.into_owned(), name);
        let index = matches
            .binary_search_by(|entry| {
                entry
                    .0
                    .cmp(&candidate.0)
                    .then_with(|| icu::compare_strings(entry.1.as_bytes(), candidate.1.as_bytes()))
                    .then_with(|| entry.2.cmp(&candidate.2))
            })
            .unwrap_or_else(|i| i);
        if index < 5 {
            matches.insert(index, candidate);
            matches.truncate(5);
        }
    };
    if scan_dir == path::normalize(directory)
        && let Some(cached) = cached
    {
        for (index, entries) in cached.iter().enumerate().skip(1) {
            for entry in entries {
                if let Some(name) = entry.as_path().file_name() {
                    consider(name.to_owned(), index == 1);
                }
            }
        }
    } else if let Ok(entries) = fs::read_dir(&scan_dir) {
        for entry in entries.flatten() {
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            consider(entry.file_name(), is_dir);
        }
    }
    matches
        .into_iter()
        .map(|(is_file, _, name)| {
            let mut suggestion = prefix.join(name).into_os_string();
            if !is_file {
                suggestion.push(std::path::MAIN_SEPARATOR_STR);
            }
            if quoted {
                let mut wrapped = OsString::from("\"");
                wrapped.push(suggestion);
                wrapped.push("\"");
                suggestion = wrapped;
            }
            DisplayablePathBuf::from_path(PathBuf::from(suggestion))
        })
        .collect()
}

#[cfg(test)]
mod save_as_tests {
    use super::*;

    #[test]
    fn repeated_save_as_releases_previous_directory_watches_and_failed_save_keeps_them() {
        let _runtime = crate::test_runtime::lock();
        let fixture = PreviewFixture::new();
        let watcher = crate::watch::FileWatcher::new();
        let mut documents = crate::documents::DocumentManager::default();
        let doc = documents.add_untitled(&crate::config::Config::default()).unwrap();
        doc.buffer.borrow_mut().write_raw(b"keep this");
        for i in 0..32 {
            let directory = fixture.0.join(format!("save-{i}"));
            fs::create_dir(&directory).unwrap();
            save_as_and_watch(doc, &directory.join("notes.txt"), &watcher).unwrap();
            assert_eq!(watcher.registration_counts(), (1, 1));
            let before = doc.path.clone();
            assert!(save_as_and_watch(doc, &directory, &watcher).is_err());
            assert_eq!(doc.path, before);
            assert_eq!(watcher.registration_counts(), (1, 1));
        }
        drop(watcher);
        drop(documents);
    }

    struct PreviewFixture(PathBuf);

    impl PreviewFixture {
        fn new() -> Self {
            let suffix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("ogedit-path-preview-{}-{suffix}", std::process::id()));
            fs::create_dir_all(root.join("sub folder")).unwrap();
            fs::write(root.join("sub folder/CLAUDE.md"), "fixture\nsecond line").unwrap();
            fs::write(root.join("sub folder/other.txt"), "other fixture").unwrap();
            Self(root)
        }
    }

    impl Drop for PreviewFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn preview_follows_quoted_paths_and_line_suffixes() {
        let fixture = PreviewFixture::new();
        let directory = fixture.0.join("sub folder");
        let target = directory.join("CLAUDE.md");
        for input in [
            target.clone(),
            PathBuf::from(format!("\"{}\"", target.display())),
            PathBuf::from(format!("\"{}\":2:3", target.display())),
        ] {
            let mut preview = FilePickerPreview::default();
            assert_eq!(preview.update(&fixture.0, &input), Some(directory.clone()));
            assert!(preview.selects(Path::new("CLAUDE.md")));
            assert!(!preview.selects(Path::new("other.txt")));
        }
        #[cfg(windows)]
        {
            let input = PathBuf::from(
                format!("\"{}\\claude.md\"", directory.display())
                    .replace("\\sub folder", "/sub folder"),
            );
            let mut preview = FilePickerPreview::default();
            assert_eq!(preview.update(&fixture.0, &input), Some(directory));
            assert!(preview.selects(Path::new("CLAUDE.md")));
        }
    }

    #[test]
    fn preview_keeps_relative_input_anchored_until_cleared_or_navigation() {
        let fixture = PreviewFixture::new();
        let directory = fixture.0.join("sub folder");
        let input = Path::new("sub folder/CLAUDE.md");
        let mut preview = FilePickerPreview::default();
        assert_eq!(preview.update(&fixture.0, input), Some(directory.clone()));
        // Renders, further typing, Enter, and Browse all use the original base.
        assert_eq!(preview.update(&directory, input), Some(directory.clone()));
        assert_eq!(
            path::join_user_input(preview.input_directory(&directory), input),
            directory.join("CLAUDE.md")
        );
        assert_eq!(
            preview.update(&directory, Path::new("sub folder/other.txt")),
            Some(directory.clone())
        );
        assert!(preview.selects(Path::new("other.txt")));
        assert!(!preview.selects(Path::new("CLAUDE.md")));
        assert_eq!(preview.update(&directory, Path::new("")), None);
        assert_eq!(preview.input_directory(&directory), directory);
        assert_eq!(preview.update(&directory, Path::new("CLAUDE.md")), Some(directory.clone()));
        preview.reset();
        assert_eq!(preview.input_directory(&fixture.0), fixture.0);
    }

    #[test]
    fn replacing_an_absolute_path_uses_the_previewed_directory() {
        let fixture = PreviewFixture::new();
        let directory = fixture.0.join("sub folder");
        let mut preview = FilePickerPreview::default();
        assert_eq!(
            preview.update(&fixture.0, &directory.join("CLAUDE.md")),
            Some(directory.clone())
        );
        assert_eq!(preview.update(&directory, Path::new("other.txt")), Some(directory));
        assert!(preview.selects(Path::new("other.txt")));
    }

    #[test]
    fn incomplete_paths_remove_preview_selection_without_changing_folder() {
        let fixture = PreviewFixture::new();
        let directory = fixture.0.join("sub folder");
        let mut preview = FilePickerPreview::default();
        assert_eq!(
            preview.update(&fixture.0, Path::new("sub folder/CLAUDE.md")),
            Some(directory.clone())
        );
        for input in
            ["sub folder/CLAU", "missing/CLAUDE.md", "sub folder", "\"sub folder/CLAUDE.md"]
        {
            assert_eq!(preview.update(&directory, Path::new(input)), None, "{input}");
            assert!(preview.selected.is_none());
            assert_eq!(preview.input_directory(&directory), fixture.0);
        }
    }

    #[test]
    fn save_as_cannot_alias_another_open_buffer() {
        let _runtime = crate::test_runtime::lock();
        let suffix =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir()
            .join(format!("ogedit-saveas-open-{}-{suffix}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, "first contents").unwrap();
        fs::write(&second, "second contents").unwrap();
        let mut documents = crate::documents::DocumentManager::default();
        let config = crate::config::Config::default();
        documents.add_file_path(&first, &config).unwrap();
        documents.add_file_path(&second, &config).unwrap();
        assert!(save_target_is_open_elsewhere(&documents, &first));
        assert!(save_target_is_open_elsewhere(&documents, &directory.join(".").join("first.txt")));
        assert!(!save_target_is_open_elsewhere(&documents, &second));
        assert!(!save_target_is_open_elsewhere(&documents, &directory.join("new.txt")));
        drop(documents);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completion_keeps_natural_top_five_in_cached_and_scanned_directories() {
        let _runtime = crate::test_runtime::lock();
        icu::init().unwrap();
        let fixture = PreviewFixture::new();
        let directory = fixture.0.join("natural");
        fs::create_dir(&directory).unwrap();
        let files = [
            "file100.txt",
            "file10.txt",
            "file30.txt",
            "file2.txt",
            "file20.txt",
            "file1.txt",
            "file11.txt",
        ];
        let dirs = ["dir10", "dir2", "dir1"];
        for file in files {
            fs::write(directory.join(file), "fixture").unwrap();
        }
        for dir in dirs {
            fs::create_dir(directory.join(dir)).unwrap();
        }
        let cached = [
            Vec::new(),
            dirs.into_iter()
                .map(|name| DisplayablePathBuf::from_path(PathBuf::from(name)))
                .collect(),
            files
                .into_iter()
                .map(|name| DisplayablePathBuf::from_path(PathBuf::from(name)))
                .collect(),
        ];
        let names = |matches: Vec<DisplayablePathBuf>| {
            matches
                .into_iter()
                .map(|entry| entry.as_path().file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        for cache in [Some(&cached), None] {
            assert_eq!(
                names(complete_entered_path(&directory, Path::new("file"), cache)),
                ["file1.txt", "file2.txt", "file10.txt", "file11.txt", "file20.txt"]
            );
            assert_eq!(
                names(complete_entered_path(&directory, Path::new("./"), cache)),
                ["dir1", "dir2", "dir10", "file1.txt", "file2.txt"]
            );
        }
        let quoted = complete_entered_path(&fixture.0, Path::new(r#""natural/file"#), None);
        assert_eq!(quoted.len(), 5);
        let resolved: Vec<_> =
            quoted.iter().map(|entry| path::join_user_input(&fixture.0, entry.as_path())).collect();
        let expected: Vec<_> = ["file1.txt", "file2.txt", "file10.txt", "file11.txt", "file20.txt"]
            .into_iter()
            .map(|name| directory.join(name))
            .collect();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn completion_breaks_equivalent_unicode_collation_ties_by_original_name() {
        let _runtime = crate::test_runtime::lock();
        icu::init().unwrap();
        let first = "cafe\u{301}.txt";
        let second = "café.txt";
        assert!(icu::compare_strings(first.as_bytes(), second.as_bytes()).is_eq());
        for order in [[first, second], [second, first]] {
            let cached = [
                Vec::new(),
                Vec::new(),
                order
                    .into_iter()
                    .map(|name| DisplayablePathBuf::from_path(PathBuf::from(name)))
                    .collect(),
            ];
            let matches = complete_entered_path(Path::new("."), Path::new("caf"), Some(&cached));
            let actual: Vec<_> = matches.iter().map(|entry| entry.as_path().as_os_str()).collect();
            let mut expected = [std::ffi::OsStr::new(first), std::ffi::OsStr::new(second)];
            expected.sort();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn completion_resolves_typed_parent_paths_and_quote_wrappers() {
        let root = std::env::temp_dir().join(format!("ogedit-completion-{}", std::process::id()));
        let directory = root.join("sub folder");
        fs::create_dir_all(&directory).unwrap();
        let target = directory.join("CLAUDE.md");
        fs::write(&target, "data").unwrap();
        fs::write(directory.join("config.toml"), "data").unwrap();
        for input in [r#""sub folder/CL"#, r#""sub folder/CL""#, "sub folder/CL"] {
            let matches = complete_entered_path(&root, Path::new(input), None);
            assert_eq!(matches.len(), 1, "{input}");
            assert_eq!(path::join_user_input(&root, matches[0].as_path()), target);
        }
        #[cfg(windows)]
        {
            let matches = complete_entered_path(&root, Path::new(r#""sub folder\CL"#), None);
            assert_eq!(matches.len(), 1);
            assert_eq!(path::join_user_input(&root, matches[0].as_path()), target);
            let input =
                format!("\"{}\"", directory.join("cl").to_string_lossy().replace('\\', "/"));
            let matches = complete_entered_path(&root, Path::new(&input), None);
            assert_eq!(matches.len(), 1);
            assert_eq!(path::join_user_input(&root, matches[0].as_path()), target);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
