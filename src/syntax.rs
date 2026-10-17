//! Incremental, bounded LSH highlighting. Checkpoints retain lexical state without copying files.
use std::ops::Range;
use std::path::Path;

use lsh::runtime::{Highlight, Runtime, RuntimeState};

use crate::document::ReadableDocument;
use crate::framebuffer::{Attributes, IndexedColor};

#[allow(dead_code)] // Kinds are constructed from checked bytecode discriminants.
mod definitions {
    include!(concat!(env!("OUT_DIR"), "/lsh_definitions.rs"));
}
use definitions::*;

/// Language IDs follow upstream LSH/VS Code names. `plaintext` disables colors.
pub fn language_exists(id: &str) -> bool {
    id == "plaintext" || LANGUAGES.iter().any(|language| language.id == id)
}

pub fn language_ids() -> impl Iterator<Item = &'static str> {
    LANGUAGES.iter().map(|language| language.id)
}

pub fn languages() -> &'static [lsh::runtime::Language] {
    LANGUAGES
}

pub fn language_for_path(path: &Path) -> Option<&'static lsh::runtime::Language> {
    let name = format!("/{}", path.to_string_lossy().replace('\\', "/"));
    FILE_ASSOCIATIONS
        .iter()
        .find(|(glob, _)| lsh::glob::glob_match(glob.as_bytes(), name.as_bytes()))
        .map(|(_, language)| *language)
}

pub struct PaintSpan {
    pub range: Range<usize>,
    pub foreground: Option<IndexedColor>,
    pub attributes: Option<Attributes>,
    pub bracket: bool,
}

pub struct SyntaxCache {
    language: &'static lsh::runtime::Language,
    plain: bool,
    revision: Option<u64>,
    checkpoints: Vec<(usize, RuntimeState)>,
    checkpoint_bytes: usize,
    blocked_at: Option<usize>,
    cached_range: Range<usize>,
    cached: Vec<(Range<usize>, HighlightKind)>,
}

impl SyntaxCache {
    pub fn for_path(path: &Path) -> Option<Self> {
        Some(Self::with_language(language_for_path(path)))
    }

    pub fn from_language(id: &str) -> Option<Self> {
        if id == "plaintext" {
            return Some(Self::with_language(None));
        }
        LANGUAGES
            .iter()
            .find(|language| language.id == id)
            .map(|language| Self::with_language(Some(language)))
    }

    fn with_language(language: Option<&'static lsh::runtime::Language>) -> Self {
        Self {
            language: language.unwrap_or(&LANGUAGES[0]),
            plain: language.is_none(),
            revision: None,
            checkpoints: Vec::new(),
            checkpoint_bytes: 0,
            blocked_at: None,
            cached_range: 0..0,
            cached: Vec::new(),
        }
    }

    fn runtime(&self) -> Runtime<'static, 'static, 'static> {
        Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, self.language.entrypoint)
    }

    fn parse(&mut self, doc: &dyn ReadableDocument, revision: u64, requested: Range<usize>) {
        if self.revision != Some(revision) {
            self.revision = Some(revision);
            self.checkpoints.clear();
            self.checkpoint_bytes = 0;
            self.blocked_at = None;
            self.cached_range = 0..0;
            self.cached.clear();
        }
        if self.blocked_at.is_some_and(|at| requested.start >= at) {
            self.cached.clear();
            return;
        }
        if self.cached_range.start <= requested.start && self.cached_range.end >= requested.end {
            return;
        }
        let mut runtime = self.runtime();
        let mut offset = 0;
        if let Some((at, state)) =
            self.checkpoints.iter().rev().find(|(at, _)| *at <= requested.start)
        {
            offset = *at;
            runtime.restore(state);
        }
        let start = offset;
        let mut checkpoint_at = offset;
        let mut line = Vec::new();
        let mut highlights: Vec<Highlight<HighlightKind>> = Vec::new();
        self.cached.clear();
        self.cached_range = requested.start..requested.start;
        // Work per frame is bounded, including when jumping deep into a very large file.
        while offset < requested.end && offset.saturating_sub(start) < 256 * 1024 {
            line.clear();
            let mut end = offset;
            loop {
                let chunk = doc.read_forward(end);
                if chunk.is_empty() {
                    break;
                }
                let scan = &chunk[..chunk.len().min(1024 * 1024 + 1 - line.len())];
                let count = scan.iter().position(|&b| b == b'\n').map_or(scan.len(), |i| i + 1);
                if line.len() + count > 1024 * 1024 {
                    self.blocked_at = Some(offset);
                    self.cached_range.end = requested.end;
                    return;
                }
                line.extend_from_slice(&chunk[..count]);
                end += count;
                if line.last() == Some(&b'\n') {
                    break;
                }
            }
            if end == offset {
                self.cached_range.end = requested.end;
                break;
            }
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            highlights.clear();
            if self.plain {
                highlights.push(Highlight { start: 0, kind: HighlightKind::Other });
                highlights.push(Highlight { start: line.len(), kind: HighlightKind::Other });
            } else {
                runtime.parse_next_line(&line, &mut highlights);
            }
            if runtime.heap_bytes() > 256 * 1024 {
                self.blocked_at = Some(offset);
                self.cached_range.end = requested.end;
                return;
            }
            if end >= requested.start {
                for pair in highlights.windows(2) {
                    let range = offset + pair[0].start..offset + pair[1].start;
                    if range.end > requested.start && range.start < requested.end {
                        self.cached.push((range, pair[0].kind));
                    }
                }
                self.cached_range.end = end;
            }
            offset = end;
            if offset - checkpoint_at >= 32 * 1024 {
                let index = self.checkpoints.partition_point(|(at, _)| *at < offset);
                if self.checkpoints.get(index).is_none_or(|(at, _)| *at != offset) {
                    let snapshot = runtime.snapshot();
                    self.checkpoint_bytes += snapshot.heap_bytes();
                    self.checkpoints.insert(index, (offset, snapshot));
                    while self.checkpoints.len() > 256 || self.checkpoint_bytes > 4 * 1024 * 1024 {
                        let eviction = if index < self.checkpoints.len() / 2 {
                            self.checkpoints.len() - 1
                        } else {
                            0
                        };
                        self.checkpoint_bytes -= self.checkpoints.remove(eviction).1.heap_bytes();
                    }
                }
                checkpoint_at = offset;
            }
        }
    }

    pub fn paint(
        &mut self,
        doc: &dyn ReadableDocument,
        revision: u64,
        visible: Range<usize>,
        cursor: usize,
    ) -> Vec<PaintSpan> {
        self.parse(doc, revision, visible.clone());
        let mut spans: Vec<_> = self
            .cached
            .iter()
            .filter(|(range, _)| range.end > visible.start && range.start < visible.end)
            .map(|(range, kind)| PaintSpan {
                range: range.clone(),
                foreground: color(*kind),
                attributes: attributes(*kind),
                bracket: false,
            })
            .collect();
        // Matching inside the parsed viewport is cheap and never treats strings/comments as code.
        if let Some((a, b)) = self.pair_in_cache(doc, cursor) {
            for at in [a, b] {
                spans.push(PaintSpan {
                    range: at..at + 1,
                    foreground: None,
                    attributes: None,
                    bracket: true,
                });
            }
        }
        spans
    }

    pub fn matching_bracket(
        &mut self,
        doc: &dyn ReadableDocument,
        revision: u64,
        cursor: usize,
    ) -> Option<usize> {
        // Explicit jump searches a bounded neighborhood; repeated renders retain lexical checkpoints.
        self.parse(
            doc,
            revision,
            cursor.saturating_sub(64 * 1024)..cursor.saturating_add(64 * 1024),
        );
        self.pair_in_cache(doc, cursor)
            .map(|(a, b)| if a == cursor || a + 1 == cursor { b } else { a })
    }

    fn pair_in_cache(&self, doc: &dyn ReadableDocument, cursor: usize) -> Option<(usize, usize)> {
        let is_bracket = |at| doc.read_forward(at).first().is_some_and(|b| b"()[]{}".contains(b));
        let at = if is_bracket(cursor) {
            cursor
        } else if cursor > 0 && is_bracket(cursor - 1) {
            cursor - 1
        } else {
            return None;
        };
        let mut stack: Vec<(u8, usize)> = Vec::new();
        for (range, kind) in &self.cached {
            if matches!(kind, HighlightKind::Comment | HighlightKind::String) {
                continue;
            }
            let mut pos = range.start;
            while pos < range.end {
                let chunk = doc.read_forward(pos);
                if chunk.is_empty() {
                    break;
                }
                let count = chunk.len().min(range.end - pos);
                for (index, &byte) in chunk[..count].iter().enumerate() {
                    let off = pos + index;
                    if b"([{".contains(&byte) {
                        stack.push((byte, off));
                    } else if let Some(i) = b")]}".iter().position(|&b| b == byte) {
                        let expected = match i {
                            0 => b'(',
                            1 => b'[',
                            _ => b'{',
                        };
                        if let Some((opening, start)) = stack.pop() {
                            if opening != expected {
                                stack.clear();
                                continue;
                            }
                            if start == at || off == at {
                                return Some((start, off));
                            }
                        }
                    }
                }
                pos += count;
            }
        }
        None
    }
}

fn color(kind: HighlightKind) -> Option<IndexedColor> {
    use HighlightKind::*;
    Some(match kind {
        Other | MarkupBold | MarkupItalic | MarkupLink | MarkupStrikethrough => return None,
        Comment => IndexedColor::White,
        String | MarkupInserted => IndexedColor::BrightGreen,
        ConstantNumeric | ConstantLanguage => IndexedColor::BrightMagenta,
        KeywordControl | KeywordOther | StorageType | KeywordPreprocessor => {
            IndexedColor::BrightCyan
        }
        MarkupDeleted => IndexedColor::BrightRed,
        _ => IndexedColor::BrightYellow,
    })
}

fn attributes(kind: HighlightKind) -> Option<Attributes> {
    Some(match kind {
        HighlightKind::MarkupBold => Attributes::Bold,
        HighlightKind::MarkupItalic => Attributes::Italic,
        HighlightKind::MarkupLink => Attributes::Underlined,
        HighlightKind::MarkupStrikethrough => Attributes::Strikethrough,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_markup_preserves_upstream_text_attributes() {
        let mut syntax = SyntaxCache::from_language("markdown").unwrap();
        let source = "**bold** *italic* ~~gone~~ [link](https://example.test)\n".to_string();
        let spans = syntax.paint(&source, 1, 0..source.len(), 0);
        for wanted in [
            Attributes::Bold,
            Attributes::Italic,
            Attributes::Underlined,
            Attributes::Strikethrough,
        ] {
            assert!(spans.iter().any(|span| span.attributes.is_some_and(|attr| attr.is(wanted))));
        }
    }

    #[test]
    fn explicit_language_override_and_plaintext_preserve_bracket_matching() {
        assert!(language_exists("rust"));
        assert!(language_exists("plaintext"));
        assert!(!language_exists("unrecognized-language"));
        assert!(SyntaxCache::from_language("unrecognized-language").is_none());
        let source = "fn main() { let value = 123; }".to_string();
        let mut plain = SyntaxCache::from_language("plaintext").unwrap();
        assert!(
            plain
                .paint(&source, 1, 0..source.len(), 0)
                .iter()
                .all(|span| span.foreground.is_none())
        );
        assert_eq!(plain.matching_bracket(&source, 1, 10), Some(source.len() - 1));
        let mut rust = SyntaxCache::from_language("rust").unwrap();
        assert!(
            rust.paint(&source, 1, 0..source.len(), 0).iter().any(|span| span.foreground.is_some())
        );
    }

    #[test]
    fn upstream_highlighting_corpus_stays_bounded_across_viewports() {
        let fixtures: &[(&str, &[u8])] = &[
            ("batch.bat", include_bytes!("../assets/highlighting-tests/batch.bat")),
            ("c.c", include_bytes!("../assets/highlighting-tests/c.c")),
            ("COMMIT_EDITMSG", include_bytes!("../assets/highlighting-tests/COMMIT_EDITMSG")),
            ("cpp.cpp", include_bytes!("../assets/highlighting-tests/cpp.cpp")),
            ("csharp.cs", include_bytes!("../assets/highlighting-tests/csharp.cs")),
            ("diff.diff", include_bytes!("../assets/highlighting-tests/diff.diff")),
            ("fsharp.fsx", include_bytes!("../assets/highlighting-tests/fsharp.fsx")),
            ("git-rebase-todo", include_bytes!("../assets/highlighting-tests/git-rebase-todo")),
            ("go.go", include_bytes!("../assets/highlighting-tests/go.go")),
            ("html.html", include_bytes!("../assets/highlighting-tests/html.html")),
            ("java.java", include_bytes!("../assets/highlighting-tests/java.java")),
            ("javascript.js", include_bytes!("../assets/highlighting-tests/javascript.js")),
            ("json.json", include_bytes!("../assets/highlighting-tests/json.json")),
            ("lua.lua", include_bytes!("../assets/highlighting-tests/lua.lua")),
            ("markdown.md", include_bytes!("../assets/highlighting-tests/markdown.md")),
            ("odin.odin", include_bytes!("../assets/highlighting-tests/odin.odin")),
            ("powershell.ps1", include_bytes!("../assets/highlighting-tests/powershell.ps1")),
            ("properties.conf", include_bytes!("../assets/highlighting-tests/properties.conf")),
            ("python.py", include_bytes!("../assets/highlighting-tests/python.py")),
            ("ruby.rb", include_bytes!("../assets/highlighting-tests/ruby.rb")),
            ("rust.rs", include_bytes!("../assets/highlighting-tests/rust.rs")),
            ("shellscript.sh", include_bytes!("../assets/highlighting-tests/shellscript.sh")),
            ("sql.sql", include_bytes!("../assets/highlighting-tests/sql.sql")),
            ("toml.toml", include_bytes!("../assets/highlighting-tests/toml.toml")),
            ("xml.xml", include_bytes!("../assets/highlighting-tests/xml.xml")),
            ("yaml.yml", include_bytes!("../assets/highlighting-tests/yaml.yml")),
            ("zig.zig", include_bytes!("../assets/highlighting-tests/zig.zig")),
        ];
        for &(name, bytes) in fixtures {
            let mut syntax = SyntaxCache::for_path(Path::new(name)).unwrap();
            for start in [0, bytes.len() / 2, bytes.len().saturating_sub(100)] {
                let end = (start + 4096).min(bytes.len());
                let spans = syntax.paint(&bytes, 1, start..end, start);
                assert!(
                    spans
                        .iter()
                        .all(|s| s.range.start <= s.range.end && s.range.end <= bytes.len()),
                    "{name}"
                );
                assert!(syntax.checkpoints.len() <= 256, "{name}");
                assert!(syntax.checkpoint_bytes <= 4 * 1024 * 1024, "{name}");
            }
        }
    }

    #[test]
    fn comments_strings_unicode_and_revision() {
        let mut syntax = SyntaxCache::for_path(Path::new("test.rs")).unwrap();
        let source = "fn main() { /* } */ let x = \"🙂 }\"; }".to_string();
        let paint = syntax.paint(&source, 1, 0..source.len(), 10);
        assert!(paint.iter().any(|s| s.foreground.is_some()));
        assert_eq!(syntax.matching_bracket(&source, 1, 10), Some(source.len() - 1));
        let changed = "fn main() { }".to_string();
        assert_eq!(syntax.matching_bracket(&changed, 2, 10), Some(changed.len() - 1));
    }
    #[test]
    fn multiline_comments_and_mismatched_pairs() {
        let mut syntax = SyntaxCache::for_path(Path::new("x.rs")).unwrap();
        let source = "/*\n{\n*/\n([)]".to_string();
        assert_eq!(syntax.matching_bracket(&source, 1, 8), None);
    }
    #[test]
    fn plain_text_brackets_and_forward_progress() {
        let mut syntax = SyntaxCache::for_path(Path::new("notes.txt")).unwrap();
        let source = "(hello)".to_string();
        assert_eq!(syntax.matching_bracket(&source, 1, 0), Some(6));
        let mut source = "// prefix\n".repeat(70_000);
        let offset = source.len();
        source.push_str("fn main() {}\n");
        let mut syntax = SyntaxCache::for_path(Path::new("deep.rs")).unwrap();
        let mut paint = Vec::new();
        for _ in 0..5 {
            paint = syntax.paint(&source, 2, offset..source.len(), offset);
        }
        assert!(paint.iter().any(|span| span.foreground.is_some()));
        assert!(paint.iter().all(|span| span.range.start >= offset));
    }

    #[test]
    fn stress_revisions_viewport_churn_and_malformed_source_stay_bounded() {
        let mut syntax = SyntaxCache::for_path(Path::new("stress.rs")).unwrap();
        let mut source = "fn main() { /* 界 */ let x = \"🙂\"; }\n".repeat(20_000).into_bytes();
        source.extend([0x80, 0xff, 0xf0, b'\n']);
        for revision in 0..64 {
            let start = revision as usize * 7919 % source.len();
            let end = (start + 2048).min(source.len());
            let spans = syntax.paint(&source.as_slice(), revision / 8, start..end, start);
            assert!(
                spans.iter().all(
                    |span| span.range.start <= span.range.end && span.range.end <= source.len()
                )
            );
            assert!(syntax.checkpoints.len() <= 256);
            assert!(syntax.checkpoint_bytes <= 4 * 1024 * 1024);
        }
        let empty = "".to_string();
        assert!(syntax.paint(&empty, 100, 0..0, 0).is_empty());
        assert!(syntax.checkpoints.is_empty());
        assert_eq!(syntax.checkpoint_bytes, 0);
    }

    #[test]
    fn oversized_line_falls_back_without_repeated_scans() {
        let mut syntax = SyntaxCache::for_path(Path::new("minified.json")).unwrap();
        let source = " ".repeat(2 * 1024 * 1024);
        assert!(syntax.paint(&source, 1, 0..80, 0).is_empty());
        assert_eq!(syntax.blocked_at, Some(0));
        assert!(syntax.paint(&source, 1, 0..80, 0).is_empty());
        assert!(syntax.checkpoints.is_empty());
    }
}
