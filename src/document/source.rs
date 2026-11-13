//! 解析文本到来源文本、页码和区域的版本化映射。

use super::{DocumentSnapshot, check_range, content_hash, invalid};
use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMapKind {
    Exact,
    Cover,
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    #[serde(default = "unit_scale")]
    pub scale: u32,
    pub unit: DocumentRegionUnit,
}

fn unit_scale() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentRegionUnit {
    Point,
    Pixel,
    Normalized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentSourceLocation {
    pub page: Option<u32>,
    pub paragraph: Option<String>,
    pub region: Option<DocumentRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSourceMapEntry {
    pub text_start: usize,
    pub text_end: usize,
    pub source_start: usize,
    pub source_end: usize,
    pub kind: SourceMapKind,
    #[serde(default)]
    pub location: DocumentSourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSource {
    pub media_type: String,
    pub content_hash: String,
    /// 省略时，来源文本与解析文本共用内容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    /// 外部解析器提供原始二进制制品的SHA-256身份。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_hash: Option<String>,
    pub mappings: Vec<DocumentSourceMapEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSourceSpan {
    pub text_start: usize,
    pub text_end: usize,
    pub source_start: usize,
    pub source_end: usize,
    pub kind: SourceMapKind,
    pub content_hash: String,
    pub text: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub location: DocumentSourceLocation,
}

pub(crate) fn validate_hash(value: &str, field: &str) -> Result<()> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value.as_bytes()[7..]
            .iter()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(value))
    {
        return Err(invalid(
            field,
            "摘要要求规范SHA-256身份 (Digest requires a canonical SHA-256 identity)",
        ));
    }
    Ok(())
}

impl DocumentSourceLocation {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.page == Some(0)
            || self
                .paragraph
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 1024)
        {
            return Err(invalid(
                "source.mappings.location",
                "来源页码或段落身份无效 (Invalid source page or paragraph identity)",
            ));
        }
        if let Some(region) = &self.region
            && (region.scale == 0
                || region.width == 0
                || region.height == 0
                || (region.unit == DocumentRegionUnit::Normalized
                    && (u64::from(region.x) + u64::from(region.width) > u64::from(region.scale)
                        || u64::from(region.y) + u64::from(region.height)
                            > u64::from(region.scale))))
        {
            return Err(invalid(
                "source.mappings.location.region",
                "来源区域坐标无效 (Invalid source region coordinates)",
            ));
        }
        Ok(())
    }
}

impl DocumentSource {
    pub fn text<'a>(&'a self, parsed: &'a str) -> &'a str {
        self.original_text.as_deref().unwrap_or(parsed)
    }
    pub(crate) fn validate(&self, parsed: &str) -> Result<()> {
        if self.media_type.is_empty() || self.media_type.len() > 256 {
            return Err(invalid(
                "source.media_type",
                "来源媒体类型无效 (Invalid source media type)",
            ));
        }
        validate_hash(&self.content_hash, "source.content_hash")?;
        if let Some(hash) = &self.artifact_hash {
            validate_hash(hash, "source.artifact_hash")?;
        }
        let source = self.text(parsed);
        super::budget("document_source_bytes", source.len(), 64 * 1024 * 1024)?;
        super::budget("document_source_mappings", self.mappings.len(), 1_000_000)?;
        if content_hash(source) != self.content_hash {
            return Err(invalid(
                "source.content_hash",
                "来源正文与摘要不匹配 (Source text and digest do not match)",
            ));
        }
        let mut cursor = 0;
        for entry in &self.mappings {
            crate::request::check_current()?;
            if entry.text_start != cursor || entry.text_end <= entry.text_start {
                return Err(invalid(
                    "source.mappings",
                    "来源映射须完整连续覆盖解析文本 (Source mappings must cover the parsed text contiguously)",
                ));
            }
            check_range(
                parsed,
                entry.text_start,
                entry.text_end,
                "source.mappings.text",
            )?;
            check_range(
                source,
                entry.source_start,
                entry.source_end,
                "source.mappings.source",
            )?;
            entry.location.validate()?;
            match entry.kind {
                SourceMapKind::Exact
                    if parsed.get(entry.text_start..entry.text_end)
                        != source.get(entry.source_start..entry.source_end) =>
                {
                    return Err(invalid(
                        "source.mappings",
                        "精确映射的文本必须相同 (Exact mappings require identical text)",
                    ));
                }
                SourceMapKind::Synthetic if entry.source_start != entry.source_end => {
                    return Err(invalid(
                        "source.mappings",
                        "合成文本映射要求零宽来源范围 (Synthetic mappings require a zero-width source range)",
                    ));
                }
                SourceMapKind::Cover if entry.source_start == entry.source_end => {
                    return Err(invalid(
                        "source.mappings",
                        "覆盖映射要求非空来源范围 (Cover mappings require a nonempty source range)",
                    ));
                }
                _ => {}
            }
            cursor = entry.text_end;
        }
        if cursor != parsed.len() {
            return Err(invalid(
                "source.mappings",
                "来源映射未覆盖完整解析文本 (Source mappings do not cover the complete parsed text)",
            ));
        }
        Ok(())
    }
    pub(crate) fn span_charge(&self, parsed: &str, start: usize, end: usize) -> Result<usize> {
        check_range(parsed, start, end, "reference")?;
        let source = self.text(parsed);
        let mut bytes = 0usize;
        for entry in self
            .mappings
            .iter()
            .skip_while(|entry| entry.text_end <= start)
            .take_while(|entry| entry.text_start < end)
        {
            crate::request::check_current()?;
            let (first, last) = if entry.kind == SourceMapKind::Exact {
                (
                    entry.source_start + start.max(entry.text_start) - entry.text_start,
                    entry.source_start + end.min(entry.text_end) - entry.text_start,
                )
            } else {
                (entry.source_start, entry.source_end)
            };
            let text = source
                .get(first..last)
                .ok_or_else(|| invalid("source.mappings", "来源范围无效 (Invalid source range)"))?;
            bytes = bytes
                .saturating_add(512)
                .saturating_add(crate::storage::payload_store::serialized_json_len(&text)?)
                .saturating_add(crate::storage::payload_store::serialized_json_len(
                    &entry.location,
                )?);
        }
        Ok(bytes)
    }
    pub(crate) fn spans(
        &self,
        parsed: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<DocumentSourceSpan>> {
        check_range(parsed, start, end, "reference")?;
        let source = self.text(parsed);
        let mut spans = Vec::new();
        let mut boundaries = std::collections::BTreeSet::new();
        for entry in self
            .mappings
            .iter()
            .skip_while(|entry| entry.text_end <= start)
            .take_while(|entry| entry.text_start < end)
        {
            crate::request::check_current()?;
            let first = entry.text_start.max(start);
            let last = entry.text_end.min(end);
            let (source_start, source_end) = if entry.kind == SourceMapKind::Exact {
                (
                    entry.source_start + (first - entry.text_start),
                    entry.source_start + (last - entry.text_start),
                )
            } else {
                (entry.source_start, entry.source_end)
            };
            boundaries.insert(source_start);
            boundaries.insert(source_end);
            spans.push(DocumentSourceSpan {
                text_start: first,
                text_end: last,
                source_start,
                source_end,
                kind: entry.kind,
                content_hash: self.content_hash.clone(),
                text: source
                    .get(source_start..source_end)
                    .ok_or_else(|| {
                        invalid("source.mappings", "来源范围无效 (Invalid source range)")
                    })?
                    .into(),
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                location: entry.location.clone(),
            });
        }
        let boundaries = boundaries.into_iter().collect::<Vec<_>>();
        let locations = crate::error::QuerySpan::at_many(source, &boundaries)?;
        let locations = boundaries
            .into_iter()
            .zip(locations)
            .collect::<std::collections::BTreeMap<_, _>>();
        for span in &mut spans {
            let first = locations[&span.source_start];
            let last = locations[&span.source_end];
            span.line = first.line;
            span.column = first.column;
            span.end_line = last.line;
            span.end_column = last.column;
        }
        Ok(spans)
    }
}

impl DocumentSnapshot {
    pub(crate) fn source_spans(&self, start: usize, end: usize) -> Result<Vec<DocumentSourceSpan>> {
        self.source
            .as_ref()
            .map(|source| source.spans(&self.content, start, end))
            .transpose()
            .map(Option::unwrap_or_default)
    }
}
