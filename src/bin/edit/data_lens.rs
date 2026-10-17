// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Editable projections of source text. A cell always points back to an exact
//! source range. Parsing and edits never reserialize the surrounding document.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use jsonc_parser::ast::Value;
use jsonc_parser::common::Ranged;

use crate::workspace_search::PreparedEdit;

const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 20_000;
const MAX_CELLS: usize = 200_000;
pub const DEFAULT_LOG_PATTERN: &str = r"(?i)^(?:(?P<time>\d{4}-\d{2}-\d{2}[T ][0-9:.+\-]+Z?|\[[^\]\r\n]+\])\s+)?(?:(?P<level>TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)\b[\s:\-]*)?(?P<message>.*)$";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LensKind {
    Csv,
    Tsv,
    Json,
    Log { pattern: String },
}

impl LensKind {
    pub fn infer(path: Option<&Path>) -> Self {
        let extension = path.and_then(Path::extension).and_then(|s| s.to_str()).unwrap_or("");
        match extension.to_ascii_lowercase().as_str() {
            "csv" => Self::Csv,
            "tsv" | "tab" => Self::Tsv,
            "json" | "jsonc" => Self::Json,
            _ => Self::Log { pattern: DEFAULT_LOG_PATTERN.into() },
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Tsv => "TSV",
            Self::Json => "JSON",
            Self::Log { .. } => "Log",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Csv,
    JsonString,
    JsonNumber,
    JsonBoolean,
    JsonNull,
    JsonContainer,
    Log,
    ReadOnly,
}

#[derive(Clone, Debug)]
pub struct DataCell {
    /// Decoded CSV / JSON string; all other cells show literal source text.
    pub value: String,
    /// None for a JSON path label or an absent optional log capture.
    pub range: Option<Range<usize>>,
    pub kind: CellKind,
}

impl DataCell {
    pub fn editable(&self) -> bool {
        self.range.is_some() && self.kind != CellKind::ReadOnly
    }
}

#[derive(Clone, Debug)]
pub struct DataRow {
    pub label: String,
    pub cells: Vec<DataCell>,
}

#[derive(Clone, Debug)]
pub struct DataLens {
    pub kind: LensKind,
    pub columns: Vec<String>,
    pub rows: Vec<DataRow>,
    pub warnings: Vec<String>,
    pub revision: u64,
    source: Arc<str>,
}

impl DataLens {
    pub fn parse(text: &str, revision: u64, kind: LensKind) -> Result<Self, String> {
        if text.len() > MAX_SOURCE_BYTES {
            return Err(
                "Data views are limited to 8 MiB. Use the source editor for larger files.".into()
            );
        }
        if text.contains('\0') {
            return Err("This file contains binary data and cannot be shown as a data view.".into());
        }
        let mut lens = Self {
            kind,
            columns: Vec::new(),
            rows: Vec::new(),
            warnings: Vec::new(),
            revision,
            source: Arc::from(text),
        };
        match lens.kind.clone() {
            LensKind::Csv => lens.parse_csv(b',')?,
            LensKind::Tsv => lens.parse_csv(b'\t')?,
            LensKind::Json => lens.parse_json()?,
            LensKind::Log { pattern } => lens.parse_log(&pattern)?,
        }
        Ok(lens)
    }

    pub fn is_current(&self, text: &str, revision: u64) -> bool {
        self.revision == revision && &*self.source == text
    }

    /// Rebuild ranges after edits, undo, reload, or a switch back from source.
    pub fn refresh(&mut self, text: &str, revision: u64) -> Result<(), String> {
        let next = Self::parse(text, revision, self.kind.clone())?;
        *self = next;
        Ok(())
    }

    pub fn prepare_edit(
        &self,
        current: &str,
        revision: u64,
        row: usize,
        column: usize,
        value: &str,
    ) -> Result<PreparedEdit, String> {
        if !self.is_current(current, revision) {
            return Err("The source changed. Refresh this view before editing.".into());
        }
        let cell = self
            .rows
            .get(row)
            .and_then(|row| row.cells.get(column))
            .ok_or("That cell no longer exists.")?;
        if !cell.editable() {
            return Err("This cell has no editable source field.".into());
        }
        let range = cell.range.clone().unwrap();
        let replacement = match cell.kind {
            CellKind::Csv => {
                let delimiter = if self.kind == LensKind::Tsv { b'\t' } else { b',' };
                encode_csv(value, delimiter, current[range.clone()].starts_with('"'))?
            }
            CellKind::JsonString => serde_json::to_string(value).map_err(|err| err.to_string())?,
            CellKind::JsonNumber
            | CellKind::JsonBoolean
            | CellKind::JsonNull
            | CellKind::JsonContainer => {
                check_json_depth(value)?;
                let parsed =
                    jsonc_parser::parse_to_ast(value, &Default::default(), &json_options(false))
                        .map_err(|err| format!("Enter a valid JSON value: {err}"))?;
                let parsed = parsed.value.ok_or("Enter a JSON value.")?;
                let valid_type = match cell.kind {
                    CellKind::JsonNumber => matches!(parsed, Value::NumberLit(_)),
                    CellKind::JsonBoolean => matches!(parsed, Value::BooleanLit(_)),
                    CellKind::JsonNull => true,
                    CellKind::JsonContainer => matches!(parsed, Value::Array(_) | Value::Object(_)),
                    _ => false,
                };
                if !valid_type {
                    return Err(
                        "Keep this field's JSON type; change its type in the source editor.".into(),
                    );
                }
                value.trim().to_owned()
            }
            CellKind::Log => {
                if value.contains(['\r', '\n']) {
                    return Err("A log field cannot contain a line break.".into());
                }
                value.to_owned()
            }
            CellKind::ReadOnly => return Err("This cell has no editable source field.".into()),
        };
        let edit = PreparedEdit::new(self.source.clone(), Some(revision), range, replacement)?;
        let updated = edit.apply_to(current, Some(revision))?;
        // Check the result before the editor commits it. In particular a log edit
        // must not silently break the extraction expression or change columns.
        let reparsed = Self::parse(&updated, revision, self.kind.clone())?;
        if cell.kind == CellKind::Log {
            let updated_cell = reparsed.rows.get(row).and_then(|row| row.cells.get(column));
            if !updated_cell.is_some_and(|next| next.editable() && next.value == value) {
                return Err(
                    "This value no longer matches that log field. Edit it in the source editor."
                        .into(),
                );
            }
        }
        Ok(edit)
    }

    fn parse_csv(&mut self, delimiter: u8) -> Result<(), String> {
        let spans = csv_spans(&self.source, delimiter)?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(delimiter)
            .flexible(false)
            .from_reader(self.source.as_bytes());
        for (index, record) in reader.records().enumerate() {
            if index >= MAX_ROWS {
                return Err("Data view exceeds 20,000 rows. Use the source editor.".into());
            }
            let record = record.map_err(|err| format!("Invalid table: {err}"))?;
            let row_spans =
                spans.get(index).ok_or("Cannot map this CSV record back to its source.")?;
            if record.len() != row_spans.len() {
                return Err("Cannot safely map this CSV record back to its source.".into());
            }
            let cells = record
                .iter()
                .zip(row_spans)
                .map(|(value, range)| DataCell {
                    value: value.to_owned(),
                    range: Some(range.clone()),
                    kind: CellKind::Csv,
                })
                .collect();
            self.rows.push(DataRow { label: (index + 1).to_string(), cells });
        }
        if self.rows.len() != spans.len() {
            return Err("Cannot safely map this CSV file back to its source.".into());
        }
        self.columns =
            (0..self.rows.first().map_or(0, |r| r.cells.len())).map(column_name).collect();
        Ok(())
    }

    fn parse_json(&mut self) -> Result<(), String> {
        check_json_depth(&self.source)?;
        let parsed =
            jsonc_parser::parse_to_ast(&self.source, &Default::default(), &json_options(true))
                .map_err(|err| format!("Invalid JSON: {err}"))?;
        let value = parsed.value.ok_or("Enter a JSON value to open this view.")?;
        self.columns = vec!["Path".into(), "Value".into()];
        collect_json_rows(&value, "$", &self.source, &mut self.rows)?;
        Ok(())
    }

    fn parse_log(&mut self, pattern: &str) -> Result<(), String> {
        if pattern.len() > 16 * 1024 {
            return Err("Log patterns are limited to 16 KiB.".into());
        }
        let regex = regex::RegexBuilder::new(pattern)
            .size_limit(4 * 1024 * 1024)
            .build()
            .map_err(|err| format!("Invalid log pattern: {err}"))?;
        if regex.captures_len() < 2 {
            return Err(
                "Use parentheses to capture log fields, for example (?P<level>\\w+).".into()
            );
        }
        if regex.captures_len() > 129 {
            return Err("Log views support up to 128 captured fields.".into());
        }
        self.columns = regex
            .capture_names()
            .enumerate()
            .skip(1)
            .map(|(i, name)| name.map_or_else(|| i.to_string(), str::to_owned))
            .collect();
        let mut offset = 0;
        let mut unmatched = 0;
        for (index, terminated) in self.source.split_inclusive('\n').enumerate() {
            if index >= MAX_ROWS || (index + 1) * self.columns.len() > MAX_CELLS {
                return Err(
                    "Log view is too large. Use the source editor or narrow the file.".into()
                );
            }
            let line = terminated.strip_suffix('\n').unwrap_or(terminated);
            let line = line.strip_suffix('\r').unwrap_or(line);
            let cells = if let Some(captures) = regex.captures(line) {
                (1..regex.captures_len())
                    .map(|index| {
                        if let Some(field) = captures.get(index) {
                            DataCell {
                                value: field.as_str().into(),
                                range: Some(offset + field.start()..offset + field.end()),
                                kind: CellKind::Log,
                            }
                        } else {
                            DataCell { value: String::new(), range: None, kind: CellKind::ReadOnly }
                        }
                    })
                    .collect()
            } else {
                unmatched += 1;
                (0..self.columns.len())
                    .map(|index| DataCell {
                        value: if index + 1 == self.columns.len() {
                            line.into()
                        } else {
                            String::new()
                        },
                        range: None,
                        kind: CellKind::ReadOnly,
                    })
                    .collect()
            };
            self.rows.push(DataRow { label: (index + 1).to_string(), cells });
            offset += terminated.len();
        }
        if unmatched > 0 {
            self.warnings
                .push(format!("{unmatched} lines do not match the pattern and are read-only."));
        }
        Ok(())
    }
}

pub(crate) fn json_options(comments: bool) -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions {
        allow_comments: comments,
        allow_trailing_commas: comments,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

fn column_name(mut index: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap()
}

/// Strict RFC-style quoting plus exact raw spans. csv handles decoding and
/// record consistency; the scanner adds locations its records do not expose.
fn csv_spans(source: &str, delimiter: u8) -> Result<Vec<Vec<Range<usize>>>, String> {
    let bytes = source.as_bytes();
    let mut at = if source.starts_with('\u{feff}') { 3 } else { 0 };
    let mut rows = Vec::new();
    let mut total_cells = 0;
    while at < bytes.len() {
        if bytes[at] == b'\r' || bytes[at] == b'\n' {
            at += if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') { 2 } else { 1 };
            continue;
        }
        let mut row = Vec::new();
        loop {
            let start = at;
            if bytes.get(at) == Some(&b'"') {
                at += 1;
                loop {
                    match bytes.get(at) {
                        None => return Err(format!("Unclosed quoted field at byte {start}.")),
                        Some(b'"') if bytes.get(at + 1) == Some(&b'"') => at += 2,
                        Some(b'"') => {
                            at += 1;
                            break;
                        }
                        _ => at += 1,
                    }
                }
                if bytes
                    .get(at)
                    .is_some_and(|byte| *byte != delimiter && *byte != b'\r' && *byte != b'\n')
                {
                    return Err(format!("Unexpected text after a closing quote at byte {at}."));
                }
            } else {
                while let Some(&byte) = bytes.get(at) {
                    if byte == delimiter || byte == b'\r' || byte == b'\n' {
                        break;
                    }
                    if byte == b'"' {
                        return Err(format!("Quote inside an unquoted field at byte {at}."));
                    }
                    at += 1;
                }
            }
            row.push(start..at);
            total_cells += 1;
            if total_cells > MAX_CELLS {
                return Err("Data view exceeds 200,000 cells. Use the source editor.".into());
            }
            if bytes.get(at) == Some(&delimiter) {
                at += 1;
                continue;
            }
            if bytes.get(at) == Some(&b'\r') {
                at += 1;
            }
            if bytes.get(at) == Some(&b'\n') {
                at += 1;
            }
            break;
        }
        rows.push(row);
        if rows.len() > MAX_ROWS {
            return Err("Data view exceeds 20,000 rows. Use the source editor.".into());
        }
    }
    Ok(rows)
}

fn encode_csv(value: &str, delimiter: u8, keep_quoted: bool) -> Result<String, String> {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .quote_style(if keep_quoted { csv::QuoteStyle::Always } else { csv::QuoteStyle::Necessary })
        .from_writer(Vec::new());
    writer.write_record([value]).map_err(|err| err.to_string())?;
    let bytes = writer.into_inner().map_err(|err| err.to_string())?;
    let mut encoded = String::from_utf8(bytes).map_err(|err| err.to_string())?;
    // WriterBuilder's default terminator is LF. The source owns the original
    // terminator, and a field replacement must never replace that terminator.
    if !encoded.ends_with('\n') {
        return Err("CSV writer did not produce a complete field.".into());
    }
    encoded.pop();
    Ok(encoded)
}

fn collect_json_rows(
    value: &Value<'_>,
    path: &str,
    source: &str,
    rows: &mut Vec<DataRow>,
) -> Result<(), String> {
    if path.len() > 4096 {
        return Err("A JSON path exceeds 4 KiB. Use the source editor for this document.".into());
    }
    match value {
        Value::Object(object) if !object.properties.is_empty() => {
            for property in &object.properties {
                let key = property.name.as_str();
                let child_path =
                    if key.chars().all(|c| c.is_alphanumeric() || c == '_') && !key.is_empty() {
                        format!("{path}.{key}")
                    } else {
                        format!(
                            "{path}[{}]",
                            serde_json::to_string(key).map_err(|err| err.to_string())?
                        )
                    };
                collect_json_rows(&property.value, &child_path, source, rows)?;
            }
            return Ok(());
        }
        Value::Array(array) if !array.elements.is_empty() => {
            for (index, element) in array.elements.iter().enumerate() {
                collect_json_rows(element, &format!("{path}[{index}]"), source, rows)?;
            }
            return Ok(());
        }
        _ => {}
    }
    if rows.len() >= MAX_ROWS {
        return Err("JSON view exceeds 20,000 values. Use the source editor.".into());
    }
    let range = value.start()..value.end();
    let raw = source.get(range.clone()).ok_or("Invalid JSON source range.")?;
    let (kind, display) = match value {
        Value::StringLit(_) => (
            CellKind::JsonString,
            serde_json::from_str::<String>(raw).map_err(|err| err.to_string())?,
        ),
        Value::NumberLit(_) => (CellKind::JsonNumber, raw.into()),
        Value::BooleanLit(_) => (CellKind::JsonBoolean, raw.into()),
        Value::NullKeyword(_) => (CellKind::JsonNull, raw.into()),
        _ => (CellKind::JsonContainer, raw.into()),
    };
    rows.push(DataRow {
        label: (rows.len() + 1).to_string(),
        cells: vec![
            DataCell { value: path.into(), range: None, kind: CellKind::ReadOnly },
            DataCell { value: display, range: Some(range), kind },
        ],
    });
    Ok(())
}

/// Guard recursive AST parsing against adversarial nesting. Ignore brackets in
/// strings and comments; malformed syntax is reported by the real parser.
pub(crate) fn check_json_depth(source: &str) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut depth = 0usize;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                at += 1;
                while at < bytes.len() {
                    match bytes[at] {
                        b'\\' => at += 2,
                        b'"' => {
                            at += 1;
                            break;
                        }
                        _ => at += 1,
                    }
                }
                continue;
            }
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
                continue;
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at + 1 < bytes.len() && &bytes[at..at + 2] != b"*/" {
                    at += 1;
                }
                at += 2;
                continue;
            }
            b'[' | b'{' => {
                depth += 1;
                if depth > 128 {
                    return Err("JSON is nested more than 128 levels deep.".into());
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        at += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_multiline_quotes_unicode_and_crlf_are_preserved() {
        let source = "name,note\r\n\"שלום\",\"one, two\r\n\"\"three\"\"\"\r\n";
        let lens = DataLens::parse(source, 1, LensKind::Csv).unwrap();
        assert_eq!(lens.rows.len(), 2);
        assert_eq!(lens.rows[1].cells[0].value, "שלום");
        assert_eq!(lens.rows[1].cells[1].value, "one, two\r\n\"three\"");
        let edit = lens.prepare_edit(source, 1, 1, 0, "café").unwrap();
        let changed = edit.apply_to(source, Some(1)).unwrap();
        assert_eq!(changed, "name,note\r\n\"café\",\"one, two\r\n\"\"three\"\"\"\r\n");
        assert!(lens.prepare_edit(&changed, 1, 1, 1, "bad").is_err());
    }

    #[test]
    fn tsv_edits_quote_tabs_and_newlines_without_changing_other_fields() {
        let source = "a\tb\n1\t2\n";
        let lens = DataLens::parse(source, 3, LensKind::Tsv).unwrap();
        let edit = lens.prepare_edit(source, 3, 1, 1, "x\ty\nz").unwrap();
        assert_eq!(edit.apply_to(source, Some(3)).unwrap(), "a\tb\n1\t\"x\ty\nz\"\n");
    }

    #[test]
    fn csv_empty_final_fields_and_bom_keep_correct_spans() {
        let source = "\u{feff}a,b\r\n1,\r\n2,";
        let lens = DataLens::parse(source, 0, LensKind::Csv).unwrap();
        assert_eq!(lens.rows.len(), 3);
        let edit = lens.prepare_edit(source, 0, 2, 1, "end").unwrap();
        assert_eq!(edit.apply_to(source, Some(0)).unwrap(), "\u{feff}a,b\r\n1,\r\n2,end");
    }

    #[test]
    fn malformed_csv_never_produces_editable_ranges() {
        for source in ["a,b\n1\n", "a,\"unclosed", "a,\"closed\"oops", "a,un\"quoted"] {
            assert!(DataLens::parse(source, 0, LensKind::Csv).is_err(), "{source:?}");
        }
    }

    #[test]
    fn json_string_edits_preserve_comments_escapes_and_formatting() {
        let source =
            "{\r\n // keep me\r\n \"name\" : \"café\",\r\n \"items\": [true, 1e2,],\r\n}\r\n";
        let lens = DataLens::parse(source, 4, LensKind::Json).unwrap();
        assert_eq!(lens.rows[0].cells[0].value, "$.name");
        assert_eq!(lens.rows[1].cells[0].value, "$.items[0]");
        let edit = lens.prepare_edit(source, 4, 0, 1, "\"שלום\"\nline").unwrap();
        let expected = source.replace("\"café\"", "\"\\\"שלום\\\"\\nline\"");
        assert_eq!(edit.apply_to(source, Some(4)).unwrap(), expected);
        assert!(lens.prepare_edit(source, 5, 0, 1, "stale").is_err());
        assert!(lens.prepare_edit(source, 4, 1, 1, "banana").is_err());
        assert!(lens.prepare_edit(source, 4, 2, 1, "\"wrong type\"").is_err());
    }

    #[test]
    fn json_invalid_syntax_and_extreme_depth_are_reported() {
        for source in ["{\"x\":}", "{\"x\":1 \"y\":2}", "{loose:1}", "{'x':1}", "{\"x\":NaN}", ""] {
            assert!(DataLens::parse(source, 0, LensKind::Json).is_err(), "{source:?}");
        }
        let nested = format!("{}0{}", "[".repeat(129), "]".repeat(129));
        assert!(DataLens::parse(&nested, 0, LensKind::Json).is_err());
    }

    #[test]
    fn log_capture_edits_preserve_crlf_and_unmatched_lines_are_read_only() {
        let source = "INFO שלום\r\nnot matching\r\nERROR café\r\n";
        let kind = LensKind::Log { pattern: r"^(?P<level>INFO|ERROR) (?P<message>.*)$".into() };
        let lens = DataLens::parse(source, 1, kind).unwrap();
        assert_eq!(lens.columns, ["level", "message"]);
        assert!(!lens.rows[1].cells[1].editable());
        assert_eq!(lens.warnings.len(), 1);
        let edit = lens.prepare_edit(source, 1, 2, 1, "changed").unwrap();
        assert_eq!(
            edit.apply_to(source, Some(1)).unwrap(),
            "INFO שלום\r\nnot matching\r\nERROR changed\r\n"
        );
        assert!(lens.prepare_edit(source, 1, 0, 0, "BOGUS").is_err());
        assert!(lens.prepare_edit(source, 1, 1, 1, "changed").is_err());
    }

    #[test]
    fn refresh_rebuilds_source_positions_and_revision() {
        let source = "{\"a\":\"é\",\"b\":42}";
        let mut lens = DataLens::parse(source, 1, LensKind::Json).unwrap();
        let edited = lens
            .prepare_edit(source, 1, 0, 1, "much longer")
            .unwrap()
            .apply_to(source, Some(1))
            .unwrap();
        lens.refresh(&edited, 2).unwrap();
        let edit = lens.prepare_edit(&edited, 2, 1, 1, "43").unwrap();
        assert_eq!(edit.apply_to(&edited, Some(2)).unwrap(), "{\"a\":\"much longer\",\"b\":43}");
    }

    #[test]
    fn default_log_fields_accept_common_timestamp_format() {
        let source = "2026-10-17T12:00:00Z INFO ready\n";
        let lens = DataLens::parse(source, 0, LensKind::infer(None)).unwrap();
        assert_eq!(lens.rows[0].cells[0].value, "2026-10-17T12:00:00Z");
        assert_eq!(lens.rows[0].cells[1].value, "INFO");
        assert_eq!(lens.rows[0].cells[2].value, "ready");
    }

    #[test]
    fn csv_span_mapping_matches_the_csv_writer_across_tricky_fields() {
        let values = ["", "plain", "café", "a,b", "a\"b", "a\r\nb", " x "];
        for first in values {
            for second in values {
                let mut writer = csv::Writer::from_writer(Vec::new());
                writer.write_record([first, second]).unwrap();
                writer.write_record([second, first]).unwrap();
                let source = String::from_utf8(writer.into_inner().unwrap()).unwrap();
                let lens = DataLens::parse(&source, 0, LensKind::Csv).unwrap();
                assert_eq!(lens.rows[0].cells[0].value, first);
                assert_eq!(lens.rows[0].cells[1].value, second);
                assert_eq!(lens.rows[1].cells[0].value, second);
                let edit = lens.prepare_edit(&source, 0, 1, 1, "new,\"value\"\nline").unwrap();
                let changed = edit.apply_to(&source, Some(0)).unwrap();
                assert!(changed.starts_with(&source[..edit.range.start]));
                assert!(changed.ends_with(&source[edit.range.end..]));
                let changed = DataLens::parse(&changed, 1, LensKind::Csv).unwrap();
                assert_eq!(changed.rows[1].cells[1].value, "new,\"value\"\nline");
            }
        }
    }

    #[test]
    fn json_numbers_are_not_rounded_or_limited_to_machine_precision() {
        let source = "{\"id\":123456789012345678901234567890,\"large\":1e9999}";
        let lens = DataLens::parse(source, 0, LensKind::Json).unwrap();
        let value = "987654321098765432109876543210";
        let edit = lens.prepare_edit(source, 0, 0, 1, value).unwrap();
        assert_eq!(
            edit.apply_to(source, Some(0)).unwrap(),
            format!("{{\"id\":{value},\"large\":1e9999}}")
        );
        assert!(lens.prepare_edit(source, 0, 0, 1, "01").is_err());
        assert!(lens.prepare_edit(source, 0, 0, 1, "NaN").is_err());
    }
}
