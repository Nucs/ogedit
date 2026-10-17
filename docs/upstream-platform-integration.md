# Upstream platform, input and Unicode integration

Compared the fork base `82cc84a` with Microsoft Edit `cb6de0a8966aecd01d19a3541dab1829d0e679ca` on 2026-10-17. Sources were read from the fetched Git objects and the read-only extraction in `target/upstream-audit/upstream`. This is the platform portion of the combined upstream integration; application and buffer dispositions are documented separately.

The code retains OGEdit's stable ICU provider addresses, initialized native caches, thread-local native service cleanup, atomic callback flags and checked arena ownership from the memory audit. Porting an upstream file wholesale over those fixes would regress the fork.

## Commit dispositions

Each row records the portion touching this review's files. A structural/refactoring disposition does not imply that the application portions of that commit were skipped.

| Upstream commit | Disposition in OGEdit |
| --- | --- |
| `5f284a1` Move arena/helpers into a crate | Layout-only boundary retained in OGEdit's existing modules. No behavior depends on creating an intermediate crate. |
| `3df9e7c` Linux issues | UTF-8 X10 coordinates now read complete characters, including across input chunks. Signal handler pointer cast was already present. |
| `dbd7465` Terminal.app Clear Dark colors | Contrast threshold now uses the palette's measured lightness midpoint, with dark/light endpoints sorted. Palette changes also invalidate existing contrast-cache entries. |
| `d0db071` Save into missing parents | Windows not-found classification now recognizes `ERROR_PATH_NOT_FOUND` as well as `ERROR_FILE_NOT_FOUND`; directory creation belongs to the application integration. |
| `ad8415f` Error handling cleanup | Retained OGEdit's public `apperr` representation and explicit legacy-console error text. Equivalent OS failures remain reported; changing all signatures to `io::Error` is not required for the fixes. |
| `e5a92eb` Clippy warning | Applied the ICU provider's while-let iteration cleanup. |
| `524b22c` macOS warnings | The fork still uses `CStr` in its retained native error formatter, so removing that import would break it. Cross-platform checking verifies the retained import. |
| `14c1def` Nightly feature reduction | Toolchain/allocator migration is outside these ports. OGEdit's validated allocator implementation remains in place; no behavior change is needed in measurement. |
| `6c79a1d` allocator_api removal | Retained checked OGEdit arenas and `MemsetSafe` instead of adopting new collections wholesale. The generic memset fallback is now implemented with `fill` for other explicitly safe element sizes. |
| `ff45b14` UTF-16/UTF-8 utilities | Superseded by the retained collection/layout design. This commit moves `Utf8Chars` unchanged into stdext and optimizes BVec UTF-8-to-UTF-16 helpers. OGEdit keeps Unicode in its main crate and uses standard `Vec` plus `str::encode_utf16` for regex patterns. Later decoder fixes are tracked under their own commits; native Windows input conversion performs the opposite UTF-16-to-UTF-8 operation and is not a substitute for this helper. |
| `baa3cb0` Natural filename sort | Thread-local ICU collators enable Unicode normalization and numeric collation: `file2` sorts before `file10`. Failed native setup still closes the handle. |
| `fa709ba` Syntax integration | Added bold and strikethrough framebuffer attributes and independent SGR enable/disable transitions, retaining OGEdit’s existing italic/underline bit values. Syntax definitions/integration are reviewed by the application and core portions. |
| `05452ab` Complete initial LSH implementation | Markup rendering now has the full attribute set required by upstream heading/emphasis/strikethrough styles. Root/core map syntax spans to these attributes; framebuffer regressions verify combined styles, selective clearing and unchanged-frame suppression. |
| `7c0b3d1` Pointer-cast lint | Applicable SIMD/VT source updates include current casts. No behavioral rewrite of already-audited native pointer code was necessary. |
| `d887414` Horizontal touchpad scrolling | Mouse decoding supports horizontal wheel directions. |
| `9e7b938` Redirected stdin | Both OS implementations expose `reopen_stdin_if_redirected`, returning the original redirected stream before raw-mode setup. The application calls it before switching modes. Failed terminal reopen preserves the existing handle. |
| `de3db9b` Xterm decoding | Shared X10/SGR decoding recognizes releases, button motion and supported wheel codes. `InputMouse.drag` distinguishes motion from clicks. Malformed X10 coordinates are additionally rejected without arithmetic underflow or losing following text. |
| `2bb7c10` ASCII comparison stability | Integrated folded comparison and additionally repaired a remaining upstream prefix/case cycle (`a` < `aa` < `Ab` < `a`). The entire folded strings, including length, now compare before the original-byte tie break. An 89-string corpus verifies antisymmetry, equality and transitivity across all triples. |
| `d14e24b` Zero-width regex | ICU's immutable `OnceLock` table was already present with stronger thread-local service ownership. Buffer-level search/replace changes are handled by the core integration. |
| `f7de351` Locale selection | OS locale list semantics retained, including underscore-to-hyphen normalization. Locale resolution and generated language enums are handled by the application/build integration. |
| `737450c` Enable stdarch_loongarch in stdext | Already covered: OGEdit's single `src/lib.rs` has the `loongarch64`-gated `stdarch_loongarch` feature and `clippy::incompatible_msrv` allowance. The upstream fix addresses a missing feature gate in the temporary split stdext crate, which OGEdit does not have. No source change required. |
| `ed52eff` Copy u16 iterator elements | Not applicable to the retained implementation: upstream changed only `BString::push_utf16_lossy` from `.cloned()` to `.copied()`. OGEdit uses native `WideCharToMultiByte` into initialized spare capacity and has neither that BString method nor a cloned u16 decoder iterator. This is a style/intent cleanup with identical behavior for Copy u16 values. No source change required. |
| `c5d5455` LoongArch SIMD | Adopted immediate LF compares and simplified SIMD accumulators in forward/backward line scans. |
| `b5303d7` Ctrl+Space | NUL maps to Ctrl+Space. Windows preserves zero-character synthesized console records whose scan code is zero. |
| `33ae9b0` memchr2 optimizer assertion | Preserved the bounded return invariant with a debug assertion. The unchecked optimizer hint alone adds no user behavior and is intentionally not introduced into this hardening-focused fork. |
| `11f0848` Big-endian colors | Packed RGBA conversions are now independent of host byte order. Existing OGEdit `from_be`/`from_le` callers stay compatible; explicit `from_rgba`/`to_rgba` APIs were added. Channel extraction and round trips are tested. |
| `a6eeb20` Modifier wheel behavior | Shift maps vertical wheel events to horizontal ticks; decoder reports one tick. TUI applies the normal/Alt-fast distance. |
| `adf09b8` Sanitize terminal output | Ported control/invalid-UTF-8 visualization with borrowed clean-text fast path. Every public framebuffer text insertion sanitizes bytes and highlights replacements. Raw filename ESC/OSC content cannot be emitted as a terminal command through that path. |
| `310d6e6` Empty frame panic | Native window sizes now travel separately from keyboard text; startup queries the terminal size and uses a valid fallback, while resize events cannot be forged by CSI input text. Zero-sized flips retain the previous viewport; empty initial frame rendering is safe. Bitmap row iterators also use a nonzero chunk size. |
| `861419d` x86 SIMD fallback | SSE2 is checked before selection; non-SSE2 x86 uses the scalar fill implementation. |
| `c470ca5` Reintegrate stdext | OGEdit already keeps these modules in its main crate. Functional changes were compared independent of path movement. |
| `731fc9a` Decode final UTF-8 character | Added `utf8_decode_last`, including malformed/truncated-tail tests. |
| `c04475c` Grapheme boundaries | Removed obsolete measurement assumptions that document chunks end at grapheme boundaries. Core owns the corresponding document and CRLF navigation changes. |
| `6118429` Undo/redo issues | Added `graphemes_may_join` and its boundary tests for use by the core transaction changes. |
| `cc1b35e` Buffer tests | Added platform anonymous temporary-file helper `memfd`; Windows uses delete-on-close, Linux uses memfd, other Unix uses a duplicated temporary stream descriptor. Core imports the buffer test corpus. |
| `fdc3d14` Unix write EINTR | Output advances the actual unwritten slice, handles zero writes, retries interruption and waits for writable output on EAGAIN/EWOULDBLOCK. Poll errors/hangups terminate output cleanly. |
| `01df252` Unix file identity | Identity uses metadata's device/inode; directories and unreadable files no longer need to be opened for reading merely to identify them. |
| `a820b05` Empty LANGUAGE | Delimiter-only/empty language overrides no longer suppress fallback. `LC_MESSAGES` participates between `LC_ALL` and `LANG`. |
| `67543cb` termios EINTR | Raw-mode setup/restoration retries interrupted `tcsetattr`; resets VMIN/VTIME; disables IXOFF and Linux IUCLC; enables CREAD. Original descriptor flags are tracked separately from current nonblocking flags so cleanup restores the real original state. Reopened Unix terminal descriptor is closed during teardown. |

## Additional integration hardening

- The latest upstream ASCII fallback still admitted a prefix/case ordering cycle. Full folded-string comparison followed by an original-byte tie break now defines a total order, including invalid-byte filename keys.
- Four SIMD dispatch tables used unsynchronized `static mut` function pointers in both the old fork and latest upstream. OGEdit's background workers can call these functions concurrently. Each dispatch now selects its implementation through an immutable `OnceLock`.
- Interrupted Unix input polling now retries or yields a pending resize rather than treating SIGWINCH/EINTR as EOF.
- Public UTF-8 iterator size hints use saturating subtraction when callers seek beyond EOF.
- X10 coordinates are range-checked before subtraction/casting; incomplete sequences remain buffered, malformed completed sequences allow following text to continue.
- Palette changes clear the contrast cache; upstream's midpoint fix alone could otherwise leave an old cached contrast color active.

## Regression evidence and limits

Focused regression tests are included alongside the affected code:

- `input::upstream_mouse_tests`: both wheel axes, Shift routing, release/motion/unsupported buttons, combined modifiers, UTF-8 coordinates split across reads, malformed coordinates, rejection of forged resize sequences and Ctrl+Space.
- `unicode::sanitize::tests`: C0/C1, invalid UTF-8, truncated encodings, legitimate U+FFFD, output byte ranges, and clean borrowed strings.
- `unicode::utf8::tests` and `unicode::tests`: scalar decoding and grapheme-seam predicates imported from current upstream.
- `framebuffer::upstream_regression_tests`: hostile OSC-looking text, zero-size initial/subsequent frames, midpoint contrast, palette-cache invalidation, independent markup attribute transitions and masked clearing.
- `icu::tests::natural_filename_sorting_and_normalized_unicode`: numeric filenames, canonical Unicode equivalence, ASCII fallback ordering. `ascii_fallback_orders_case_prefixes_transitively` verifies 89 strings and 704,969 triples, including prefixes, mixed case, NUL and non-ASCII bytes.
- `oklab::tests::packed_color_channels_are_independent_of_host_endianness`: packed value/channel/format round trips.
- `simd::dispatch_tests`: 16 threads × 128 rounds of simultaneous forward/backward scanning, two-byte search and guarded fills.
- Windows missing-file/missing-parent error classification; Unix locale fallbacks and metadata-only file identity.

The coordinating agent runs the combined Cargo, sanitizer and browser suites. Platform checks on this Windows host do not constitute native Linux/macOS terminal execution. LoongArch immediate instructions and big-endian runtime behavior require their respective hardware/emulation to execute; retained tests and cross compilation cover only what is explicitly recorded in the combined validation report. No native desktop automation was used.


## Major-release Linux runtime preflight

The release preflight found an additional packaging/startup defect: the default Unix binary tried only `libicuuc.so` and `libicui18n.so`, names usually supplied by development packages. A clean installation with only ICU runtime SONAMEs such as `libicuuc.so.74` could not initialize the editor.

`build/main.rs` now explicitly records whether both library names use defaults. For that default configuration, the Unix loader first tries the original aliases and then searches a bounded set of matching major-version SONAME pairs through the native dynamic loader. It never combines core and i18n from different candidate versions. Explicit compile-time library-name overrides remain exact and never fall back silently. If loading the second library fails, the first library's loader reference is closed before continuing.

Symbol suffix detection now probes the documented public `u_errorName` export and its versioned forms. It no longer depends on an incidental exported C++ destructor or symlink naming.

Four Unix regressions cover matching version pairs, exact custom overrides/default alias preference, bounded failed lookup, and actual runtime public-symbol resolution. On the available Ubuntu WSL environment, only ICU 74 runtime libraries were installed and both unversioned development aliases were absent. The native run passed all **198 library tests**, including these four cases, plus three LSH library tests. Application tests exposed a separate external-file-change detection issue, handled by the coordinating review. This checkpoint does not imply that the final release matrix or installers have completed.

The release workflow's package dependency declarations and AppImage library bundling are coordinated separately. Versioned runtime lookup removes the development-symlink requirement; it does not make a Linux binary independent of ICU runtime installation or its glibc baseline.
