# Legacy editing audit

Audit boundary: Microsoft Edit fork parent `82cc84a` through OGEdit `01cf53f` (the committed tree before the modernization work of 2026-10-17). The current working tree also includes that day's authorized changes. Findings below distinguish historical defects from earlier fixes already present when this audit began.

## Commit map

| Commit | Delivered change | Audit evidence |
| --- | --- | --- |
| `1af1a9b` | Ctrl+D dispatch in `main.rs`. | The commit message describes a large feature/test suite, but the actual commit changes only two lines. The message is not evidence that those tests existed. |
| `7686004` | `TextBuffer::duplicate_line_or_selection`, inline versus line duplication, and temporary wrap suppression; related dispatch/state changes. | Reviewed selection classification, byte extraction, newline handling, empty input, undo state, and wrapped layout. |
| `d120e7b` | Recompute the edit-start cursor after wrapped replacement to avoid stale layout positions. | The replacement used the final cursor line rather than the original edited line, losing newly inserted rows from the height delta. |
| `0ff7f96` | Selection occurrence highlighting, with filtering and a nominal 1,000-match cap. | Reviewed byte/chunk matching, Unicode rules, gap crossings and limit enforcement. |
| `09da7a6` | Restore reload cursor by matching previous line content; move visual-stat update ahead of cursor validation. | Reviewed line extraction, substring versus complete-line matching, duplicate-line choice, CRLF, and scan complexity. |
| `1d85463` | Silence the unused `safe_start` field warning. | The field is needed again by the corrected wrapped-height calculation. |
| `3f95547` | Unit tests/documentation for highlight rules and reload helpers. | Tests cover ASCII/basic lines, but did not cover the failures below. |
| `396c111` | Gutter click, Shift-click, and drag line selection (credited to upstream PR #622 by jenia90). | Reviewed anchor direction, newline inclusion, logical versus wrapped rows, idle frames and viewport scrolling. |

`src/buffer/navigation.rs` has no fork delta between the two historical boundaries. Its existing word navigation implementation was inspected; no change was needed for the audited fork features.

## Historical defects corrected in this audit

- **Ctrl+D lost selected blank lines.** The copied range already excluded the final line terminator, then a second trailing-newline strip removed real selected content. It now inserts the exact byte range, including interior empty lines.
- **Partial multiline selections expanded to whole lines.** Starting at column zero was enough to classify every multiline selection as complete lines. A partial final line now duplicates the exact selection inline.
- **Ctrl+D at empty EOF did nothing.** An empty final line now duplicates as a newline, with each repetition independently undoable.
- **Ctrl+D could normalize mixed line endings and lost the original undo selection.** It now uses one checked byte transaction, preserves source bytes and the selected line's terminator, and restores original cursor/selection on undo. It also retains the normal wrap layout throughout.
- **Wrapped multiline paste undercounted visual rows.** Recalculation now begins at the original edited logical line, so rows inserted before the final cursor remain included. The pre-existing safety ordering (update statistics before validating cursor) is retained.
- **Gutter upward drag excluded the beginning of the upper line; downward drag omitted the last terminator.** Inclusive logical-line ranges now work in either direction, including wrapped continuation rows and the final unterminated line. The logical anchor persists across idle frames and scrolling; Shift-click extends it.
- **Reload could jump to a longer line containing the old line as a substring.** `nearest_matching_line` matches complete lines and chooses the nearest occurrence, with an earlier-line tie. It scans once, without a potentially huge match vector or repeated prefix scans. Root integrates this helper in document reload.
- **Highlight rules treated UTF-8 byte length as character count.** A lone accented, Hebrew, CJK, or combining-grapheme letter no longer highlights. Unicode whitespace-only selections are excluded; symbols and emoji remain eligible.

## Earlier modernization fixes retained

The current tree already used a streaming KMP matcher before this audit. This had corrected historical highlight/reload matching across gap-buffer chunk boundaries and enforced the result cap inside the match loop. The historical implementation searched each contiguous chunk independently, and its cap was checked only between chunks. This audit retains the streaming matcher and also makes the remaining `find_line_matches` line-number conversion linear instead of rescanning each prefix.

The current tree already supported multicursor transactions, exact-byte undo, and capacity reservations. Ctrl+D now uses those shared transactions; this audit does not replace that work. Existing `multi_transaction_undo_redo_under_wrap_and_multiline_edits` tests set width before enabling wrap, so they never activated the layout they intended to test. The setup is corrected and asserts wrapping is active.

## Regression coverage

- `legacy_duplicate_preserves_empty_lines_partial_selections_and_undo`
- `legacy_duplicate_empty_eof_repetition_and_mixed_newlines`
- `legacy_wrapped_multiline_paste_and_duplicate_keep_measured_layout`
- `legacy_reload_matching_uses_complete_lines_nearest_tie_and_gap`
- `legacy_line_selection_is_inclusive_reversible_and_wrap_independent`
- `legacy_selection_highlights_classify_unicode_graphemes_and_whitespace`
- `legacy_gutter_drag_and_shift_click_select_complete_lines_in_both_directions`
- `legacy_gutter_continuation_row_selects_the_whole_logical_line`
- `application_shortcut_hook_precedes_editor_defaults_but_not_modal_input`
- `legacy_configured_tab_typing_with_wrap_and_final_crlf_remains_responsive`

The wrap regression compares cached logical/visual line counts and cursor positions with a fresh measurement after paste, duplicate, undo and redo at three widths, for LF and CRLF documents. The TUI tests dispatch actual input events rather than invoking only buffer methods. A browser observation initially suggested Tab might hang with wrapping, real tabs, CRLF and final-newline insertion; subsequent browser state showed input had arrived. No hang fix was introduced. A focused event-dispatch regression covers that configuration. The shortcut-hook test supports the root audit's configurable-hotkey correction: application bindings get first refusal only in the focused editor, leaving modal input isolated.

## Validation and limits

Central automated validation and browser-only manual QA are coordinated by the root audit. No native desktop interaction is used. The regression coverage above is intended to make specific failure modes reproducible; it is not a proof that arbitrary terminal, encoding, filesystem or memory-pressure conditions can never fail.
