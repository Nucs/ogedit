// Adapted from microsoft/edit 8b24907 and the current cb6de0a harness.
// Copyright (c) Microsoft Corporation. Licensed under the MIT License.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ogedit::syntax::{SyntaxCache, language_ids};

fuzz_target!(|data: &[u8]| {
    // Limit each input so one mutation still explores every supported language.
    if data.len() > 256 * 1024 {
        return;
    }
    for id in language_ids() {
        let mut syntax = SyntaxCache::from_language(id).unwrap();
        for start in [0, data.len() / 2, data.len().saturating_sub(4096)] {
            let end = (start + 4096).min(data.len());
            for span in syntax.paint(&data, 1, start..end, start) {
                assert!(span.range.start <= span.range.end && span.range.end <= data.len());
            }
        }
        // Force invalidation of any state cached for this source.
        let empty: &[u8] = &[];
        assert!(syntax.paint(&empty, 2, 0..0, 0).is_empty());
    }
});
