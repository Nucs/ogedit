use std::borrow::Borrow;
use std::ops::Range;

use crate::arena::{Arena, ArenaString};

/// Borrow unchanged text; allocate only when unsafe terminal bytes need replacement.
pub enum SanitizedText<'a> {
    Borrowed(&'a str),
    Owned(SanitizedControlChars<'a>),
}

impl std::ops::Deref for SanitizedText<'_> {
    type Target = str;
    fn deref(&self) -> &str {
        match self {
            Self::Borrowed(text) => text,
            Self::Owned(value) => &value.text,
        }
    }
}
use crate::unicode::Utf8Chars;

pub struct SanitizedControlChars<'a> {
    /// Sanitized string with all C0/C1 control characters replaced by their Unicode representations.
    pub text: ArenaString<'a>,
    /// Byte ranges of the replacement characters within [`Self::text`].
    pub unsane_ranges: Vec<Range<usize>, &'a Arena>,
}

impl Borrow<str> for SanitizedControlChars<'_> {
    fn borrow(&self) -> &str {
        &self.text
    }
}

/// Strips all C0/C1 control characters and invalid UTF8 from the text.
#[inline]
pub fn sanitize_control_chars<'a>(
    arena: &'a Arena,
    text: &'a (impl AsRef<[u8]> + ?Sized),
) -> SanitizedText<'a> {
    sanitize_control_chars_impl(arena, text.as_ref())
}

/// Strips all C0/C1 control characters and invalid UTF8 from the text.
pub fn sanitize_control_chars_impl<'a>(arena: &'a Arena, text: &'a [u8]) -> SanitizedText<'a> {
    #[inline(always)]
    fn is_unsane(text: &[u8], beg: usize, end: usize, ch: char) -> bool {
        // Utf8Chars yields U+FFFD for invalid inputs, but it can also be a legitimate source character.
        // So, we need to check if the original bytes were actually invalid UTF8 (slow path).
        #[cold]
        fn is_invalid_utf8(text: &[u8], beg: usize, end: usize) -> bool {
            &text[beg..end] != "\u{FFFD}".as_bytes()
        }
        ch < '\x20'
            || ('\u{7F}'..='\u{9F}').contains(&ch)
            || (ch == char::REPLACEMENT_CHARACTER && is_invalid_utf8(text, beg, end))
    }

    if text.is_empty() {
        return SanitizedText::Borrowed("");
    }

    let mut sanitized = SanitizedControlChars {
        text: ArenaString::new_in(arena),
        unsane_ranges: Vec::new_in(arena),
    };
    let mut chars = Utf8Chars::new(text, 0);
    let mut sane_beg = 0;
    let mut visualizer_buf = [0xE2, 0x90, 0x80]; // U+2400 in UTF8

    while sane_beg < text.len() {
        let mut sane_end;
        let mut ch = '\0';

        // Find the next insane... err unsane...itized character.
        loop {
            sane_end = chars.offset();
            ch = match chars.next() {
                Some(ch) => ch,
                None => break,
            };
            if is_unsane(text, sane_end, chars.offset(), ch) {
                break;
            }
        }

        // Everything up to `sane_end` decoded successfully and is thus valid UTF8.
        let sane = unsafe { std::str::from_utf8_unchecked(&text[sane_beg..sane_end]) };

        // First chunk? We may have a fully sane string.
        if sanitized.text.is_empty() && sane_end == text.len() {
            return SanitizedText::Borrowed(sane);
        }

        // Copy the sane chunk into the new string.
        sanitized.text.push_str(sane);

        // Done?
        if sane_end == text.len() {
            break;
        }

        // Copy and sanitize as many characters as necessary.
        let unsane_beg = sanitized.text.len();
        loop {
            // Append a Unicode representation of the C0 or C1 control character.
            let mut visualized = "\u{FFFD}";

            if ch != '\u{FFFD}' {
                visualizer_buf[2] = if ch <= '\x1f' {
                    0x80 | ch as u8 // U+2400..=U+241F
                } else if ch == '\x7f' {
                    0xA1 // U+2421
                } else {
                    // NOTE: Unicode says to use U+FFFD, but that one is ambiguous width.
                    0xA6 // U+2426, because there are no pictures for C1 control characters.
                };
                // Our manually constructed UTF8 is never going to be invalid. Trust.
                visualized = unsafe { std::str::from_utf8_unchecked(&visualizer_buf) };
            }

            sanitized.text.push_str(visualized);

            // Peek the next char. And because we peek, we must stash the offset first.
            sane_beg = chars.offset();
            ch = match chars.next() {
                Some(ch) => ch,
                None => break,
            };
            if !is_unsane(text, sane_beg, chars.offset(), ch) {
                break;
            }
        }

        sanitized.unsane_ranges.push(unsane_beg..sanitized.text.len());
    }

    SanitizedText::Owned(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn test_sanitize_control_chars() {
        #[derive(Debug, PartialEq)]
        enum Result<'a> {
            Borrowed(&'a str),
            Owned(&'a str, &'a [Range<usize>]),
        }

        const TESTS: &[(&[u8], Result)] = &[
            (b"", Result::Borrowed("")),
            ("aé".as_bytes(), Result::Borrowed("aé")),
            ("a\u{FFFD}b".as_bytes(), Result::Borrowed("a\u{FFFD}b")),
            ("aé\u{1}\u{7f}\u{9f}b".as_bytes(), Result::Owned("aé␁␡␦b", &[3..12])),
            ("\u{1}a\0".as_bytes(), Result::Owned("␁a␀", &[0..3, 4..7])),
            (b"a\xff\xffb", Result::Owned("a\u{FFFD}\u{FFFD}b", &[1..7])),
            (b"\xf0\x9f\x98", Result::Owned("\u{FFFD}", &[0..3])),
        ];

        for (test, expected) in TESTS {
            let scratch = Arena::new(1024 * 1024).unwrap();
            let actual = sanitize_control_chars(&scratch, test);
            let actual = match &actual {
                SanitizedText::Borrowed(b) => Result::Borrowed(b),
                SanitizedText::Owned(o) => Result::Owned(&o.text, &o.unsane_ranges),
            };
            assert_eq!(&actual, expected, "test: {test:?}");
        }
    }
}
