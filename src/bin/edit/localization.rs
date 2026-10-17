// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ogedit::arena::scratch_arena;
use ogedit::helpers::AsciiStringHelpers as _;
use ogedit::sys;

include!(concat!(env!("OUT_DIR"), "/i18n_edit.rs"));

static S_LANG: std::sync::OnceLock<LangId> = std::sync::OnceLock::new();

pub fn init() {
    let scratch = scratch_arena(None);
    let langs = sys::preferred_languages(&scratch);
    let lang = select_language(langs.iter().map(|lang| &**lang));

    let _ = S_LANG.set(lang);
}

fn select_language<'a>(langs: impl IntoIterator<Item = &'a str>) -> LangId {
    let mut lang = LangId::en;

    'outer: for l in langs {
        for (prefix, id) in LANGUAGES {
            if l.starts_with_ignore_ascii_case(prefix) {
                lang = *id;
                break 'outer;
            }
        }
    }

    lang
}

pub fn loc(id: LocId) -> &'static str {
    TRANSLATIONS[*S_LANG.get_or_init(|| LangId::en) as usize][id as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression test for https://github.com/microsoft/edit/issues/832.
    #[test]
    fn chinese_region_aliases_select_expected_script() {
        for (actual, expected) in [
            ("zh-CN.UTF-8", "zh-hans"),
            ("zh-SG.UTF-8", "zh-hans"),
            ("zh-TW.UTF-8", "zh-hant"),
            ("zh-HK.UTF-8", "zh-hant"),
            ("zh-MO.UTF-8", "zh-hant"),
            ("zh-Hant.UTF-8", "zh-hant"),
            ("zh", "zh-hans"),
        ] {
            let Some(&(_, expected)) = LANGUAGES.iter().find(|(l, _)| expected == *l) else {
                continue; // Disabled by EDIT_CFG_LANGUAGES
            };

            assert!(select_language(std::iter::once(actual)) == expected);
        }
    }
}
