// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Adapted from microsoft/edit cc1b35e; eager newline conversion and separate
// paste transactions are intentional OGEdit behavior.

use std::io::{Read, Seek, SeekFrom, Write};

use super::*;
use crate::sys::memfd;

fn prep_buffer(text: impl AsRef<[u8]>) -> TextBuffer {
    fn innert(text: &[u8]) -> TextBuffer {
        icu::init().unwrap();
        let mut buf = TextBuffer::new(false).unwrap();

        // `read_file` allows us to initialize the buffer with `text` as-is.
        if !text.is_empty() {
            let mut file = memfd().unwrap();
            file.write_all(text.as_ref()).unwrap();
            file.seek(SeekFrom::Start(0)).unwrap();
            assert!(buf.read_file(&mut file, Some("UTF-8")).is_ok());
        }

        // NOTE: `read_file` changes these settings, so we need to `read_file` first, then change them.
        buf.set_crlf(false);
        buf.set_insert_final_newline(false);
        buf.set_indent_with_tabs(false);
        buf.set_tab_size(4);
        buf
    }
    innert(text.as_ref())
}

fn buffer_stored_bytes(buf: &TextBuffer) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(buf.text_length());
    while let chunk = buf.read_forward(bytes.len())
        && !chunk.is_empty()
    {
        bytes.extend_from_slice(chunk);
    }
    bytes
}

fn buffer_write_file_bytes(buf: &mut TextBuffer) -> Vec<u8> {
    let mut file = memfd().unwrap();
    assert!(buf.write_file(&mut file).is_ok());

    file.seek(SeekFrom::Start(0)).unwrap();

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();

    bytes
}

#[derive(Debug, PartialEq, Eq)]
struct BufferState {
    text: Vec<u8>,
    cursor_offset: usize,
    cursor_pos: Point,
    selection: Option<TextBufferSelection>,
}

fn buffer_state(buf: &TextBuffer) -> BufferState {
    BufferState {
        text: buffer_stored_bytes(buf),
        cursor_offset: buf.cursor.offset,
        cursor_pos: buf.cursor.logical_pos,
        selection: buf.selection,
    }
}

#[track_caller]
fn assert_layout(buf: &TextBuffer) {
    let text = buffer_stored_bytes(buf);
    let document = text.as_slice();
    let config = crate::unicode::MeasurementConfig::new(&document)
        .with_tab_size(buf.tab_size())
        .with_word_wrap_column(if buf.is_word_wrap_enabled() && buf.text_width() >= 2 {
            buf.text_width()
        } else {
            0
        });

    let end = config.clone().goto_logical(Point::MAX);
    assert_eq!(buf.logical_line_count(), end.logical_pos.y + 1, "logical line count");
    assert_eq!(buf.visual_line_count(), end.visual_pos.y + 1, "visual line count");

    let cursor = config.clone().goto_offset(buf.cursor_offset());
    assert_eq!(buf.cursor_offset(), cursor.offset, "cursor boundary");
    assert_eq!(buf.cursor_logical_pos(), cursor.logical_pos, "logical cursor");
    assert_eq!(buf.cursor.visual_pos, cursor.visual_pos, "visual cursor");
    assert_eq!(buf.cursor.column, cursor.column, "cursor column");
}

#[track_caller]
fn assert_round_trip(buf: &mut TextBuffer, before: &BufferState, expected: impl AsRef<[u8]>) {
    #[track_caller]
    fn innert(buf: &mut TextBuffer, before: &BufferState, expected: &[u8]) {
        let after = buffer_state(buf);

        assert_eq!(after.text, expected);
        assert_layout(buf);

        // Doing two iterations ensures that undo<>redo really round-trips.
        for cycle in 0..2 {
            buf.undo();
            assert_eq!(&buffer_state(buf), before, "undo cycle {cycle}");
            assert_layout(buf);

            buf.redo();
            assert_eq!(buffer_state(buf), after, "redo cycle {cycle}");
            assert_layout(buf);
        }
    }

    innert(buf, before, expected.as_ref());
}

// Successive writes must remain one undo step even when they extend an existing grapheme.
#[test]
fn test_undo_redo_typing_run() {
    for (initial, offset, writes, expected) in [
        ("abcdef", 0, &["a", "b", "c"][..], "abcabcdef"),
        ("", 0, &["a", "\u{301}"][..], "a\u{301}"),
        ("a", 1, &["\u{301}", "b"][..], "a\u{301}b"),
        ("\u{1f469}", 4, &["\u{200d}", "\u{1f4bb}"][..], "\u{1f469}\u{200d}\u{1f4bb}"),
        ("", 0, &["\u{1f1fa}", "\u{1f1f8}"][..], "\u{1f1fa}\u{1f1f8}"),
    ] {
        let mut buf = prep_buffer(initial);
        buf.cursor_move_to_offset(offset);

        let before = buffer_state(&buf);
        for text in writes {
            buf.write_canon(text.as_bytes());
        }
        assert_round_trip(&mut buf, &before, expected);
    }
}

// Grapheme joins must leave word navigation usable both immediately after editing and after replay.
#[test]
fn test_joining_edits_preserve_word_navigation() {
    for (initial, offset, delete, insert, word_end) in [
        ("a\rX\nb\n", 2, 1, "", 4),
        ("a\n\u{301}z next", 1, 1, "", 4),
        ("\u{301}z next", 0, 0, "ab", 5),
        ("\u{1f469}\u{1f4bb} next", 4, 0, "\u{200d}", 16),
        ("\u{1f1fa}\u{1f1f8}\u{1f1ec}\u{1f1e7}", 8, 0, "\u{1f1e9}", 20),
    ] {
        let mut buf = prep_buffer(initial);
        buf.cursor_move_to_offset(offset);

        if delete != 0 {
            buf.delete(CursorMovement::Grapheme, delete);
        } else {
            buf.write_raw(insert.as_bytes());
        }

        let cursor = buf.cursor_offset();
        for _ in 0..2 {
            buf.cursor_move_to_offset(cursor);
            buf.cursor_move_delta(CursorMovement::Word, 1);
            assert_eq!(buf.cursor_offset(), word_end, "{initial:?}");

            buf.cursor_move_to_offset(cursor);
            buf.cursor_move_delta(CursorMovement::Word, -1);
            assert_eq!(buf.cursor_offset(), 0, "{initial:?}");

            buf.undo();
            buf.redo();
        }
    }
}

// Moving away from a joining edit must not change where history replays its bytes.
#[test]
fn test_undo_redo_after_cursor_movement() {
    let mut buf = prep_buffer("first\nabcdefghij\nlast");
    buf.cursor_move_to_offset(11);

    let before = buffer_state(&buf);
    buf.write_raw("\u{301}".as_bytes());

    for offset in [0, 9, 10, 11, 13, buf.text_length()] {
        buf.cursor_move_to_offset(offset);
        let after = buffer_state(&buf);

        buf.undo();
        assert_eq!(buffer_state(&buf), before);
        assert_layout(&buf);

        buf.redo();
        assert_eq!(buffer_state(&buf), after);
        assert_layout(&buf);
    }
}

// Deleting a separator can join its neighbors into a grapheme containing the original edit offset.
#[test]
fn test_undo_redo_joining_deletions() {
    for (prefix, removed, suffix) in [
        ("a", "\n", "\u{301}z"),
        ("a", "\r\n", "\u{301}z"),
        ("a\u{308}", "\n", "\u{301}z"),
        ("\u{1100}", "\n", "\u{1161}z"),
        ("\u{1f469}\u{200d}", "\n", "\u{1f4bb}z"),
        ("\u{1f1fa}", "\n", "\u{1f1f8}z"),
        ("\u{600}", "\n", "a"),
        ("\r", "X", "\nz"),
    ] {
        for backward in [false, true] {
            let initial = format!("{prefix}{removed}{suffix}");

            let mut buf = prep_buffer(&initial);
            let offset = prefix.len() + if backward { removed.len() } else { 0 };
            buf.cursor_move_to_offset(offset);

            let before = buffer_state(&buf);
            buf.delete(CursorMovement::Grapheme, if backward { -1 } else { 1 });
            assert_round_trip(&mut buf, &before, format!("{prefix}{suffix}"));
        }
    }
}

// The deletion phase may temporarily join graphemes or CRLF before insertion changes the layout again.
#[test]
fn test_undo_redo_joining_replacements_with_wrapping() {
    for (prefix, removed, suffix) in [("abcde", "\n", "\u{301}z"), ("abcde\r", "X", "\nz")] {
        for replacement in ["", "X", "\n", "X\nY"] {
            let initial = format!("{prefix}{removed}{suffix}");

            let mut buf = prep_buffer(&initial);
            buf.set_word_wrap(true);
            buf.set_width(4);
            buf.cursor_move_to_offset(prefix.len());
            buf.selection_update_offset(prefix.len() + removed.len());

            let before = buffer_state(&buf);
            buf.write_raw(replacement.as_bytes());
            assert_round_trip(&mut buf, &before, format!("{prefix}{replacement}{suffix}"));
        }
    }
}

// Insertions can change grapheme boundaries on either side, so history must retain exact byte offsets.
#[test]
fn test_undo_redo_joining_insertions_at_both_edges() {
    for (initial, offset, text, expected) in [
        ("\u{301}z", 0, "a\nb", "a\nb\u{301}z"),
        ("a\rb", 2, "\n", "a\r\nb"),
        ("a\u{308}z", 3, "\u{301}", "a\u{308}\u{301}z"),
        ("\u{301}z", 0, "ab", "ab\u{301}z"),
        ("\u{1f469}\u{1f4bb}z", 4, "\u{200d}", "\u{1f469}\u{200d}\u{1f4bb}z"),
        (
            "\u{1f1fa}\u{1f1f8}\u{1f1ec}\u{1f1e7}",
            8,
            "\u{1f1e9}",
            "\u{1f1fa}\u{1f1f8}\u{1f1e9}\u{1f1ec}\u{1f1e7}",
        ),
        ("az", 1, "\u{301}\n\u{600}", "a\u{301}\n\u{600}z"),
    ] {
        let mut buf = prep_buffer(initial);
        buf.cursor_move_to_offset(offset);

        let before = buffer_state(&buf);
        buf.write_raw(text.as_bytes());
        assert_round_trip(&mut buf, &before, expected);
    }
}

// Measuring malformed UTF-8 as replacement characters must not rewrite its original bytes in history.
#[test]
fn test_joining_edits_preserve_invalid_utf8() {
    let mut buf = prep_buffer([b'a', 0xE1, 0x80, b'z']);
    buf.cursor_move_to_offset(3);

    let before = buffer_state(&buf);
    buf.write_raw("\u{301}".as_bytes());
    assert_round_trip(&mut buf, &before, [b'a', 0xE1, 0x80, 0xCC, 0x81, b'z']);
}

// Typing resumes before the automatically inserted newline, making consecutive writes nonadjacent.
#[test]
fn test_undo_redo_automatic_final_newline() {
    for crlf in [false, true] {
        let mut buf = prep_buffer("");
        buf.set_crlf(crlf);
        buf.set_insert_final_newline(true);

        let initial = buffer_state(&buf);
        for text in [b"a", b"b", b"c"] {
            buf.write_canon(text);
        }
        assert_round_trip(&mut buf, &initial, if crlf { "abc\r\n" } else { "abc\n" });
    }
}

// A line paste inserts at the line start rather than at the end of the preceding typing edit.
#[test]
fn test_undo_redo_line_paste_after_typing_keeps_paste_separate() {
    let mut buf = prep_buffer("tail");
    buf.cursor_move_to_offset(2);

    buf.write_canon(b"x");
    // OGEdit deliberately keeps a paste separate from the preceding typing group.
    let initial = buffer_state(&buf);

    let mut clipboard = crate::clipboard::Clipboard::default();
    clipboard.write(b"line\n".to_vec());
    clipboard.write_was_line_copy(true);

    buf.paste(&clipboard);
    assert_round_trip(&mut buf, &initial, "line\ntaxil");
}

// Crossing from 9 to 10 logical lines widens the line-number margin and changes the available wrap width.
#[test]
fn test_undo_redo_group_changes_wrap_width() {
    let initial = format!("{}abcdef", "\n".repeat(8));

    let mut buf = prep_buffer(&initial);
    buf.set_margin_enabled(true);
    buf.set_word_wrap(true);
    buf.set_width(8);

    let before = buffer_state(&buf);
    buf.find_and_replace_all("a", Default::default(), b"a\n").unwrap();
    assert_round_trip(&mut buf, &before, initial.as_str().replace('a', "a\n"));
}

// No-op edits must not discard a redo branch as a real edit would.
#[test]
fn test_empty_edits_preserve_redo() {
    let mut buf = prep_buffer("a");
    buf.cursor_move_to_offset(1);

    buf.write_raw(b"b");
    buf.undo();

    buf.write_raw(b"");
    buf.delete(CursorMovement::Grapheme, 0);

    buf.redo();
    assert_eq!(buffer_stored_bytes(&buf), b"ab");
}

// Grouped deletions must compose correctly across direction changes, newlines, and newly joined graphemes.
#[test]
fn test_undo_redo_deletion_run() {
    let cases: &[(&str, usize, &[CoordType], &str)] = &[
        ("abcdef", 3, &[-1, -1, -1], "def"),
        ("abcdef", 3, &[1, 1, 1], "abc"),
        ("abcdef", 3, &[-1, 1, -1], "aef"),
        ("abcdef", 3, &[1, -1, 1], "abf"),
        ("a\nb\nc", 4, &[-1, -1, -1], "ac"),
        ("first\nab\ncd", 11, &[-1, -1, -1, -1], "first\na"),
        ("a\r\nb\r\nc", 1, &[1, 1, 1], "ac"),
        ("a\u{301}\u{1f600}\u{754c}z", 0, &[1, 1, 1], "z"),
        ("a\u{301}\u{1f600}\u{754c}z", 10, &[-1, -1, -1], "z"),
        ("abc", 0, &[1, 1, 1, 1], ""),
        ("a\n\u{301}z", 1, &[1, 1], "a\u{301}"),
        ("a\n\u{301}z", 1, &[1, -1], "z"),
        ("a\n\u{301}z", 2, &[-1, 1], "a\u{301}"),
        ("a\n\u{301}z", 2, &[-1, -1], "z"),
        ("a\r\n\u{301}z\nq", 3, &[-1, 1, 1, -1], "q"),
    ];

    for &(initial, offset, deletes, expected) in cases {
        let mut buf = prep_buffer(initial);
        buf.set_crlf(initial.contains('\r'));
        buf.cursor_move_to_offset(offset);

        let before = buffer_state(&buf);
        for &delta in deletes {
            buf.delete(CursorMovement::Grapheme, delta);
            assert_layout(&buf);
        }
        assert_round_trip(&mut buf, &before, expected);
    }
}

// Auto-unindent followed by deleting the newline must remain one deletion group with correct wrapped layout.
#[test]
fn test_backspace_unindents_across_lines_in_one_undo() {
    let mut buf = prep_buffer("first\n    ");
    buf.set_word_wrap(true);
    buf.set_width(4);
    buf.cursor_move_to_logical(Point::MAX);

    let before = buffer_state(&buf);
    buf.backspace_with_auto_unindent(CursorMovement::Grapheme);
    assert_eq!(buffer_stored_bytes(&buf), b"first\n");

    buf.backspace_with_auto_unindent(CursorMovement::Grapheme);
    assert_round_trip(&mut buf, &before, "first");
}

// Replacing a multiline selection must restore both its original bytes and its selection direction on undo.
#[test]
fn test_undo_redo_selection_replacement() {
    for (crlf, beg, end, replacement, expected) in [
        (false, 0, 1, "", "\tata\nlast"),
        (false, 1, 0, "X", "\taXta\nlast"),
        (true, 0, 1, "longer\nreplacement\n", "\talonger\r\nreplacement\r\nta\r\nlast"),
        (false, 0, 1, "\u{754c}\u{1f600}", "\ta\u{754c}\u{1f600}ta\nlast"),
    ] {
        let initial = if crlf { "\talpha\r\nbeta\r\nlast" } else { "\talpha\nbeta\nlast" };

        let mut buf = prep_buffer(initial);
        buf.set_crlf(crlf);
        buf.cursor_move_to_logical(Point { x: 2, y: beg });
        buf.selection_update_logical(Point { x: 2, y: end });

        let before = buffer_state(&buf);
        buf.write_raw(replacement.as_bytes());
        assert!(!buf.has_selection());
        assert_round_trip(&mut buf, &before, expected);
    }
}

// Overtype history must combine inserted and displaced text even when writes cross newlines or extend past EOF.
#[test]
fn test_undo_redo_overtype() {
    let cases: &[(&str, &[&str], &str)] = &[
        ("abcdef", &["XY", "Z"], "aXYZef"),
        ("ab", &["XYZ", "!"], "aXYZ!"),
        ("abcdef", &["X\nY", "Z"], "aX\nYZef"),
        ("a\u{754c}\u{1f600}z", &["x", "y"], "axyz"),
    ];

    for &(initial, writes, expected) in cases {
        let mut buf = prep_buffer(initial);
        buf.set_overtype(true);
        buf.cursor_move_to_offset(1);

        let before = buffer_state(&buf);
        for text in writes {
            buf.write_canon(text.as_bytes());
        }
        assert_round_trip(&mut buf, &before, expected);
    }
}

// Cursor-separated edits must replay in order and stop changing state when either history stack is exhausted.
#[test]
fn test_undo_redo_multiple_steps() {
    let mut buf = prep_buffer("a\nb\nc");

    let first = buffer_state(&buf);
    buf.write_raw(b"prefix\n");
    buf.cursor_move_to_offset(10);

    let second = buffer_state(&buf);
    buf.write_raw(b"\nsuffix");
    buf.cursor_move_to_offset(3);

    let third = buffer_state(&buf);
    buf.write_raw(b"!");
    let last = buffer_state(&buf);
    assert_eq!(buffer_stored_bytes(&buf), b"pre!fix\na\nb\nsuffix\nc");

    for state in [&third, &second, &first, &first] {
        buf.undo();
        assert_eq!(&buffer_state(&buf), state);
        assert_layout(&buf);
    }

    for state in [&second, &third, &last, &last] {
        buf.redo();
        assert_eq!(&buffer_state(&buf), state);
        assert_layout(&buf);
    }
}

// Typing after undo must discard redo without merging into the older write at the branch point.
#[test]
fn test_undo_redo_branching() {
    let mut buf = prep_buffer("tail");
    buf.write_raw(b"a");
    buf.cursor_move_to_offset(1);

    let branch_point = buffer_state(&buf);
    buf.write_raw(b"b");

    buf.undo();
    assert_eq!(buffer_state(&buf), branch_point);

    buf.write_raw(b"x");
    buf.redo();
    assert_eq!(buffer_stored_bytes(&buf), b"axtail", "editing must discard the redo branch");
    assert_round_trip(&mut buf, &branch_point, "axtail");
}

// Replacements at separate matches must undo atomically despite changing lengths and subsequent match offsets.
#[test]
fn test_undo_redo_replace_all() {
    for (crlf, pattern, replacement, expected) in [
        (false, "cat", "lion", "lion\nlion lion\n"),
        (true, "cat", "", "\r\n \r\n"),
        (false, "^", ">", ">cat\n>cat cat\n"),
        (true, "(c)(at)", "$2$1", "atc\r\natc atc\r\n"),
    ] {
        let initial = if crlf { "cat\r\ncat cat\r\n" } else { "cat\ncat cat\n" };

        let mut buf = prep_buffer(initial);
        buf.set_crlf(crlf);
        buf.cursor_move_to_logical(Point { x: 1, y: 1 });

        let before = buffer_state(&buf);
        buf.find_and_replace_all(
            pattern,
            SearchOptions { use_regex: true, ..Default::default() },
            replacement.as_bytes(),
        )
        .unwrap();
        assert_round_trip(&mut buf, &before, expected);
    }
}

// Multiline indentation must undo atomically, preserving selection direction and excluding its final line-start endpoint.
#[test]
fn test_undo_redo_indentation() {
    for (initial, direction, beg, end, expected) in
        [("a\nb\nc", 1, 0, 2, "\ta\n\tb\nc"), ("\ta\n\tb\nc", -1, 2, 0, "a\nb\nc")]
    {
        let mut buf = prep_buffer(initial);
        buf.set_indent_with_tabs(true);
        buf.cursor_move_to_logical(Point { x: 0, y: beg });
        buf.selection_update_logical(Point { x: 0, y: end });

        let before = buffer_state(&buf);
        buf.indent_change(direction);
        assert_round_trip(&mut buf, &before, expected);
    }
}

// Moving selected lines combines a cut and paste that must replay as one edit with the selection restored.
#[test]
fn test_undo_redo_move_lines() {
    for (crlf, beg, end, direction, expected) in [
        (false, 1, 2, MoveLineDirection::Up, "bb\ncc\naa\ndd\n"),
        (true, 2, 1, MoveLineDirection::Down, "aa\r\ndd\r\nbb\r\ncc\r\n"),
    ] {
        let initial = if crlf { "aa\r\nbb\r\ncc\r\ndd\r\n" } else { "aa\nbb\ncc\ndd\n" };

        let mut buf = prep_buffer(initial);
        buf.set_crlf(crlf);
        buf.cursor_move_to_logical(Point { x: 1, y: beg });
        buf.selection_update_logical(Point { x: 1, y: end });

        let before = buffer_state(&buf);
        buf.move_selected_lines(direction);
        assert_round_trip(&mut buf, &before, expected);
    }
}

// History must measure with current wrapping and tab settings rather than restore layout cached during recording.
#[test]
fn test_undo_redo_uses_current_layout() {
    let mut buf = prep_buffer("\tabcdefghij\nlast");

    let before = buffer_state(&buf);
    buf.find_and_replace_all("a", Default::default(), b"\talpha\n").unwrap();
    let after = buffer_state(&buf);

    for (wrap, width, tab_size) in [(true, 4, 4), (true, 8, 8), (false, 20, 4)] {
        buf.set_word_wrap(wrap);
        buf.set_width(width);
        buf.set_tab_size(tab_size);

        buf.undo();
        assert_eq!(buffer_state(&buf), before);
        assert_layout(&buf);

        // Redo must not reuse layout from either recording or the preceding undo.
        buf.set_word_wrap(true);
        buf.set_width(6);
        buf.set_tab_size(2);

        buf.redo();
        assert_eq!(buffer_state(&buf), after);
        assert_layout(&buf);
    }
}

// Wrapped height accounting must cover the entire replaced range, including EOF with or without a final newline.
#[test]
fn test_undo_redo_multiline_replacement_with_wrapping() {
    for trailing_newline in [false, true] {
        for delete_to_eof in [false, true] {
            let mut original = "abcd\nefgh\nijkl\nmnop\nuntouched suffix".to_string();
            if trailing_newline {
                original.push('\n');
            }

            let mut buf = prep_buffer(&original);
            buf.set_word_wrap(true);
            buf.set_width(4);
            buf.cursor_move_to_offset(1);
            let end = if delete_to_eof { original.len() } else { 17 };
            buf.selection_update_offset(end);

            let before = buffer_state(&buf);
            buf.write_raw(b"XX\n\tY");
            let expected = format!("aXX\n\tY{}", &original[end..]);
            assert_round_trip(&mut buf, &before, &expected);
        }
    }
}

// Requesting LF normalization is a real branching edit even when the restored insertion preference is already LF.
#[test]
fn test_changing_newline_format_discards_redo() {
    let mut buf = prep_buffer(b"a\r\nb\n");

    buf.normalize_newlines(true);
    buf.undo();

    buf.normalize_newlines(false);
    buf.redo();
    assert!(!buf.is_crlf());
    assert_eq!(buffer_write_file_bytes(&mut buf), b"a\nb\n");
}

// Replacing a zero-width line-end match must advance before searching again, including at the automatic final newline.
#[test]
fn test_replace_one_zero_width() {
    let mut buf = prep_buffer("a\nb\n");
    buf.set_insert_final_newline(true);

    for _ in 0..6 {
        buf.find_and_replace("$", SearchOptions { use_regex: true, ..Default::default() }, b"x")
            .unwrap();
    }

    assert_eq!(buffer_stored_bytes(&buf), b"axx\nbxx\nx\n");
}

// Replace All must advance past zero-width matches rather than repeatedly replace the same line end.
#[test]
fn test_replace_all_zero_width() {
    let mut buf = prep_buffer("a\nb\n");
    buf.set_insert_final_newline(true);

    buf.find_and_replace_all("$", SearchOptions { use_regex: true, ..Default::default() }, b"x")
        .unwrap();

    assert_eq!(buffer_stored_bytes(&buf), b"ax\nbx\nx\n");
}

#[test]
fn empty_zero_width_replacement_makes_progress_and_preserves_undo() {
    let options = SearchOptions { use_regex: true, ..Default::default() };
    for pattern in ["^", "$", "(?=a)"] {
        let mut buf = prep_buffer("a\na\na");
        buf.write_raw(b"x");
        buf.undo();
        buf.find_and_replace_all(pattern, options, b"").unwrap();
        assert_eq!(buffer_stored_bytes(&buf), b"a\na\na");
        buf.redo();
        assert_eq!(buffer_stored_bytes(&buf), b"xa\na\na");
    }
    let mut buf = prep_buffer("a\na\na");
    for _ in 0..8 {
        buf.find_and_replace("$", options, b"").unwrap();
    }
    assert_eq!(buffer_stored_bytes(&buf), b"a\na\na");
}

#[test]
fn replacing_with_a_changed_pattern_does_not_replace_the_previous_match() {
    let mut buf = prep_buffer("cat dog");
    buf.find_and_select("cat", Default::default()).unwrap();
    buf.find_and_replace("dog", Default::default(), b"bird").unwrap();
    assert_eq!(buffer_stored_bytes(&buf), b"cat dog");
    buf.find_and_replace("dog", Default::default(), b"bird").unwrap();
    assert_eq!(buffer_stored_bytes(&buf), b"cat bird");
}

#[test]
fn saved_revision_survives_undo_and_diverging_edits() {
    let mut buf = prep_buffer("");
    buf.write_canon(b"a");
    buf.mark_saved();
    buf.undo();
    assert!(buf.is_dirty());
    buf.write_canon(b"b");
    assert!(buf.is_dirty(), "a new branch must not reuse the saved generation");
    buf.undo();
    buf.redo();
    assert!(buf.is_dirty());
    assert_eq!(buffer_stored_bytes(&buf), b"b");

    buf.mark_saved();
    buf.write_canon(b"c");
    buf.undo();
    assert_eq!(buffer_stored_bytes(&buf), b"b");
    assert!(!buf.is_dirty(), "saving must end the current typing group");
}

#[test]
fn changing_encoding_cannot_become_clean_by_undoing_text() {
    let mut buf = prep_buffer("text");
    buf.write_canon(b"a");
    buf.set_encoding("UTF-8 BOM");
    buf.undo();
    assert!(buf.is_dirty());
    assert_eq!(buf.encoding(), "UTF-8 BOM");
}

#[test]
fn loaded_revision_does_not_collide_with_later_edits() {
    let mut buf = prep_buffer("");
    for _ in 0..5 {
        buf.write_raw(b"x");
    }
    buf.replace_loaded_content(prep_buffer("new"));
    assert!(!buf.is_dirty());
    for _ in 0..10 {
        buf.write_canon(b"z");
        assert!(buf.is_dirty());
    }
    buf.undo();
    assert_eq!(buffer_stored_bytes(&buf), b"new");
    assert!(!buf.is_dirty());
}

#[test]
fn newline_preference_without_existing_newlines_is_undoable() {
    let mut buf = prep_buffer("text");
    buf.normalize_newlines(true);
    assert!(buf.is_crlf());
    assert!(buf.is_dirty());
    buf.undo();
    assert!(!buf.is_crlf());
    assert!(!buf.is_dirty());
    buf.redo();
    assert!(buf.is_crlf());
    assert_eq!(buffer_stored_bytes(&buf), b"text");
}

#[test]
fn paste_into_single_line_fields_respects_cursor_and_line_copy() {
    for text in ["a\nb", "a\rb", "a\r\nb"] {
        let mut buf = prep_buffer("xy");
        buf.cursor_move_to_offset(1);
        let mut clipboard = Clipboard::default();
        clipboard.write(text.as_bytes().to_vec());
        clipboard.write_was_line_copy(true);
        let before = buffer_state(&buf);
        buf.paste_single_line(&clipboard);
        assert_round_trip(&mut buf, &before, b"xay");
    }
}

#[test]
fn auto_unindent_preserves_multiple_cursors_and_undo() {
    let mut buf = prep_buffer("    a\n  b\n\tc");
    buf.set_selection_ranges(&[(4, 4), (8, 8), (11, 11)]);
    let before = buf.selection_ranges();
    buf.backspace_with_auto_unindent(CursorMovement::Grapheme);
    assert_eq!(buffer_stored_bytes(&buf), b"a\nb\nc");
    assert_eq!(buf.selection_count(), 3);
    buf.undo();
    assert_eq!(buffer_stored_bytes(&buf), b"    a\n  b\n\tc");
    assert_eq!(buf.selection_ranges(), before);
}

#[test]
fn replace_all_invalid_regex_does_not_leave_an_open_undo_group() {
    let mut buf = prep_buffer("text");
    assert!(
        buf.find_and_replace_all(
            "[",
            SearchOptions { use_regex: true, ..Default::default() },
            b"x"
        )
        .is_err()
    );
    buf.write_canon(b"a");
    buf.undo();
    assert_eq!(buffer_stored_bytes(&buf), b"text");
    assert_eq!(buf.edit_group_depth, 0);
}

#[test]
fn loading_beyond_small_buffer_capacity_reports_failure() {
    let mut file = memfd().unwrap();
    file.write_all(&vec![b'x'; 129 * KIBI]).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = TextBuffer::new(true).unwrap();
    assert!(buf.read_file(&mut file, Some("UTF-8")).is_err());
}

#[test]
fn buffered_encoded_saves_and_loads_preserve_conversion_boundaries() {
    let text = "αβ界🙂\r\n".repeat(3_000);
    for encoding in ["UTF-16LE", "UTF-32BE"] {
        let mut buf = prep_buffer(&text);
        buf.set_encoding(encoding);
        let mut file = memfd().unwrap();
        buf.write_file(&mut file).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut loaded = TextBuffer::new(false).unwrap();
        loaded.read_file(&mut file, None).unwrap();
        assert_eq!(buffer_stored_bytes(&loaded), text.as_bytes());
    }
}

#[test]
fn markdown_styles_reach_the_terminal_with_syntax_enabled() {
    let mut buf = prep_buffer("**bold** *italic* [link](https://example.test) ~~gone~~");
    buf.set_syntax_path(std::path::Path::new("sample.md"));
    buf.set_width(80);
    let mut frame = Framebuffer::new();
    frame.flip(Size { width: 80, height: 2 });
    buf.render(Point::default(), Rect { left: 0, top: 0, right: 80, bottom: 2 }, true, &mut frame);
    let arena = scratch_arena(None);
    let output = frame.render(&arena);
    for code in ["\x1b[1m", "\x1b[3m", "\x1b[4m", "\x1b[9m"] {
        assert!(output.contains(code), "missing {code:?} in {output:?}");
    }
}
