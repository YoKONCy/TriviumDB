//! 内置文本解析和外部解析结果契约，所有范围采用UTF-8半开字节。

use super::{
    DocumentBlockInfo, DocumentSource, DocumentSourceLocation, DocumentSourceMapEntry,
    SourceMapKind, budget, content_hash, invalid,
};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ops::Range;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    #[default]
    Auto,
    Plain,
    Markdown,
    Html,
    Json,
    Jsonl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentParseConfig {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_blocks: usize,
    pub max_mappings: usize,
    pub max_block_bytes: usize,
    pub include_lines: bool,
    pub normalize_newlines: bool,
}
impl Default for DocumentParseConfig {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_output_bytes: 64 * 1024 * 1024,
            max_blocks: 100_000,
            max_mappings: 1_000_000,
            max_block_bytes: 65536,
            include_lines: false,
            normalize_newlines: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentParseInput {
    pub source_id: String,
    pub source_revision: String,
    pub content: String,
    #[serde(default)]
    pub format: DocumentFormat,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub config: DocumentParseConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ParsedDocument {
    pub source_id: String,
    pub source_revision: String,
    pub parser_id: String,
    pub content_hash: String,
    pub text: String,
    pub source: DocumentSource,
    pub blocks: Vec<DocumentBlockInfo>,
}
impl ParsedDocument {
    pub fn validate(&self) -> Result<()> {
        if self.source_id.is_empty() || self.source_revision.is_empty() || self.parser_id.is_empty()
        {
            return Err(invalid(
                "parsed",
                "解析来源身份不能为空 (Parsed source identities must be nonempty)",
            ));
        }
        budget("document_content_bytes", self.text.len(), 64 * 1024 * 1024)?;
        budget("document_blocks", self.blocks.len(), 100_000)?;
        if content_hash(&self.text) != self.content_hash
            || self.blocks.iter().any(|block| block.node_id.is_some())
        {
            return Err(invalid(
                "parsed",
                "解析摘要或逻辑块身份无效 (Invalid parsed digest or logical block identity)",
            ));
        }
        super::validate_blocks(&self.text, &self.blocks, None)?;
        self.source.validate(&self.text)
    }
}

pub fn parse(input: &DocumentParseInput) -> Result<ParsedDocument> {
    crate::request::check_current()?;
    if input.source_id.is_empty() || input.source_revision.is_empty() {
        return Err(invalid(
            "source_id",
            "来源身份与修订不能为空 (Source identity and revision must be nonempty)",
        ));
    }
    let config = &input.config;
    if !(4..=1024 * 1024).contains(&config.max_block_bytes) {
        return Err(invalid(
            "config.max_block_bytes",
            "分块字节数要求4至1048576 (Block bytes must be between 4 and 1048576)",
        ));
    }
    budget(
        "document_parse_input_bytes",
        input.content.len(),
        config.max_input_bytes.min(64 * 1024 * 1024),
    )?;
    let format = detect_format(input)?;
    let mut writer = Writer::new(&input.content, config);
    match format {
        DocumentFormat::Plain => plain(&mut writer)?,
        DocumentFormat::Markdown => markdown(&mut writer)?,
        DocumentFormat::Html => html(&mut writer, 0..input.content.len(), None)?,
        DocumentFormat::Json => json(&mut writer, false)?,
        DocumentFormat::Jsonl => json(&mut writer, true)?,
        DocumentFormat::Auto => {
            return Err(invalid(
                "format",
                "文档格式尚未解析 (Document format is unresolved)",
            ));
        }
    }
    writer.finish_blocks()?;
    let name = match format {
        DocumentFormat::Plain => "plain-utf8",
        DocumentFormat::Markdown => "markdown",
        DocumentFormat::Html => "html",
        DocumentFormat::Json => "json",
        DocumentFormat::Jsonl => "jsonl",
        DocumentFormat::Auto => "auto",
    };
    let media = match format {
        DocumentFormat::Markdown => "text/markdown",
        DocumentFormat::Html => "text/html",
        DocumentFormat::Json => "application/json",
        DocumentFormat::Jsonl => "application/x-ndjson",
        _ => "text/plain",
    };
    let config_json = serde_json::to_string(config).map_err(|error| {
        invalid(
            "config",
            &format!("解析配置编码失败 (Parse configuration encoding failed): {error}"),
        )
    })?;
    let source = DocumentSource {
        media_type: media.into(),
        content_hash: content_hash(&input.content),
        original_text: (writer.text != input.content).then(|| input.content.clone()),
        artifact_hash: None,
        mappings: writer.maps,
    };
    let parsed = ParsedDocument {
        source_id: input.source_id.clone(),
        source_revision: input.source_revision.clone(),
        parser_id: format!("tdb-{name}-v1:{}", content_hash(&config_json)),
        content_hash: content_hash(&writer.text),
        text: writer.text,
        source,
        blocks: writer.blocks,
    };
    parsed.validate()?;
    Ok(parsed)
}

fn detect_format(input: &DocumentParseInput) -> Result<DocumentFormat> {
    if input.format != DocumentFormat::Auto {
        return Ok(input.format);
    }
    let mime = input
        .media_type
        .as_deref()
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let name = input
        .source_id
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let extension = name.rsplit('.').next().unwrap_or("");
    let by_media = match mime.as_str() {
        "text/plain" => Some(DocumentFormat::Plain),
        "text/markdown" => Some(DocumentFormat::Markdown),
        "text/html" => Some(DocumentFormat::Html),
        "application/x-ndjson" | "application/jsonl" => Some(DocumentFormat::Jsonl),
        "application/json" | "application/ld+json" => Some(DocumentFormat::Json),
        "application/pdf"
        | "application/msword"
        | "application/vnd.ms-excel"
        | "application/vnd.ms-powerpoint"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            return Err(invalid(
                "format",
                "该格式要求外部解析结果接入 (This format requires external parsed-document input)",
            ));
        }
        _ => None,
    };
    if let Some(format) = by_media {
        return Ok(format);
    }
    let format = match extension {
        "md" | "markdown" => DocumentFormat::Markdown,
        "html" | "htm" => DocumentFormat::Html,
        "jsonl" | "ndjson" => DocumentFormat::Jsonl,
        "json" => DocumentFormat::Json,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" => {
            return Err(invalid(
                "format",
                "该格式要求外部解析结果接入 (This format requires external parsed-document input)",
            ));
        }
        _ => DocumentFormat::Plain,
    };
    Ok(format)
}

struct Writer<'a> {
    raw: &'a str,
    config: &'a DocumentParseConfig,
    text: String,
    maps: Vec<DocumentSourceMapEntry>,
    blocks: Vec<DocumentBlockInfo>,
}
impl<'a> Writer<'a> {
    fn new(raw: &'a str, config: &'a DocumentParseConfig) -> Self {
        Self {
            raw,
            config,
            text: String::new(),
            maps: Vec::new(),
            blocks: Vec::new(),
        }
    }
    fn append(&mut self, text: &str, source: Range<usize>) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        crate::request::check_current()?;
        let raw = self.raw.get(source.clone()).ok_or_else(|| {
            invalid(
                "source.mappings",
                &format!(
                    "解析器来源范围无效 (Parser source range is invalid): {:?}, bytes={}",
                    source,
                    self.raw.len()
                ),
            )
        })?;
        let kind = if source.is_empty() {
            SourceMapKind::Synthetic
        } else if raw == text {
            SourceMapKind::Exact
        } else {
            SourceMapKind::Cover
        };
        let end =
            self.text.len().checked_add(text.len()).ok_or_else(|| {
                invalid("content", "解析文本长度溢出 (Parsed text length overflow)")
            })?;
        budget(
            "document_parse_output_bytes",
            end,
            self.config.max_output_bytes.min(64 * 1024 * 1024),
        )?;
        if let Some(last) = self.maps.last_mut() {
            let merge = last.kind == kind
                && ((kind == SourceMapKind::Exact && last.source_end == source.start)
                    || (kind != SourceMapKind::Exact
                        && last.source_start == source.start
                        && last.source_end == source.end));
            if merge {
                last.text_end = end;
                if kind == SourceMapKind::Exact {
                    last.source_end = source.end;
                }
                self.text.push_str(text);
                return Ok(());
            }
        }
        budget(
            "document_source_mappings",
            self.maps.len().saturating_add(1),
            self.config.max_mappings.min(1_000_000),
        )?;
        self.maps.push(DocumentSourceMapEntry {
            text_start: self.text.len(),
            text_end: end,
            source_start: source.start,
            source_end: source.end,
            kind,
            location: DocumentSourceLocation::default(),
        });
        self.text.push_str(text);
        Ok(())
    }
    fn newline(&mut self, source: usize) -> Result<()> {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.append("\n", source..source)?;
        }
        Ok(())
    }
    fn open(&mut self, kind: &str, parent: Option<String>) -> Result<usize> {
        budget(
            "document_parse_blocks",
            self.blocks.len().saturating_add(1),
            self.config.max_blocks.min(100_000),
        )?;
        let index = self.blocks.len();
        self.blocks.push(DocumentBlockInfo {
            key: format!("block-{index}"),
            parent,
            kind: kind.into(),
            start_byte: self.text.len(),
            end_byte: self.text.len(),
            node_id: None,
        });
        Ok(index)
    }
    fn close(&mut self, index: usize) {
        self.blocks[index].end_byte = self.text.len();
    }
    fn finish_blocks(&mut self) -> Result<()> {
        let parents = self
            .blocks
            .iter()
            .filter_map(|block| block.parent.clone())
            .collect::<BTreeSet<_>>();
        let original = std::mem::take(&mut self.blocks);
        for mut block in original {
            budget(
                "document_parse_blocks",
                self.blocks.len().saturating_add(1),
                self.config.max_blocks.min(100_000),
            )?;
            let leaf = !parents.contains(&block.key);
            let start = block.start_byte;
            let end = block.end_byte;
            if leaf && end - start > self.config.max_block_bytes {
                let kind = block.kind.clone();
                let parent = block.key.clone();
                block.kind = "section".into();
                self.blocks.push(block);
                let mut cursor = start;
                let mut part = 0;
                while cursor < end {
                    let mut last = cursor.saturating_add(self.config.max_block_bytes).min(end);
                    while !self.text.is_char_boundary(last) {
                        last -= 1;
                    }
                    budget(
                        "document_parse_blocks",
                        self.blocks.len().saturating_add(1),
                        self.config.max_blocks.min(100_000),
                    )?;
                    self.blocks.push(DocumentBlockInfo {
                        key: format!("{parent}/part-{part}"),
                        parent: Some(parent.clone()),
                        kind: kind.clone(),
                        start_byte: cursor,
                        end_byte: last,
                        node_id: None,
                    });
                    cursor = last;
                    part += 1;
                }
            } else {
                self.blocks.push(block);
            }
        }
        if self.config.include_lines {
            let parents = self
                .blocks
                .iter()
                .filter_map(|block| block.parent.clone())
                .collect::<BTreeSet<_>>();
            let leaves = self
                .blocks
                .iter()
                .filter(|block| !parents.contains(&block.key))
                .cloned()
                .collect::<Vec<_>>();
            for block in leaves {
                let mut cursor = block.start_byte;
                for (line, text) in self.text[block.start_byte..block.end_byte]
                    .split_inclusive('\n')
                    .enumerate()
                {
                    let end = cursor + text.trim_end_matches(['\n', '\r']).len();
                    if end > cursor {
                        budget(
                            "document_parse_blocks",
                            self.blocks.len().saturating_add(1),
                            self.config.max_blocks.min(100_000),
                        )?;
                        self.blocks.push(DocumentBlockInfo {
                            key: format!("{}/line-{line}", block.key),
                            parent: Some(block.key.clone()),
                            kind: "line".into(),
                            start_byte: cursor,
                            end_byte: end,
                            node_id: None,
                        });
                    }
                    cursor += text.len();
                }
            }
        }
        Ok(())
    }
}

fn plain(writer: &mut Writer<'_>) -> Result<()> {
    let raw = writer.raw;
    let start = usize::from(raw.starts_with('\u{feff}')) * 3;
    if writer.config.normalize_newlines {
        let mut cursor = start;
        while cursor < raw.len() {
            let last = raw[cursor..]
                .find('\r')
                .map_or(raw.len(), |offset| cursor + offset);
            writer.append(&raw[cursor..last], cursor..last)?;
            if last == raw.len() {
                break;
            }
            let end = last + 1 + usize::from(raw.as_bytes().get(last + 1) == Some(&b'\n'));
            writer.append("\n", last..end)?;
            cursor = end;
        }
    } else {
        writer.append(&raw[start..], start..raw.len())?;
    }
    let mut cursor = 0;
    let mut paragraph = None;
    let mut paragraph_end = 0;
    while cursor < writer.text.len() {
        let end = writer.text[cursor..]
            .find('\n')
            .map_or(writer.text.len(), |offset| cursor + offset + 1);
        let line = &writer.text[cursor..end];
        if line.trim().is_empty() {
            if let Some(first) = paragraph.take() {
                let index = writer.open("paragraph", None)?;
                writer.blocks[index].start_byte = first;
                writer.blocks[index].end_byte = paragraph_end;
            }
        } else {
            if paragraph.is_none() {
                paragraph = Some(cursor);
            }
            paragraph_end = cursor + line.trim_end_matches(char::is_whitespace).len();
        }
        cursor = end;
    }
    if let Some(first) = paragraph {
        let index = writer.open("paragraph", None)?;
        writer.blocks[index].start_byte = first;
        writer.blocks[index].end_byte = paragraph_end;
    }

    Ok(())
}

fn markdown(writer: &mut Writer<'_>) -> Result<()> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut options = Options::empty();
    options
        .insert(Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS);
    let mut parser = Parser::new_ext(writer.raw, options)
        .into_offset_iter()
        .peekable();
    let mut open = Vec::<(TagEnd, usize)>::new();
    let mut sections = Vec::<(u8, usize)>::new();
    while let Some((event, range)) = parser.next() {
        crate::request::check_current()?;
        match event {
            Event::Start(tag) => {
                if let Tag::Heading { level, .. } = &tag
                    && open.is_empty()
                {
                    let level = *level as u8;
                    while sections.last().is_some_and(|(depth, _)| *depth >= level) {
                        if let Some((_, index)) = sections.pop() {
                            writer.close(index);
                        }
                    }
                    let parent = sections
                        .last()
                        .map(|(_, index)| writer.blocks[*index].key.clone());
                    let index = writer.open("section", parent)?;
                    sections.push((level, index));
                }
                let kind = match &tag {
                    Tag::Paragraph => Some("paragraph"),
                    Tag::Heading { .. } => Some("heading"),
                    Tag::CodeBlock(_) => Some("code"),
                    Tag::Item => Some("list_item"),
                    Tag::Table(_) => Some("table"),
                    Tag::TableCell => Some("cell"),
                    Tag::BlockQuote(_) => Some("quote"),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let parent = open
                        .last()
                        .map(|(_, index)| *index)
                        .or_else(|| sections.last().map(|(_, index)| *index))
                        .map(|index| writer.blocks[index].key.clone());
                    let index = writer.open(kind, parent)?;
                    open.push((tag.to_end(), index));
                }
            }
            Event::End(tag) => {
                if open.last().is_some_and(|(kind, _)| *kind == tag)
                    && let Some((_, index)) = open.pop()
                {
                    writer.close(index);
                    writer.newline(range.end)?;
                }
            }
            Event::Text(text)
            | Event::Code(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => writer.append(&text, range)?,
            Event::SoftBreak | Event::HardBreak => writer.append("\n", range)?,
            Event::Rule => writer.newline(range.end)?,
            Event::Html(_) => {
                let mut end = range.end;
                while parser
                    .peek()
                    .is_some_and(|(event, _)| matches!(event, Event::Html(_)))
                {
                    if let Some((_, next)) = parser.next() {
                        end = next.end;
                    }
                }
                let parent = open
                    .last()
                    .map(|(_, index)| *index)
                    .or_else(|| sections.last().map(|(_, index)| *index))
                    .map(|index| writer.blocks[index].key.clone());
                html(writer, range.start..end, parent)?;
            }
            Event::InlineHtml(_) => {
                let parent = open
                    .last()
                    .map(|(_, index)| *index)
                    .or_else(|| sections.last().map(|(_, index)| *index))
                    .map(|index| writer.blocks[index].key.clone());
                html(writer, range, parent)?;
            }
            Event::TaskListMarker(value) => {
                writer.append(if value { "[x] " } else { "[ ] " }, range)?
            }
            Event::FootnoteReference(text) => writer.append(&text, range)?,
        }
    }
    for (_, index) in open.into_iter().rev() {
        writer.close(index);
    }
    for (_, index) in sections.into_iter().rev() {
        writer.close(index);
    }
    Ok(())
}

fn html(writer: &mut Writer<'_>, range: Range<usize>, base_parent: Option<String>) -> Result<()> {
    use html5gum::{DefaultEmitter, Token, Tokenizer};
    let raw = writer
        .raw
        .get(range.clone())
        .ok_or_else(|| invalid("source", "HTML来源范围无效 (Invalid HTML source range)"))?;
    let mut emitter = DefaultEmitter::<usize>::new_with_span();
    emitter.naively_switch_states(true);
    let parser = Tokenizer::new_with_emitter(raw, emitter);
    let mut open = Vec::<(String, usize)>::new();
    let mut skipped = Vec::<String>::new();
    for token in parser {
        crate::request::check_current()?;
        let token = token.map_err(|error| {
            invalid(
                "content",
                &format!("HTML读取失败 (HTML read failed): {error}"),
            )
        })?;
        match token {
            Token::StartTag(tag) => {
                let name = std::str::from_utf8(&tag.name)
                    .map_err(|error| {
                        invalid(
                            "content",
                            &format!("HTML标签编码无效 (Invalid HTML tag encoding): {error}"),
                        )
                    })?
                    .to_owned();
                if matches!(name.as_str(), "script" | "style" | "template") {
                    if !tag.self_closing {
                        skipped.push(name.clone());
                    }
                    continue;
                }
                if !skipped.is_empty() {
                    continue;
                }
                let kind = match name.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "title" => Some("heading"),
                    "p" => Some("paragraph"),
                    "pre" => Some("code"),
                    "li" => Some("list_item"),
                    "td" | "th" => Some("cell"),
                    "blockquote" => Some("quote"),
                    "div" | "section" | "article" | "table" | "tr" | "ul" | "ol" => Some("section"),
                    _ => None,
                };
                if matches!(name.as_str(), "br" | "hr") {
                    writer.newline(range.start + tag.span.end)?;
                }
                if let Some(kind) = kind {
                    writer.newline(range.start + tag.span.start)?;
                    let parent = open
                        .last()
                        .map(|(_, index)| writer.blocks[*index].key.clone())
                        .or_else(|| base_parent.clone());
                    let index = writer.open(kind, parent)?;
                    if tag.self_closing {
                        writer.close(index);
                    } else {
                        open.push((name.clone(), index));
                    }
                }
            }
            Token::EndTag(tag) => {
                let name = std::str::from_utf8(&tag.name)
                    .map_err(|error| {
                        invalid(
                            "content",
                            &format!("HTML标签编码无效 (Invalid HTML tag encoding): {error}"),
                        )
                    })?
                    .to_owned();
                if skipped.last().is_some_and(|previous| *previous == name) {
                    skipped.pop();
                    continue;
                }
                if !skipped.is_empty() {
                    continue;
                }
                if let Some(index) = open.iter().rposition(|(previous, _)| *previous == name) {
                    for (_, block) in open.drain(index..).rev() {
                        writer.close(block);
                    }
                    writer.newline(range.start + tag.span.end)?;
                }
            }
            Token::String(text) if skipped.is_empty() => {
                let value = std::str::from_utf8(&text.value).map_err(|error| {
                    invalid(
                        "content",
                        &format!("HTML文本编码无效 (Invalid HTML text encoding): {error}"),
                    )
                })?;
                // 原文换行接替块边界的合成换行，避免重复插入同一分隔。
                if value.starts_with('\n')
                    && writer.maps.last().is_some_and(|entry| {
                        entry.kind == SourceMapKind::Synthetic
                            && entry.text_start + 1 == writer.text.len()
                            && writer.text.ends_with('\n')
                    })
                {
                    writer.text.pop();
                    writer.maps.pop();
                }
                writer.append(
                    value,
                    (range.start + text.span.start)..(range.start + text.span.end),
                )?;
            }
            _ => {}
        }
    }
    for (_, index) in open.into_iter().rev() {
        writer.close(index);
    }
    Ok(())
}

fn json(writer: &mut Writer<'_>, lines: bool) -> Result<()> {
    let raw = writer.raw;
    let mut ranges = Vec::new();
    if lines {
        let mut offset = 0;
        for line in raw.split_inclusive('\n') {
            if !line.trim().is_empty() {
                budget(
                    "document_parse_blocks",
                    ranges.len().saturating_add(1),
                    writer.config.max_blocks.min(100_000),
                )?;
                ranges.push(offset..offset + line.len());
            }
            offset += line.len();
        }
    } else {
        ranges.push(0..raw.len());
    }
    for range in ranges {
        crate::request::check_current()?;
        serde_json::from_str::<serde_json::Value>(&raw[range.clone()]).map_err(|error| {
            invalid(
                "content",
                &format!("JSON材料无效 (Invalid JSON document): {error}"),
            )
        })?;
        let record = writer.open("record", None)?;
        let mut cursor = range.start;
        while cursor < range.end {
            let byte = raw.as_bytes()[cursor];
            if byte.is_ascii_whitespace() || matches!(byte, b'{' | b'}' | b'[' | b']' | b',' | b':')
            {
                cursor += 1;
                continue;
            }
            let first = cursor;
            let (text, source) = if byte == b'"' {
                cursor += 1;
                while cursor < range.end {
                    match raw.as_bytes()[cursor] {
                        b'\\' => cursor = cursor.saturating_add(2),
                        b'"' => {
                            cursor += 1;
                            break;
                        }
                        _ => cursor += 1,
                    }
                }
                let text =
                    serde_json::from_str::<String>(&raw[first..cursor]).map_err(|error| {
                        invalid(
                            "content",
                            &format!("JSON字符串无效 (Invalid JSON string): {error}"),
                        )
                    })?;
                (text, (first + 1)..cursor.saturating_sub(1))
            } else {
                while cursor < range.end
                    && !raw.as_bytes()[cursor].is_ascii_whitespace()
                    && !matches!(raw.as_bytes()[cursor], b',' | b']' | b'}')
                {
                    cursor += 1;
                }
                (raw[first..cursor].into(), first..cursor)
            };
            let key = raw[cursor..range.end].trim_start().starts_with(':');
            let block = writer.open(
                if key { "field" } else { "value" },
                Some(writer.blocks[record].key.clone()),
            )?;
            writer.append(&text, source)?;
            writer.close(block);
            if key {
                writer.append(": ", cursor..cursor)?;
            } else {
                writer.newline(cursor)?;
            }
        }
        writer.close(record);
    }
    Ok(())
}
