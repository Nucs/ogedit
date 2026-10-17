Vendored from microsoft/edit commit cb6de0a8966aecd01d19a3541dab1829d0e679ca, crates/lsh. MIT license. Only workspace-inherited Cargo metadata is made standalone.
Compatibility: enable maybe_uninit_slice for the pinned nightly compiler (the upstream revision targets a newer compiler).
RuntimeState exposes heap_bytes() so OGEdit can bound checkpoint memory.
