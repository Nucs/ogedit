# Memory and worker lifetime review

Reviewed `src/tui.rs`, `src/fuzzy.rs`, and the workbench, palette, workspace search, filesystem
watcher, and native file picker on 2026-10-17. This is the focused lifecycle
review; integrated sanitizer, browser stress, and process measurements are
recorded separately by the main validation run.

## Confirmed defects repaired

| Defect | Consequence | Repair |
| --- | --- | --- |
| TUI cached editors were always marked `seen = true`, with no per-frame reset | Closed documents and disappearing input fields could remain strongly referenced for the lifetime of the app | Reset marks before each frame and evict unseen cache entries after adopting the new tree |
| Arena textarea nodes stored borrowed buffer references whose only owner could be replaced in a cache slot | A previous layout tree could contain a dangling reference during the next frame | Give each arena tree a matching vector of `Rc` owners, released only after that arena is reset |
| Closing workbench panels retained a data view, its source buffer, search snapshots, and diff allocation | Closing a tab after viewing its data could retain the whole document; large previews remained allocated | Release heavy resources on close and on unrelated panel transitions; clear pending result snapshots after editing |
| Native filesystem notifications used an unbounded channel and an unbounded drain loop | A busy watched directory could grow memory indefinitely or starve the UI | Nonblocking 256-event queue, an overflow flag, a bounded drain, and metadata reconciliation after overflow |
| Every visited disk file was inserted into the search deduplication set | A search with no results still accumulated one owned path per file | Retain only the open-buffer paths that need precedence over disk reads |
| Dropping a search/index task only requested cooperative cancellation | Repeated replacements could accumulate detached workers if an OS filesystem call stalled | Two permits shared by discovery and search workers; an unavailable permit reports a retryable error; permits release on every worker exit or failed spawn |
| File discovery bounded file count but not aggregate path storage | Long names could make a 200,000-item index much larger than expected | Stop at the file-count cap or a conservative 64 MiB candidate-storage budget |
| The fuzzy scorer multiplied query and target lengths without a work budget | A large pasted query or filename could exhaust an arena with a quadratic score matrix | Check multiplication, cap the matrix at 262,144 cells, and use a linear subsequence/KMP fallback; reject fuzzy queries above 16 KiB while leaving palette text editable |
| Full Unicode case folding can expand characters, while fuzzy matrix indices referred to original characters | Later characters could be matched/highlighted at shifted positions | Switch expanding folds to the position-preserving fallback |

## Ownership and resource invariants

- `arena_prev`/`arena_next` and their corresponding buffer-owner vectors rotate
  together. Nodes contain borrowed buffer references, not `Rc` values whose
  destructors would be skipped by arena reset. Cache eviction cannot invalidate
  either live tree. A disappeared buffer can remain alive for one previous frame;
  it must be released when that tree is retired.
- Arena node strings and vectors allocate their backing storage in the same
  arena. Their element types own no external resources. New node fields that own
  heap allocations, handles, or reference counts must have an explicit destruction
  strategy rather than relying on arena reset.
- A workbench panel owns the data needed for its visible operation. Switching
  from a data/search panel into its cell editor retains the source; cancelling or
  applying the cell returns to that parent panel. Closing or replacing the panel
  releases its source, results, draft, and diff capacity.
- Native watcher callbacks never wait for the UI to drain events. Overflow can
  discard individual notifications, but forces a rescan of all watched documents.
  The existing periodic metadata reconciliation remains in place.
- Cancelled workers can finish asynchronously, but at most two discovery/search
  workers may be alive at once. Detached does not mean unbounded. Local scans
  release permits promptly; an indefinitely blocked network filesystem call can
  retain its permit until the operating system returns.
- Search retains at most its configured 64 MiB of matching source snapshots,
  10,000 matches, and 20 warnings by default. The temporary current-file read is
  independently limited to 8 MiB. The file-index budget is conservative accounting
  for paths, matching columns, and candidate overhead; it is not a process-RSS cap.
- The palette resolves an exact file path independently of its fuzzy query budget,
  so an existing pasted destination can still be activated. Oversized fuzzy queries
  do not reach either fuzzy engine. The linear fallback bounds intermediate space
  by query length and keeps positions in original character coordinates; its
  ranking is intentionally simpler than the matrix scorer's ranking.

## Regression coverage added

| Test | Evidence |
| --- | --- |
| `frame_buffer_owners_expire_after_tree_replacement` | 200 replace/remove cycles; `Weak` references prove previous-tree ownership remains valid during replacement and expires after retirement |
| `disappearing_editlines_release_their_cached_buffers` | A removed input field is evicted and its buffer destroyed |
| `closing_panels_releases_document_drafts_search_snapshots_and_diff_capacity` | Source, result snapshot, and draft ownership all expire; retained diff capacities become zero |
| `native_event_storm_is_bounded_and_overflow_rescans_watched_files` | Injects 20,000 unrelated events into 256 slots, then verifies a watched modification is detected through overflow reconciliation; unwatch empties file/directory registrations |
| `worker_permits_bound_live_tasks_and_are_reusable_after_drop` | A third worker is refused, released permits can be reacquired, and the count returns to zero |
| `repeatedly_cancelled_searches_release_worker_and_snapshot_ownership` | 48 searches with captured open-buffer snapshots are cancelled; each worker relinquishes its cancellation ownership within the timeout |
| `large_fuzzy_inputs_use_bounded_work_and_correct_source_positions` | A 40,001-character Unicode target exercises both linear subsequence and contiguous matching, with source positions checked and less than 32 KiB of result allocation |
| `oversized_fuzzy_query_returns_without_allocating_a_matrix` | A query exceeding 16 KiB returns no fuzzy result without allocating from the result arena |
| `case_fold_expansion_keeps_later_character_positions` | `Straße` and an uppercase sharp-S query retain correct positions after a full-fold expansion |
| `expanding_folds_preserve_icu_equivalences_in_linear_fallback` | Final sigma, long s, and the micro sign retain ICU case-fold equivalence when another character forces the bounded fallback |

These tests distinguish ownership release from allocator behavior. A process may
retain committed pages for reuse even after the last buffer owner disappears;
working-set or private-byte measurements alone cannot prove a leak.

## Reviewed without a code change

The Windows native picker already has one outstanding instance in application
state and one completion message. COM interfaces are scoped to their STA worker;
the apartment is uninitialized after interface destruction, and the selected
path's COM allocation has a `CoTaskMemFree` guard. Dropping the app terminates the
process; the picker is not replaced repeatedly while another picker is active.
No native desktop automation was used for this review.

## Limits

The native OS picker, pathological remote filesystem stalls, and platform-specific
notification providers cannot be exhaustively proven by deterministic tests.
The worker cap bounds the stalled-scan case without blocking editor shutdown on
a thread join. Overflow reconciliation cannot detect a transient write that is
fully reverted between metadata samples with identical identity, size, and
timestamp. Sanitizers and stress runs improve confidence but cannot establish
that arbitrary future input is failure-proof.
