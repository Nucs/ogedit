# Upstream application and terminal UI integration

Compared fork point `82cc84a610ded54d57f956285a8c4307c74bf364` with Microsoft Edit `cb6de0a8966aecd01d19a3541dab1829d0e679ca` on 2026-10-17. This is the application/UI slice of the complete upstream ledger. Core buffer, platform, LSH, packaging and build changes are covered separately.

Existing OGEdit modernization, recovery, safe saves, configurable shortcuts, and memory-audit fixes were preserved. Changes were adapted to the fork's APIs instead of replacing its files with upstream versions.

| Upstream commit | Disposition in OGEdit |
|---|---|
| `1601c3f` Arabic translation | Merged the latest translation catalogue. |
| `ca661a2` Estonian translation | Merged. |
| `d71e94b` Korean translation corrections | Merged. |
| `364579a` Indonesian translation | Merged. |
| `dbd7465` macOS terminal color handling | Application code remains compatible with the platform/framebuffer fix; zero-size startup handling also incorporates the later `310d6e6` fix. |
| `48c2ab2` platform newline for one-line files | Buffer integration handles this; configured OGEdit defaults remain available. |
| `d0db071` create missing save parents | Added to the atomic staged-save path after conflict and target validation. Failed writes leave no partial destination or staging file. |
| `ad8415f` error cleanup and unstable sorting | File and encoding lists now use unstable sorting. Retained OGEdit's error type because its recovery/conflict/unsupported-save errors are integrated throughout the fork; equivalent OS errors still reach the UI. |
| `719a478` copyright wording | Updated the About dialog to the upstream wording. |
| `14c1def` remove nightly linked-list traversal | Replaced document MRU storage with safe `VecDeque` removal/promotion and removed the `linked_list_cursors` feature. Retained the fork's already tested memory-safe allocation model. |
| `baa3cb0` natural filename sorting | File lists and the bounded top-five filename completions use numeric ICU ordering within directory/file groups. Completion ties use original names for deterministic ordering; both cached and scanned entries follow the same comparator. ICU settings and fallback ordering are integrated in the platform slice. |
| `8ad89c8`, `0bfb7a3`, `fa709ba`, `05452ab` LSH | Existing bounded syntax cache retained; root integrates current compiler/runtime/definitions. Added the missing status-bar language picker, automatic detection, per-document overrides and Plain Text mode. |
| `d1908b0` preferences and file associations | Added `settings.json`, File → Preferences, ordered `files.associations` glob overrides, JSONC validation, and reload on save. Existing rich `state.json` configuration remains separate. |
| `9e7b938` redirected stdin | Read redirected input after command-line early exits and before raw-mode operation, using the reopened native input handle. |
| `c8a9787` scrolling inside modals | Hit-test the topmost hovered node and its ancestry. Covered editors cannot consume a modal's wheel input. |
| `24e2d79` multiline paste into single-line fields | Ctrl+V, Shift+Insert and bracketed paste use the single-line buffer API in edit fields. The clipboard retains its full contents. |
| `e5f0c45` search option focus | Changing search options no longer forces focus back into the search field. Existing platform/configurable final-newline defaults retained. |
| `c481b71` Turkish translation corrections | Merged. |
| `58408a7` Traditional Chinese translation corrections | Merged. |
| `f7de351` locale selection | Runtime follows the normalized hyphenated catalogue; build/sys adapters normalize locale input. |
| `3afb4f7` Serbian Cyrillic and Latin | Merged both language catalogues and aliases. |
| `9642f7f` Polish translation corrections | Merged. |
| `428ef55` Chinese region/script fallback | Merged region aliases and regression checks for CN/SG/TW/HK/MO/Hant. |
| `10cbfcc` settings before CLI files | Preferences load during state initialization before any argument files open, so custom language associations apply immediately. |
| `843e48f` explicit CLI `--goto` | CLI paths are literal by default. `-g`/`--goto` applies location parsing only to the next filename. GUI Open File retains the fork's quoted-path and location-suffix convenience. |
| `3e20615` negative Go To lines | CLI and Go To accept negative lines, counted from the last logical line. Coordinates use saturating arithmetic; negative counting applies only to lines. |
| `0a6f8a3` line-number gutter selection | Already superseded by the fork's tested whole-line dragging, Shift extension, wrapped-line support and fixed logical anchors. Preserved that implementation. |
| `11f0848` endian-independent colors | Terminal query parsing and TUI diagnostics now use packed RGBA APIs alongside the platform color fix. |
| `a6eeb20` modified wheel scrolling | Input parser produces one-step ticks; TUI applies horizontal/vertical scale and Alt acceleration. Shift-to-horizontal mapping is integrated in the input slice. |
| `721d848` horizontal drag near viewport edges | Autoscroll zones include the original press position, so horizontal drags on the top/bottom row do not trigger vertical drift. Clamped speed lookup to its final valid index. |
| `93dfdf9` compatible checkbox glyphs | Replaced ballot-box emoji with the upstream square markers. |
| `9b34f18` click a menu again to close it | Added a menu-toggle latch armed after the opening press ends. |
| `d6ba99b` Chinese translation updates | Merged. |
| `adf09b8` terminal text sanitization | Terminal title uses the shared C0/C1/invalid-byte sanitizer; framebuffer and buffer integration handle other output. Removed the title's old C0-only helper. |
| `01b3bc6` Norwegian translations | Merged Bokmål/Nynorsk and the `no` alias. |
| `310d6e6` zero-sized frame panic | TUI starts at 80×24 with an immediate initial read. Native window size and resize events are delivered separately from terminal text. Zero-sized reports preserve a valid viewport; terminal text cannot forge a resize. |
| `650edc1` Backspace unindents leading whitespace | Routed editor Backspace/Ctrl+Backspace and recorded macro deletion to the new buffer API. Single-line input fields retain ordinary backward deletion. |
| `8c97fa2` Tab ancestry check | Focus traversal only runs in an actual ancestor of the focused node, preventing unrelated focus wells from borrowing the wrong subtree. |

## Preferences and intentional compatibility choices

User preferences live in `OGEDIT_DATA_DIR/settings.json` (normally `~/.ogedit/settings.json`). Automatically saved recent files, keyboard shortcuts and project folders remain in the existing `state.json`. This prevents an automatic recent-file update from overwriting a preferences document being edited.

```jsonc
{
  "files.associations": {
    "**/special/*.txt": "rust",
    "*.custom": "python",
    "*.generated": "plaintext"
  }
}
```

Rules use upstream LSH glob semantics and retain file order; the first matching rule wins. Patterns without `/` receive the upstream `**/` prefix. A per-document language choice overrides preferences; Auto Detect restores preference/path detection. Invalid preference reloads retain the previous working associations and leave the original file untouched. Preferences are bounded to 1 MiB, 1,024 associations, 4,096 bytes per pattern and 128 nesting levels.

CLI usage is now `ogedit --goto file.txt:-1:4`. This opens at character 4 of the final logical line. `ogedit file.txt:10` now names that literal file, as upstream requires. Open File and the file palette retain the earlier requested location-suffix convenience, with an existing literal filename taking precedence. Quoted Windows paths and either directory separator are preserved.

Directory entries remain grouped before files, matching OGEdit's existing picker. Sorting within each group is natural. Safe saves retain permissions, metadata checks, atomic replacement, watcher registration and recovery cleanup; adding parent creation does not replace those safeguards.

The latest upstream catalogue was merged while preserving all nine OGEdit-only translation sections (`EditDuplicate`, `FileReloadFromDisk`, and the seven reload-dialog sections). Language initialization uses `OnceLock`, avoiding a mutable global without changing startup locale selection.

## Regression coverage

Added regression cases cover modal-wheel isolation, first-frame and zero-size rendering, menu click toggling, bracketed multiline paste into single-line fields, horizontal drag on the bottom row, nested-parent save success and interrupted staging, negative/extreme Go To values, literal Unix colon filenames, ordered language preferences, malformed/oversized preference rejection, failure-preserving reload, and Chinese region aliases.

Existing document MRU, quoted/mixed-separator path, Save As watch cleanup, recovery, multicursor/macro, gutter-selection and memory ownership regressions remain enabled. Exact integrated test and browser results are recorded in the parent upstream integration report rather than duplicated here before the complete suite runs.
