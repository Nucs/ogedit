# Buffer, Unicode, syntax and ICU memory review

Reviewed on 2026-10-17. This is one part of the memory investigation; application lifecycle, allocator, browser and AddressSanitizer results are recorded separately by the coordinating audit.

## Confirmed defects repaired

| Area | Trigger and defect | Repair / regression evidence |
| --- | --- | --- |
| ICU provider lifetime | A safe move of a `TextBuffer` after starting a search left native `UText` callbacks referring to its former address. A loaded-buffer swap also needed to invalidate a provider that referenced replaced storage. | Place the underlying `GapBuffer` in a stable allocation, point the native provider at that allocation, close search objects before storage replacement/destruction, and close regex before its provider. Repeated vector reallocations, moves, reloads, edits, undo and destruction are covered. |
| ICU initialization and cached services | Safe APIs wrote unsynchronized `static mut` function/catalog state and shared mutable collator/case-map handles. Concurrent callers could race during initialization or use. | `OnceLock` initializes process-wide immutable tables once. Native collator and case-map handles belong to a thread and close at thread exit. Sixteen concurrent threads repeatedly initialize, compare and fold Unicode text. |
| Native allocation failures | `Regex::new` could return after a later setup error without closing a successfully allocated regex. | Close non-null native regex handles on the error path. Stress creates/destroys 512 valid regex objects and rejects three invalid patterns each iteration. |
| Native provider cache initialization | Native extension storage was turned into Rust references without explicitly establishing initialized `DoubleCache` values. | Initialize both caches, arrays and the boolean MRU field before constructing Rust references, including cloned providers. |
| Malformed UTF-8 in native search | Backward cache construction skipped any run of continuation bytes, including malformed runs. This could skip beyond the requested offset and cause underflow/index failures. | Align only inside a validated UTF-8 sequence. Raw invalid bytes remain replacement characters. Test every byte offset in both directions through valid Unicode, long invalid continuation runs, and boundary-crossing sequences. |
| Search after undo branches | Native cached chunks used the buffer generation, which undo can restore and a new branch can reuse. Refreshing at EOF also left an old chunk available. | Explicitly invalidate native chunks whenever the search text is refreshed, including empty/EOF refreshes. Existing branch regression and repeated edit/undo search tests pass. |
| Unicode case folding | A second native conversion call after buffer overflow retained the error status, so expanding Unicode folds could fall back to ASCII handling. | Clear the native status before retry. Repeated U+0390 expansion proves the retry produces the complete folded key. |
| Wrapped tabs | A tab wider than a narrow viewport exceeded the permitted cursor column; wrapping a word suffix containing a wide tab could still overflow after moving the suffix. | Clip visual tab width while preserving its logical tab stop, and hard-wrap suffixes that still cannot fit. Look-ahead uses the same widths and rules. |
| Wrapping look-ahead at EOF | Look-ahead failed to advance its independent byte offset. At EOF it could repeatedly count the final glyph and manufacture a wrapped row. | Track forward progress separately from the requested cursor position. Deterministic randomized edits found the failure. |
| Reflow and undo layout | Turning wrap off at a logical line start could preserve the previous wrapped row. Undo restored historical visual-line counts after viewport/tab changes. Ambiguous cached wrap anchors also produced incorrect edited-line height deltas. | Clear wrapped coordinates when disabling wrap; record the history entry's layout settings and reflow when they differ; measure edited wrapped lines from consistent logical-line origins. Wrapped undo restores the cursor from a fresh measurement. |
| Unicode width setting | A safe public setter and measurements accessed an unsynchronized mutable global. | Use an atomic width value clamped to the supported one/two-cell range. |

## Reproducible stress coverage

The ordinary library suite includes eight fixed seeds with 400 operations each. A separate ignored test executes 128 fixed seeds with 1,000 operations each. The random generator is deterministic, without a new dependency. Failures report seed, step, bytes and layout; `OGEDIT_STRESS_TRACE=1` enables a detailed operation trace.

The oracle maintains independent byte vectors and undo/redo snapshots. It exercises exact byte replacements, reversed/overlapping/duplicate multi-selections, multiple insertions, empty replacements, Unicode, CRLF and LF, tabs, tiny viewports, wrap and tab-size changes, extreme offsets, undo, redo and edits that discard redo branches. Every step checks bytes, line counts, measured visual bounds and cursor/selection bounds. It subsequently undoes and redoes each surviving branch and compares every intermediate state.

Additional tests cover:

- Invalid, overflowing, overlapping and partial-grapheme edit ranges, plus capacity rejection, preserving content, selection and history.
- 256 malformed-byte documents, repeatedly replacing, navigating, rendering with syntax highlighting, undoing and redoing.
- 128 native search lifecycles with buffer moves, reloads and edits.
- Native UTF-8/UTF-16 cache boundaries, invalid regex syntax and repeated construction/destruction.
- Parallel ICU initialization and per-thread service cleanup, with expansion-heavy Unicode folds.
- Syntax cache viewport/revision churn across a large Unicode source plus malformed bytes; checkpoint count and heap budget remain within their existing limits, and a new empty revision drops the cached states.
- Narrow wrapped tabs across widths 2–8 and tab stops 1–8, then viewport changes and undo/redo.

Before sanitizer validation, the complete compiled library test executable passed **126 tests**, including all ignored ICU and long stress tests, in 2.18 seconds. The transaction tests account for **131,200 generated operations**, plus their final undo/redo walks. This count is a checkpoint for this subreview; later combined suite totals can be higher.

```powershell
cargo test --locked --lib -- --include-ignored
# Just the long deterministic buffer corpus:
cargo test --locked --lib stress_randomized_transactions_long -- --ignored
```

## Ownership and bounded retention

- Buffer storage, native search providers and native regex objects now have an explicit destruction order. Thread-local native services close at thread exit.
- The immutable ICU function table, loaded libraries and encoding catalog are intentionally process-lifetime state. The encoding catalog is initialized once; it is not allocated on every dialog/search.
- Syntax lexical checkpoints already have count and byte budgets. Reusable vectors may retain their peak capacity while the document is open; closing the document releases their owners.
- Undo intentionally retains deleted/added content. Its existing limit is 1,000 entries, with complete transactions kept intact; there is no byte-budget cap. A large transaction can therefore retain substantial memory. This review did not silently shorten the user's undo history and does not classify reachable undo content as a leak.
- These tests are evidence against the exercised failures, not proof that arbitrary native code or every possible edit sequence is memory-safe. Sanitizer and browser measurements must be reported with their own coverage and limitations.
