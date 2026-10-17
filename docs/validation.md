# Workbench validation — 2026-10-17

Historical validation snapshots from the initial modernization. Later integration results are in [the upstream audit](upstream-integration.md); the public release summary is [OGEdit 2.0.0](releases/2.0.0.md).

Validated on Windows x64 with the pinned Rust toolchain. Manual checks ran the real
OGEdit executable directly in ConPTY, rendered through xterm.js in a local browser.
Each run used an isolated data directory and disposable fixtures. No shell was run
inside the terminal, and no user configuration or documents were used as fixtures.

## Automated checks

- 75 library tests and 74 application tests passed with all features.
- Both explicitly ignored ICU tests passed serially.
- Three documentation tests passed.
- Clippy passed for all features and all targets with warnings denied.
- Linux x86_64 and macOS ARM64 compile checks passed. Runtime QA on those systems
  remains outstanding.
- The Windows release builds with the repository's static-runtime release profile.

Regression coverage includes atomic save failures and concurrent changes, NTFS
alternate streams, read-only and hard-linked files, watcher behavior after atomic
replacement, damaged recovery snapshots and live-instance isolation, recovery after
undo branches, graphemes and CRLF boundaries, overlapping selections, capacity
failure rollback, long macro undo groups, clipboard metadata, JSON number precision,
quoted multiline CSV, UTF-8 BOM offsets, ignored paths, stale results, cancellation,
palette query races, and bounded syntax/diff rendering.

## Manual checks

| Workflow | Observed result |
| --- | --- |
| F2 palette and Ctrl+P workspace switch | Fuzzy results filter and open the selected file. |
| Search result edit | Source buffer changes, results refresh, disk remains unchanged until save. |
| Search after source undo | Stale edit is rejected; original content remains intact. |
| CSV cell with an embedded comma | Required quoting is added; one undo restores the original source. |
| JSONC boolean edit | Invalid `tru` is rejected; `false` saves while preserving comments and spacing. |
| Log field edit and cancellation | Field updates the source; Escape discards the subsequent draft. |
| Three matching selections | Typing replaces all three; one undo restores all three. |
| Keyboard rectangular selection | Three selected columns replace independently. |
| Paused Alt-click and Alt-drag | Extra cursor survives idle frames; dragging replaces three distinct ranges. |
| Named macro | Recording persists to JSON; replay works and is one undo step. |
| Forced process termination | Restart offers the snapshot; Enter restores exact unsaved text as a separate copy. |
| External modification then Save | Comparison opens with both versions; the external file is not overwritten. |
| Syntax and Ctrl+] | Rust tokens are colored; matching skips string braces and jumps to the closing brace. |
| Terminal resize | Core panels remain usable at 110×32, 80×24, 60×15, and 40×12. |

Manual QA found and drove fixes for a swallowed cell-editor Escape key, recovery
list focus, stale status notices, and idle frames resetting an Alt-drag anchor.
Independent review also found and drove fixes for macro transaction boundaries,
large undo groups, rollback preserving redo, newline conversion history, and
syntax cache resource bounds.

The [workbench guide](workbench.md) documents resource limits and save semantics.
These checks establish the tested behaviors above; they are not a claim that every
terminal emulator or filesystem has been exercised.

## Follow-up: insertion cursors and file opening

The updated suite passes 82 library tests, 80 application tests, both explicitly
ignored ICU tests, and three documentation tests (167 total). Clippy remains clean,
and Windows release plus Linux/macOS compile checks pass.

Browser-only editor QA verified:

- Alt-click shows an underlined insertion point without a selected background.
  Typing at three positions inserted text while retaining the adjacent characters.
- A quoted absolute Windows path opens with one Enter (shown as `"K:\projects\sample\README.md"`; the original machine-specific path is omitted).
- Mixed `/` and `\` separators resolve to the same document.
- Ctrl+P accepts a quoted absolute path outside the workspace.
- Browse File appears directly below Open File, and Open File includes Browse.

The Windows native browser uses the Common Item Dialog on an STA worker. Its
starting-folder preparation, real Shell path parsing, Unicode paths, and
cancellation/error result handling have automated coverage. Native dialog creation
was observed; choosing and cancelling a file in the OS dialog were not fully
manually exercised. Desktop automation was stopped at the user's request, and
subsequent UI checks used browser tools only.

## Follow-up: live Open File path preview

Entering an existing file path now updates the folder listing, highlights the
matching file, and scrolls it into view while File name keeps focus. Browser-only
QA against the Windows release verified quoted paths, mixed separators, relative
paths, switching to another filename in the displayed folder, and a file created
after the listing was loaded. Enter opens the expected file; Tab and arrow keys
can select a different file without retaining the old path anchor.

The suite passes 82 library and 84 application tests (two ICU tests remain ignored
in this run). Four new regression tests cover preview path resolution, relative
anchors, partial paths, and replacing an absolute path with a basename. Clippy
passes with warnings denied, and Linux x86_64/macOS ARM64 compile checks pass.
The installed Windows executable was updated and verified against the release
build's SHA-256 hash.

## Legacy fork audit

The subsequent [legacy audit](legacy-audit.md) maps all 46 fork commits and records
additional repairs, baseline/current browser comparisons, and the final integrated
197 Rust tests plus 18 delivery regressions. Its final results supersede the
earlier counts above; native package lifecycle limits are listed explicitly.
