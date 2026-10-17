# Syntax fuzz target

Adapted from Microsoft Edit's LSH harness for OGEdit's bounded SyntaxCache API.
The target visits every compiled language, exercises three viewports, checks
output bounds and invalidates the cached document. Inputs are capped at 256 KiB
so one mutation cannot monopolize the run.

With cargo-fuzz installed on a supported host:

```sh
cargo fuzz run highlighter assets/highlighting-tests -- -max_total_time=60
```

The normal library suite also runs the 27 upstream highlighting samples. A
successful `cargo check --manifest-path fuzz/Cargo.toml` verifies compilation;
it is not a coverage-guided fuzzing result. See the integration report for the
exact checks run and native/custom-arena sanitizer limitations.
