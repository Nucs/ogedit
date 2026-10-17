# Low-level memory safety review

Scope: `src/arena/`, `src/buffer/gap_buffer.rs`, `src/helpers.rs`, and the virtual-memory / input-conversion portions of `src/sys/windows.rs` and `src/sys/unix.rs`. This review concerns the working tree after the legacy-fork audit, including the modernization changes.

## Corrected findings

| Area | Failure mechanism | Correction and regression coverage |
| --- | --- | --- |
| Arena size arithmetic | Capacity rounding and typed slice multiplication could wrap in optimized builds. A small allocation could then be exposed as a much larger mutable slice. | Checked capacity rounding, `Layout::array`, checked offsets/end positions, `isize::MAX` byte bound. Impossible-size tests verify failure leaves existing allocations intact. |
| Arena alignment | Aligning the offset alone assumed that the virtual-memory base met every requested type alignment. Zero-sized allocations in an empty arena returned a byte-aligned dangling pointer regardless of layout. | Align the actual address; return a layout-aligned dangling pointer for zero-sized requests. A deliberately shifted backing region makes the alignment regression independent of OS placement. |
| Reallocation alignment | The custom `Allocator` assumed grow/shrink could not increase alignment. Rust's allocator contract permits this. | Retain the pointer only if it meets the new alignment; otherwise allocate and copy. Tests verify alignment, preserved prefixes, zero-filled growth, and shrink behavior. |
| Scratch allocator concurrency | Production builds used process-global mutable arenas while unit tests used thread-local arenas. Safe calls from different workers could concurrently mutate the same offsets. | All builds use independent thread-local pools. Eight workers allocate and check distinct sentinel buffers concurrently. |
| Scratch scope lifetime | Release builds omitted nested-borrow checks. Reinitialization could replace arenas underneath live scopes, and a lifetime-erased delegate could outlive its thread-local pool. | Keep borrow-order checks in every build; reject live-scope reinitialization; scopes own backing arenas through private `Rc` delegates. No lifetime transmute remains. A TLS destructor-order test deliberately retains a scope until after its pool is destroyed. |
| Oversized gap requests | `allocate_gap(..., usize::MAX, ...)` overflowed during spare-space rounding. In release this could shrink the gap unexpectedly and underflow subsequent arithmetic. | Saturating rounding bounded by the buffer's reserve. Heap growth uses `try_reserve_exact` before mutation. Tests cover oversized requests, full-capacity replacement rollback, and 50,000 deterministic edits against a `Vec<u8>` oracle across both heap and virtual-memory backing. |
| Generic file reads | `file_read_uninit` constructed `&mut [u8]` over uninitialized bytes and passed it to safe `Read::read`. A valid custom reader may inspect all destination bytes; an invalid reported length also must not escape to unsafe callers. | Initialize the whole destination first and reject reported lengths beyond it. Custom-reader regressions inspect every byte before writing and return a deliberately invalid count. |
| String and vector arithmetic | Unicode repeat multiplication, inclusive/exclusive bound adjustment, and replacement lengths could overflow in optimized builds. | Check arithmetic before mutation; vector bounds retain the existing clamping behavior. Tests include oversized Unicode repeats, maximum inclusive bounds, and a maximum-length zero-sized-element vector. |
| Windows virtual-memory debug hints | A mutable static address counter was incremented concurrently by tests/workers. Repeated stress could also exhaust the artificial address range despite available memory. | Atomic address counter; fall back to OS-selected placement when a debug address hint fails. |
| Asynchronous platform notifications | The Windows console control callback and Unix signal handler wrote ordinary booleans also read by the UI thread. | Atomic exit/resize flags; Unix resize consumption uses an atomic swap so a newly arriving event is retained. |
| FFI input/output boundaries | Windows UTF-16 conversion wrote into spare allocation through a pointer derived from an empty string; the Unix ICU name helper exposed raw-pointer dereference through a safe function. | Use `Vec::spare_capacity_mut` for the native output buffer; mark the ICU helper unsafe and specify its live C-string / UTF-8 / arena-lifetime requirements. |

## Focused validation

The added tests are part of the regular library test suite. The parent stress audit records the executed commands, exact counts, sanitizer results, and browser runs. Useful targeted commands are:

```powershell
cargo test --locked --lib arena::
cargo test --locked --lib buffer::gap_buffer::
cargo test --locked --lib helpers::tests::
```

The allocator stress case performs 16,384 mixed-size/alignment allocations, verifies all live neighbors, and repeatedly resets the arena after its allocations are dead. Scratch tests include 1,000 nested scope/reset cycles, live reinitialization rejection, release-mode borrow-order checks, concurrent worker isolation, and reverse TLS teardown order. The gap test checks both forward/backward reads, gap boundaries, commit/reserve invariants, arbitrary byte contents, clear, insert/delete/replace, and out-of-range offsets after every operation.

## Limits and intentional retention

An arena keeps its committed high-water mark until its owner is dropped; dropping a scratch scope resets the allocation watermark rather than returning committed pages to the OS. This is bounded reusable capacity, not evidence of a leak. A large gap buffer similarly reserves virtual address space without eagerly committing all of it. Process memory measurements must distinguish virtual reserve, committed/private memory, and working set.

AddressSanitizer cannot identify every intra-arena overwrite: many logical allocations share one committed virtual-memory region without individual sanitizer redzones. Explicit invariant, sentinel, reference-model, alignment, and lifetime tests supplement sanitizer runs. Passing those tests does not prove absence of every possible memory defect. OS console lifecycle routines still assume initialization, input, and teardown occur on the UI thread; only asynchronous notification flags are shared with callbacks. Native platform behavior not exercised by the available host is limited to code review and cross-compilation, as recorded in the parent audit.
