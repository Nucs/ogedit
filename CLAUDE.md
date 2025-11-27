# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OGEdit is a terminal-based text editor that pays homage to MS-DOS Editor, built in Rust with a strong focus on small binary size and high performance. This is a fork/customization of Microsoft's Edit project. The editor can handle large files (1GB+) efficiently through SIMD-optimized operations and a unique architecture that avoids tracking line breaks in memory.

**Binary Names:**
- Primary: `ogedit` (or `ogedit.exe` on Windows)
- Alternative: `ogmsedit` (to avoid potential naming conflicts)

**Configuration:**
- Global settings are stored in `~/.ogedit/state.json`
- Configuration is automatically loaded on startup and saved when settings change
- Current configurable settings:
  - `word_wrap`: Whether Word Wrap is enabled by default for new documents
- **Corruption Handling:**
  - If `state.json` is corrupted or contains invalid JSON, it will be backed up to `state.json.backup`
  - A fresh configuration file with default values will be automatically created
  - Unknown fields in the JSON are ignored (forward compatibility)
  - Invalid values for known fields trigger corruption recovery

## Build and Test Commands

### Building

**Development builds:**
```bash
cargo build
```

**Release builds:**
- For Rust 1.90 or earlier:
  ```bash
  cargo build --config .cargo/release.toml --release
  ```
- For Rust nightly (1.91+):
  ```bash
  cargo build --config .cargo/release-nightly.toml --release
  ```

The release build configuration is highly optimized for binary size reduction using:
- `opt-level = "s"` (size optimization)
- LTO enabled
- `panic = "abort"` with `panic_immediate_abort`
- Symbol stripping
- Custom MSVC linker flags to avoid vcruntime140.dll dependency

### Testing

**Run all tests:**
```bash
cargo test
```

**Run ICU-related tests (requires proper ICU configuration):**
```bash
cargo test -- --ignored
```

### Running

```bash
cargo run -- [files...]
```

### Benchmarking

```bash
cargo bench
```

## Toolchain Setup and Prerequisites

### Initial Setup (Windows)

This project requires Rust nightly with specific components and Windows build tools. Here's the complete setup process:

#### 1. Install Rust

```bash
# Download and install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable

# Add cargo to PATH (or restart shell)
export PATH="$HOME/.cargo/bin:$PATH"
```

#### 2. Install Nightly Toolchain

The project uses `rust-toolchain.toml` to specify nightly, but you need to ensure it's installed:

```bash
# Install nightly toolchain
rustup toolchain install nightly

# Add rust-src component (required for release builds with build-std)
rustup component add rust-src --toolchain nightly-x86_64-pc-windows-msvc
```

#### 3. Install Visual Studio Build Tools (Windows MSVC target)

The project requires MSVC toolchain on Windows for linking and the winresource build dependency:

**Download and install:**
```bash
# Download VS Build Tools installer
curl -L "https://aka.ms/vs/17/release/vs_buildtools.exe" -o vs_buildtools.exe

# Install with C++ workload (requires admin privileges)
./vs_buildtools.exe --quiet --wait --norestart --nocache \
  --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
```

This installs:
- MSVC compiler and linker (link.exe)
- Windows SDK
- C++ build tools

**Why it's needed:**
- The `winresource` build dependency (in `build/main.rs`) requires Windows resource compiler
- MSVC linker is needed for the MSVC target (default on Windows)
- Custom linker flags in `.cargo/release-nightly.toml` require MSVC toolchain

#### 4. Verify Installation

```bash
# Check Rust installation
rustc --version  # Should show nightly version
cargo --version

# Verify rust-src is installed
rustup component list --toolchain nightly | grep rust-src
```

### Common Issues and Solutions

#### Issue: "link.exe: command not found" or wrong link.exe

**Symptom:**
```
error: linking with `link.exe` failed: exit code: 1
note: link: extra operand '...'
```

**Cause:** Git's `/usr/bin/link.exe` is in PATH before MSVC's link.exe

**Solution:** Install Visual Studio Build Tools (see step 3 above). The Rust toolchain will automatically find the MSVC linker once installed.

#### Issue: "Missing manifest in toolchain 'nightly-x86_64-pc-windows-msvc'"

**Symptom:** Corrupted nightly installation

**Solution:**
```bash
# Remove corrupted toolchain
rm -rf "$HOME/.rustup/toolchains/nightly-x86_64-pc-windows-msvc"

# Reinstall
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly-x86_64-pc-windows-msvc
```

#### Issue: Release build fails with "unable to build with the standard library"

**Symptom:**
```
error: "...\\Cargo.lock" does not exist, unable to build with the standard library
try: rustup component add rust-src --toolchain nightly-x86_64-pc-windows-msvc
```

**Solution:** Install rust-src component (see step 2 above)

#### Issue: "Access is denied" when running executable

**Symptom:** Built executable cannot run from bash/PowerShell with exit code 5

**Solution:** This is a Windows security restriction. Options:
1. Run from cmd.exe or Windows Explorer directly
2. Configure Windows Defender to allow the executable
3. The tests via `cargo test` work fine and validate the build

### Alternative: GNU Toolchain (Advanced)

If you cannot install Visual Studio Build Tools, you can use the GNU toolchain:

```bash
# Install GNU nightly toolchain
rustup toolchain install nightly-x86_64-pc-windows-gnu

# Build with GNU toolchain
cargo +nightly-x86_64-pc-windows-gnu build
```

**Note:** This may require additional configuration and the `build/main.rs` script might need modification for the `winresource` dependency.

### Build Artifacts

After successful builds, executables are located at:
- Debug: `target/debug/ogedit.exe` (~1.1 MB with debug symbols)
- Release: `target/release/ogedit.exe` (~246 KB, highly optimized)

The release build is significantly smaller due to:
- `-Zbuild-std-features=panic_immediate_abort` (removes panic infrastructure)
- `opt-level = "s"` (optimize for size)
- LTO (link-time optimization)
- Symbol stripping

## Core Architecture

The codebase has a unique architecture centered around performance optimization:

### Text Buffer Design (src/buffer/)

**Critical design principle:** The text buffer does NOT track line breaks in memory. This permeates the entire codebase.

- Only the current cursor position is kept as state
- Navigation uses O(n) seeking through the document to count line breaks
- This enables exceptional performance through SIMD operations:
  - `src/simd/memchr2.rs`: Find next/previous line breaks at >100GB/s
  - `src/unicode/mod.rs`: UTF-8 iteration with U+FFFD replacement at ~4GB/s
  - `src/unicode/measurement.rs`: Grapheme cluster segmentation via `MeasurementConfig` at ~600MB/s
- Without word-wrap: memchr2 enables navigating 1GB files like 1MB files
- With word-wrap: still smooth thanks to optimized `MeasurementConfig`

### Key Modules

- **src/framebuffer.rs**: Implements a video game-style framebuffer
  - Draw UI to intermediate buffer, accumulating changes and handling color blending
  - Compare with previous frame and only send deltas to terminal

- **src/tui.rs**: Immediate mode UI implementation
  - Read the module documentation for architecture overview

- **src/vt.rs**: VT (Virtual Terminal) parser for terminal escape sequences

- **src/sys/**: Platform abstraction layer
  - `sys/windows.rs`: Windows-specific code
  - `sys/unix.rs`: Unix-specific code

- **src/bin/edit/**: Main application that ties everything together
  - ~90% UI code and business logic
  - `main.rs`: Entry point with `setup_terminal` function containing VT logic
  - `documents.rs`: Document management
  - `draw_*.rs`: Rendering modules for editor, filepicker, menubar, statusbar
  - `localization.rs`: i18n support
  - `state.rs`: Application state

### Performance-Critical Code Paths

When making changes, be aware these areas are heavily optimized:
- `src/simd/`: SIMD implementations for string operations
- `src/unicode/`: UTF-8 and grapheme cluster handling
- `src/buffer/`: Core text buffer operations

## Internationalization (i18n)

All translations are in `i18n/edit.toml`. At build time, `build/main.rs` processes this file and generates Rust code in `$OUT_DIR/i18n_edit.rs`.

**To limit languages in build:**
```bash
EDIT_CFG_LANGUAGES="en,de,fr" cargo build
```

Available languages are listed in `i18n/edit.toml` under `__default__`.

## ICU Configuration

The project optionally depends on ICU for Search and Replace functionality. Default SONAMEs:
- Windows: `icuuc.dll`, `icuin.dll`
- macOS: `libicucore.dylib` (both)
- Unix: `libicuuc.so`, `libicui18n.so`

**Configure custom ICU at build time:**
```bash
# Custom SONAME (e.g., versioned library)
EDIT_CFG_ICUUC_SONAME="libicuuc.so.76" cargo build

# Versioned exports
EDIT_CFG_ICU_RENAMING_VERSION="76" cargo build

# C++ prefixed exports (macOS default)
EDIT_CFG_ICU_CPP_EXPORTS="true" cargo build

# Auto-detect version (Unix default if no other options set)
EDIT_CFG_ICU_RENAMING_AUTO_DETECT="true" cargo build
```

## Development Guidelines

### Binary Size Priority

Keeping binary size small is a top priority. Generally:
- Do NOT add new dependencies without strong justification
- Use `#[cold]` and `#[inline]` attributes strategically
- Profile binary size impact of changes

### Nightly Features

This project requires Rust nightly and uses several unstable features:
- `allocator_api`
- `breakpoint`
- `cold_path`
- `linked_list_cursors`
- `maybe_uninit_*` features
- `stdarch_loongarch*` (for loongarch64 target)

See `src/lib.rs` for the complete list.

### Memory Management

The project uses a custom arena allocator (`src/arena/`):
- `scratch_arena()`: Temporary allocations
- `ArenaString`: Arena-allocated strings
- Scratch arena capacity: 128 MiB (32-bit), 512 MiB (64-bit)

### Terminal-Related Issues

If debugging terminal compatibility issues, check:
1. VT parser: `src/vt.rs`
2. Platform-specific code: `src/sys/windows.rs` or `src/sys/unix.rs`
3. Terminal setup: `setup_terminal()` in `src/bin/edit/main.rs`

## File Structure

```
src/
├── lib.rs                    # Library root with feature gates
├── bin/edit/                 # Main application binary
│   ├── main.rs              # Entry point and main loop
│   ├── state.rs             # Application state
│   ├── config.rs            # Global configuration (~/.ogedit/state.json)
│   ├── documents.rs         # Document management
│   ├── draw_*.rs            # UI rendering modules
│   └── localization.rs      # i18n wrapper
├── arena/                    # Custom memory allocator
├── buffer/                   # Core text buffer (no line tracking!)
│   ├── gap_buffer.rs        # Gap buffer implementation
│   ├── line_cache.rs        # Line position caching
│   └── navigation.rs        # Cursor movement
├── simd/                     # SIMD-optimized operations
│   ├── memchr2.rs           # Fast line break search
│   ├── memset.rs            # Fast memory filling
│   └── lines_*.rs           # Line iteration
├── unicode/                  # UTF-8 and grapheme handling
│   ├── utf8.rs              # UTF-8 validation and iteration
│   ├── measurement.rs       # Grapheme segmentation
│   └── tables.rs            # Unicode property tables
├── sys/                      # Platform abstraction
│   ├── windows.rs           # Windows implementation
│   └── unix.rs              # Unix implementation
├── tui.rs                    # Immediate mode UI framework
├── vt.rs                     # VT escape sequence parser
├── framebuffer.rs           # Terminal rendering backend
├── document.rs              # Document model
├── input.rs                 # Input event handling
├── clipboard.rs             # Clipboard operations
└── [other utility modules]   # helpers, path, hash, fuzzy, etc.

build/
└── main.rs                   # Build script (i18n generation, ICU config)

i18n/
└── edit.toml                 # Translation strings
```

## Testing and Validation

### Test Suite

The project has comprehensive test coverage:

```bash
# Run all tests (unit + integration + doc tests)
cargo test

# Expected output:
# - ~36 unit tests (SIMD, Unicode, buffer operations)
# - ~1 integration test (document parsing)
# - ~3 doc tests (API examples)
# Total: ~40 tests, all should pass

# Run with verbose output
cargo test -- --nocapture

# Run specific test
cargo test test_name

# Run benchmarks
cargo bench
```

### ICU Tests (Optional)

Some tests are ignored by default because they require ICU library:

```bash
# Run ICU-related tests
cargo test -- --ignored

# These tests validate:
# - ICU library loading and symbol resolution
# - String comparison with locale support
```

### Continuous Integration Notes

For CI/CD pipelines on Windows:
1. Install Visual Studio Build Tools in CI environment
2. Ensure `rust-src` component is available
3. Run `cargo test` to validate builds
4. Release builds take ~2-3 minutes (rebuilding std library)

## Running the Application

### Command Line Usage

```bash
# Open editor without file
cargo run

# Open specific file(s)
cargo run -- file1.txt file2.txt

# Show help
cargo run -- --help

# Use release build for better performance
cargo run --release -- largefile.txt
```

### Executable Distribution

After building, the standalone executable can be distributed:

```bash
# Release build location
./target/release/ogedit.exe

# The executable is statically linked (on Windows with MSVC config):
# - No vcruntime140.dll dependency (statically linked)
# - Only depends on ucrtbase.dll (Universal CRT, part of Windows 10+)
# - Self-contained: no need to ship additional DLLs
```

**Distribution Note:** Include both binary names in portable packages:
- `ogedit.exe` - Primary binary name
- `ogmsedit.exe` - Copy of the same binary (alternative name to avoid conflicts)

### Distribution Checklist

When preparing binaries for release:

1. **Build with release config:**
   ```bash
   cargo build --config .cargo/release-nightly.toml --release
   ```

2. **Verify binary size:**
   - Should be ~246 KB (Windows x86_64 MSVC)
   - Significantly smaller than typical Rust binaries due to aggressive optimizations

3. **Test on clean system:**
   - Ensure no missing DLL dependencies
   - Verify terminal compatibility

4. **Package naming conventions:**
   - Primary binary: `ogedit` or `ogedit.exe`
   - Alternative name: `ogmsedit` or `ogmsedit.exe` (to avoid potential conflicts)
   - Distribution folder: `ogedit-v{VERSION}-{PLATFORM}-{ARCH}`
   - Example: `ogedit-v1.2.1-windows-x86_64.zip`

5. **Include assets (if distributing with installer):**
   - Icon: `assets/edit.ico`
   - Manifest: `src/bin/edit/edit.exe.manifest`
   - Both binaries: `ogedit.exe` and `ogmsedit.exe` (copy of same binary)

## Performance Profiling

For performance analysis:

```bash
# Build with profiling enabled
cargo build --release --config .cargo/release-nightly.toml

# Run benchmarks
cargo bench

# The benchmark suite tests:
# - SIMD operations (memchr, memset, line scanning)
# - Unicode operations (UTF-8 validation, grapheme segmentation)
# - Buffer operations (gap buffer, navigation)
```

## Environment Variables (Build-Time)

These affect how the binary is built:

```bash
# Limit included languages (reduces binary size)
EDIT_CFG_LANGUAGES="en,es,fr" cargo build --release

# ICU configuration (see ICU Configuration section)
EDIT_CFG_ICUUC_SONAME="..." cargo build
EDIT_CFG_ICU_RENAMING_VERSION="76" cargo build

# Use stable Rust features only (alternative to nightly)
RUSTC_BOOTSTRAP=1 cargo +stable build
# Note: Not officially supported, use at own risk
```

## Troubleshooting Checklist

If builds fail, verify:

1. ✅ Rust nightly installed: `rustup toolchain list | grep nightly`
2. ✅ rust-src component: `rustup component list --toolchain nightly | grep rust-src`
3. ✅ Visual Studio Build Tools (Windows): Check for MSVC in Program Files
4. ✅ Git link.exe not interfering: `where link.exe` should show MSVC path first
5. ✅ Clean build if switching configurations: `cargo clean`

If tests fail:
1. Check terminal compatibility (VT100/ANSI support)
2. Verify locale settings for Unicode tests
3. ICU tests are optional (ignored by default)

## Quick Start Summary

For first-time setup on Windows:

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Install nightly and components
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

# 3. Install VS Build Tools (download installer first)
# See "Toolchain Setup and Prerequisites" section above

# 4. Build and test
cargo build                          # Debug build
cargo test                          # Run tests
cargo build --config .cargo/release-nightly.toml --release  # Release build

# 5. Run
cargo run -- myfile.txt
```

Expected build times:
- Debug build: ~20-30 seconds (first build), <1 second (incremental)
- Release build: ~2-3 minutes (first build, rebuilds std library)
- Tests: ~1 minute (includes downloading test dependencies)

## Version Management

### Bumping Version Numbers

When releasing a new version, update the version number in **ONE place**:

**File:** `Cargo.toml`
```toml
[package]
name = "ogedit"
version = "1.2.1"  # <-- UPDATE THIS
```

The version automatically propagates to:
- Binary output via `env!("CARGO_PKG_VERSION")` macro
- Version display: `ogedit --version` output
- About dialog in the UI
- No manual updates needed elsewhere in the code

### Release Checklist

When preparing a new release:

1. **Update version in Cargo.toml**
   ```bash
   # Edit Cargo.toml and change version = "X.Y.Z"
   ```

2. **Clean and rebuild**
   ```bash
   cargo clean
   cargo build --config .cargo/release-nightly.toml --release
   cargo test
   ```

3. **Create distribution package**
   ```bash
   mkdir -p ./dist/ogedit-vX.Y.Z-windows-x86_64
   cp ./target/release/ogedit.exe ./dist/ogedit-vX.Y.Z-windows-x86_64/
   cp ./target/release/ogedit.exe ./dist/ogedit-vX.Y.Z-windows-x86_64/ogmsedit.exe
   cp ./target/release/ogedit.pdb ./dist/ogedit-vX.Y.Z-windows-x86_64/
   cp ./README.md ./LICENSE ./dist/ogedit-vX.Y.Z-windows-x86_64/
   cp -r ./assets ./dist/ogedit-vX.Y.Z-windows-x86_64/

   # Create INSTALL.txt, SHA256SUMS.txt
   # Zip the distribution folder
   ```

4. **Test the distribution package**
   ```bash
   ./dist/ogedit-vX.Y.Z-windows-x86_64/ogedit.exe --version
   ./dist/ogedit-vX.Y.Z-windows-x86_64/ogmsedit.exe --version
   ```

5. **Generate checksums**
   ```bash
   cd ./dist/ogedit-vX.Y.Z-windows-x86_64
   sha256sum ogedit.exe ogmsedit.exe > SHA256SUMS.txt
   cd ..
   sha256sum ogedit-vX.Y.Z-windows-x86_64.zip > ogedit-vX.Y.Z-windows-x86_64.zip.sha256
   ```

### Version Numbering Scheme

Follow semantic versioning (semver):
- **Major** (X.0.0): Breaking changes, incompatible API changes
- **Minor** (x.Y.0): New features, backwards compatible
- **Patch** (x.y.Z): Bug fixes, backwards compatible

Current version: **1.2.1**
