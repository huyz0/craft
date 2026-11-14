//! Bounded Markdown AST and deterministic provider document serialization.

use std::fmt;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};

pub(crate) mod compose;
mod import;
#[cfg(test)]
mod tests;
mod visible;
pub use visible::{VisibleField, VisibleSection};

pub const MAX_INPUT_BYTES: usize = 1_048_576;
pub const MAX_OUTPUT_BYTES: usize = 8_388_608;
const MAX_DEPTH: usize = 64;
const MAX_EVENTS: usize = 65_536;

#[derive(Debug, PartialEq, Eq)]
pub struct ContentError {
    pub offset: usize,
    pub path: Option<String>,
    pub reason: &'static str,
}

impl fmt::Display for ContentError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            return write!(out, "Document error at {path}: {}.", self.reason);
        }
        write!(
            out,
            "Document error at byte {}: {}.",
            self.offset, self.reason
        )
    }
}
impl std::error::Error for ContentError {}

fn failure(offset: usize, reason: &'static str) -> ContentError {
    ContentError {
        offset,
        path: None,
        reason,
    }
}

#[derive(Debug, Clone)]
enum Kind {
    Root,
    Paragraph,
    Heading(u8),
    Quote,
    List(Option<u64>),
    Item,
    Table(usize),
    Row(bool),
    Cell,
    Strong,
    Emphasis,
    Link(String),
    CodeBlock(Option<String>),
    Text(String),
    Code(String),
    Break,
    Rule,
}

#[derive(Debug, Clone)]
struct Node {
    kind: Kind,
    offset: usize,
    path: Option<String>,
    children: Vec<Node>,
}

impl Node {
    fn new(kind: Kind, offset: usize) -> Self {
        Self {
            kind,
            offset,
            path: None,
            children: Vec::new(),
        }
    }

    fn error(&self, reason: &'static str) -> ContentError {
        ContentError {
            offset: self.offset,
            path: self.path.clone(),
            reason,
        }
    }

    fn inline(&self) -> bool {
        matches!(
            self.kind,
            Kind::Strong
                | Kind::Emphasis
                | Kind::Link(_)
                | Kind::Text(_)
                | Kind::Code(_)
                | Kind::Break
        )
    }

    fn block(&self) -> bool {
        matches!(
            self.kind,
            Kind::Paragraph
                | Kind::Heading(_)
                | Kind::Quote
                | Kind::List(_)
                | Kind::Table(_)
                | Kind::CodeBlock(_)
                | Kind::Rule
        )
    }

    fn finish(&mut self) -> Result<(), ContentError> {
        if matches!(self.kind, Kind::List(_) | Kind::Quote) && self.children.is_empty() {
            return Err(self.error("empty_block_container"));
        }
        if matches!(self.kind, Kind::Strong | Kind::Emphasis | Kind::Link(_))
            && self.children.iter().any(Node::contains_code)
        {
            return Err(self.error("unsupported_formatted_code"));
        }
        if matches!(self.kind, Kind::Item) {
            // Tight Markdown lists omit paragraph events, while ADF requires them.
            let mut children = Vec::new();
            let mut paragraph = Node::new(Kind::Paragraph, self.offset);
            for child in std::mem::take(&mut self.children) {
                if child.inline() {
                    paragraph.children.push(child);
                } else {
                    if paragraph.children.iter().any(
                        |node| !matches!(&node.kind,Kind::Text(text) if text.trim().is_empty()),
                    ) {
                        children.push(paragraph);
                    }
                    paragraph = Node::new(Kind::Paragraph, child.offset);
                    children.push(child);
                }
            }
            if paragraph
                .children
                .iter()
                .any(|node| !matches!(&node.kind,Kind::Text(text) if text.trim().is_empty()))
            {
                children.push(paragraph);
            }
            if children
                .first()
                .is_none_or(|child| !matches!(child.kind, Kind::Paragraph))
            {
                children.insert(0, Node::new(Kind::Paragraph, self.offset));
            }
            self.children = children;
        }
        for child in &self.children {
            let valid = match self.kind {
                Kind::Root => child.block(),
                Kind::Quote | Kind::Item => matches!(
                    child.kind,
                    Kind::Paragraph | Kind::List(_) | Kind::CodeBlock(_)
                ),
                Kind::Paragraph
                | Kind::Heading(_)
                | Kind::Cell
                | Kind::Strong
                | Kind::Emphasis
                | Kind::Link(_) => child.inline(),
                Kind::CodeBlock(_) => matches!(child.kind, Kind::Text(_)),
                Kind::List(_) => matches!(child.kind, Kind::Item),
                Kind::Table(columns) => {
                    matches!(child.kind, Kind::Row(_)) && child.children.len() == columns
                }
                Kind::Row(_) => matches!(child.kind, Kind::Cell),
                _ => false,
            };
            if !valid {
                return Err(child.error("unsupported_structure"));
            }
        }
        Ok(())
    }

    fn contains_code(&self) -> bool {
        matches!(self.kind, Kind::Code(_)) || self.children.iter().any(Node::contains_code)
    }
}

struct Frame {
    node: Node,
    end: Option<TagEnd>,
}

/// Private AST construction ensures renderers only see bounded, checked content.
#[derive(Debug, Clone)]
pub struct Document {
    root: Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    Markdown,
    Storage,
    Adf,
}

/// Bounded original content remains available even when semantic import is refused.
pub struct RawDocument {
    format: DocumentFormat,
    source: String,
}

impl RawDocument {
    pub fn new(format: DocumentFormat, source: String) -> Result<Self, ContentError> {
        if source.len() > MAX_OUTPUT_BYTES {
            return Err(failure(0, "input_byte_limit"));
        }
        Ok(Self { format, source })
    }
    pub fn format(&self) -> DocumentFormat {
        self.format
    }
    pub fn raw(&self) -> &str {
        &self.source
    }
    pub fn import(&self) -> Result<Document, ContentError> {
        match self.format {
            DocumentFormat::Markdown => Document::from_markdown(&self.source),
            DocumentFormat::Storage => Document::from_storage(&self.source),
            DocumentFormat::Adf => Document::from_adf(self.source.as_bytes()),
        }
    }
}

fn safe_link(link: &str) -> bool {
    !link.chars().any(char::is_control)
        && url::Url::parse(link).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

fn row_columns(source: &str) -> usize {
    let row = source.trim();
    let mut separators = Vec::new();
    let mut escaped = false;
    for (index, byte) in row.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
        } else if byte == b'|' {
            separators.push(index);
        }
    }
    let leading = usize::from(separators.first() == Some(&0));
    let trailing = usize::from(
        separators.len() > leading
            && separators
                .last()
                .is_some_and(|index| *index + 1 == row.len()),
    );
    separators.len() - leading - trailing + 1
}

fn kind(tag: &Tag<'_>, offset: usize) -> Result<Kind, ContentError> {
    Ok(match tag {
        Tag::Paragraph => Kind::Paragraph,
        Tag::Heading {
            level,
            id,
            classes,
            attrs,
        } if id.is_none() && classes.is_empty() && attrs.is_empty() => Kind::Heading(*level as u8),
        Tag::BlockQuote(None) => Kind::Quote,
        Tag::List(start) if start.is_none_or(|number| (1..=999_999_999).contains(&number)) => {
            Kind::List(*start)
        }
        Tag::Item => Kind::Item,
        Tag::Table(alignments)
            if !alignments.is_empty()
                && alignments.iter().all(|value| *value == Alignment::None) =>
        {
            Kind::Table(alignments.len())
        }
        Tag::TableHead => Kind::Row(true),
        Tag::TableRow => Kind::Row(false),
        Tag::TableCell => Kind::Cell,
        Tag::Strong => Kind::Strong,
        Tag::Emphasis => Kind::Emphasis,
        Tag::Link {
            dest_url, title, ..
        } if title.is_empty() && safe_link(dest_url) => Kind::Link(dest_url.to_string()),
        Tag::CodeBlock(block) => {
            let language = match block {
                CodeBlockKind::Indented => None,
                CodeBlockKind::Fenced(info) if info.is_empty() => None,
                CodeBlockKind::Fenced(info)
                    if info.len() <= 64
                        && info.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || b"_+.#-".contains(&byte)
                        }) =>
                {
                    Some(info.to_string())
                }
                _ => return Err(failure(offset, "unsupported_code_info")),
            };
            Kind::CodeBlock(language)
        }
        Tag::HtmlBlock => return Err(failure(offset, "unsupported_html")),
        Tag::Image { .. } => return Err(failure(offset, "unsupported_image")),
        Tag::Link { .. } => return Err(failure(offset, "unsupported_link")),
        Tag::Table(_) => return Err(failure(offset, "unsupported_table_alignment")),
        _ => return Err(failure(offset, "unsupported_markdown_construct")),
    })
}

impl Document {
    /// Parses the supported CommonMark subset. Errors never include source content.
    pub fn from_markdown(source: &str) -> Result<Self, ContentError> {
        if source.len() > MAX_INPUT_BYTES {
            return Err(failure(0, "input_byte_limit"));
        }
        for (offset, character) in source.char_indices() {
            if !matches!(character as u32, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
            {
                return Err(failure(offset, "invalid_document_character"));
            }
        }
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_HEADING_ATTRIBUTES
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
            | Options::ENABLE_MATH
            | Options::ENABLE_GFM;
        let mut stack = vec![Frame {
            node: Node::new(Kind::Root, 0),
            end: None,
        }];
        for (count, (event, span)) in Parser::new_ext(source, options)
            .into_offset_iter()
            .enumerate()
        {
            if count >= MAX_EVENTS {
                return Err(failure(span.start, "event_limit"));
            }
            match event {
                Event::Start(tag) => {
                    if stack.len() >= MAX_DEPTH {
                        return Err(failure(span.start, "depth_limit"));
                    }
                    let node = Node::new(kind(&tag, span.start)?, span.start);
                    // GFM pads short rows and discards excess cells; neither is safe for plans.
                    if matches!(tag, Tag::TableHead | Tag::TableRow)
                        && let Some(Frame {
                            node:
                                Node {
                                    kind: Kind::Table(columns),
                                    ..
                                },
                            ..
                        }) = stack.last()
                        && row_columns(&source[span.clone()]) != *columns
                    {
                        return Err(failure(span.start, "non_rectangular_table"));
                    }
                    stack.push(Frame {
                        node,
                        end: Some(tag.to_end()),
                    });
                }
                Event::End(end) => {
                    let mut frame = stack
                        .pop()
                        .ok_or_else(|| failure(span.start, "invalid_structure"))?;
                    if frame.end != Some(end) {
                        return Err(failure(span.start, "invalid_structure"));
                    }
                    frame.node.finish()?;
                    stack
                        .last_mut()
                        .ok_or_else(|| failure(span.start, "invalid_structure"))?
                        .node
                        .children
                        .push(frame.node);
                }
                event => {
                    let kind = match event {
                        Event::Text(text) => Kind::Text(text.to_string()),
                        Event::Code(text) => {
                            if stack.iter().any(|frame| {
                                matches!(
                                    frame.node.kind,
                                    Kind::Strong | Kind::Emphasis | Kind::Link(_)
                                )
                            }) {
                                return Err(failure(span.start, "unsupported_formatted_code"));
                            }
                            Kind::Code(text.to_string())
                        }
                        Event::SoftBreak => Kind::Text(" ".into()),
                        Event::HardBreak => Kind::Break,
                        Event::Rule => Kind::Rule,
                        Event::Html(_) | Event::InlineHtml(_) => {
                            return Err(failure(span.start, "unsupported_html"));
                        }
                        Event::TaskListMarker(_) => {
                            return Err(failure(span.start, "unsupported_checkbox"));
                        }
                        _ => return Err(failure(span.start, "unsupported_markdown_construct")),
                    };
                    let frame = stack
                        .last_mut()
                        .ok_or_else(|| failure(span.start, "invalid_structure"))?;
                    if let Kind::Text(text) = &kind
                        && let Some(Node {
                            kind: Kind::Text(previous),
                            ..
                        }) = frame.node.children.last_mut()
                    {
                        previous.push_str(text);
                    } else {
                        frame.node.children.push(Node::new(kind, span.start));
                    }
                }
            }
        }
        if stack.len() != 1 {
            return Err(failure(0, "invalid_structure"));
        }
        let root = stack
            .pop()
            .ok_or_else(|| failure(0, "invalid_structure"))?
            .node;
        import::check_tree(root)
    }

    /// Emits ADF v1 without performing network requests or granting publication authority.
    pub fn to_adf(&self) -> Result<Value, ContentError> {
        let mut content = adf_children(&self.root, &[]);
        if content.is_empty() {
            content.push(json!({"type":"paragraph","content":[]}));
        }
        let value = json!({"type":"doc","version":1,"content":content});
        let bytes = serde_json::to_vec(&value).map_err(|_| failure(0, "encoding_failed"))?;
        if bytes.len() > MAX_OUTPUT_BYTES {
            return Err(failure(0, "output_byte_limit"));
        }
        craft_core::input::parse_json(&bytes, MAX_OUTPUT_BYTES)
            .map_err(|_| failure(0, "adf_structure_limit"))?;
        Ok(value)
    }

    /// Emits escaped storage markup; live tenant/editor normalization is separately qualified.
    pub fn to_storage(&self) -> Result<String, ContentError> {
        let mut output = String::new();
        storage(&self.root, &mut output);
        if output.is_empty() {
            output.push_str("<p></p>");
        }
        if output.len() > MAX_OUTPUT_BYTES {
            return Err(failure(0, "output_byte_limit"));
        }
        Ok(output)
    }
}

fn text(node: &Node) -> String {
    node.children
        .iter()
        .filter_map(|child| match &child.kind {
            Kind::Text(value) => Some(value.as_str()),
            _ => None,
        })
        .collect()
}

fn adf_children(node: &Node, marks: &[Value]) -> Vec<Value> {
    node.children
        .iter()
        .flat_map(|child| adf(child, marks))
        .collect()
}

fn adf(node: &Node, marks: &[Value]) -> Vec<Value> {
    let (name, attrs) = match &node.kind {
        Kind::Root => return adf_children(node, marks),
        Kind::Strong | Kind::Emphasis | Kind::Link(_) => {
            let mut marks = marks.to_vec();
            let mark = match &node.kind {
                Kind::Strong => json!({"type":"strong"}),
                Kind::Emphasis => json!({"type":"em"}),
                Kind::Link(href) => json!({"type":"link","attrs":{"href":href}}),
                _ => return Vec::new(),
            };
            if !marks.contains(&mark) {
                marks.push(mark);
            }
            return adf_children(node, &marks);
        }
        Kind::Text(value) | Kind::Code(value) => {
            if value.is_empty() {
                return Vec::new();
            }
            let mut result = json!({"type":"text","text":value});
            let marks = if matches!(node.kind, Kind::Code(_)) {
                vec![json!({"type":"code"})]
            } else {
                marks.to_vec()
            };
            if !marks.is_empty() {
                result["marks"] = json!(marks);
            }
            return vec![result];
        }
        Kind::CodeBlock(language) => {
            let body = text(node);
            let mut result = json!({"type":"codeBlock","content":if body.is_empty() {Vec::<Value>::new()} else {vec![json!({"type":"text","text":body})]}});
            if let Some(language) = language {
                result["attrs"] = json!({"language":language});
            }
            return vec![result];
        }
        Kind::Paragraph => ("paragraph", None),
        Kind::Heading(level) => ("heading", Some(json!({"level":level}))),
        Kind::Quote => ("blockquote", None),
        Kind::List(None) => ("bulletList", None),
        Kind::List(Some(start)) => ("orderedList", Some(json!({"order":start}))),
        Kind::Item => ("listItem", None),
        Kind::Table(_) => ("table", None),
        Kind::Row(_) => ("tableRow", None),
        Kind::Cell => {
            return vec![
                json!({"type":"tableCell","content":[{"type":"paragraph","content":adf_children(node,marks)}]}),
            ];
        }
        Kind::Break => return vec![json!({"type":"hardBreak"})],
        Kind::Rule => return vec![json!({"type":"rule"})],
    };
    let mut children = adf_children(node, marks);
    if matches!(node.kind, Kind::Row(true)) {
        for cell in &mut children {
            cell["type"] = json!("tableHeader");
        }
    }
    let mut result = json!({"type":name,"content":children});
    if let Some(attrs) = attrs {
        result["attrs"] = attrs;
    }
    vec![result]
}

fn escape(text: &str, output: &mut String) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
}

fn storage(node: &Node, out: &mut String) {
    let tag = match &node.kind {
        Kind::Root => {
            for child in &node.children {
                storage(child, out);
            }
            return;
        }
        Kind::Text(value) => {
            escape(value, out);
            return;
        }
        Kind::Code(value) => {
            out.push_str("<code>");
            escape(value, out);
            out.push_str("</code>");
            return;
        }
        Kind::CodeBlock(language) => {
            out.push_str("<ac:structured-macro ac:name=\"code\" ac:schema-version=\"1\">");
            if let Some(language) = language {
                out.push_str("<ac:parameter ac:name=\"language\">");
                escape(language, out);
                out.push_str("</ac:parameter>");
            }
            out.push_str("<ac:plain-text-body><![CDATA[");
            out.push_str(&text(node).replace("]]>", "]]]]><![CDATA[>"));
            out.push_str("]]></ac:plain-text-body></ac:structured-macro>");
            return;
        }
        Kind::Break => {
            out.push_str("<br />");
            return;
        }
        Kind::Rule => {
            out.push_str("<hr />");
            return;
        }
        Kind::Heading(level) => {
            out.push_str(&format!("<h{level}>"));
            for child in &node.children {
                storage(child, out);
            }
            out.push_str(&format!("</h{level}>"));
            return;
        }
        Kind::Link(href) => {
            out.push_str("<a href=\"");
            escape(href, out);
            out.push_str("\">");
            for child in &node.children {
                storage(child, out);
            }
            out.push_str("</a>");
            return;
        }
        Kind::List(Some(start)) => {
            out.push_str(&format!("<ol start=\"{start}\">"));
            for child in &node.children {
                storage(child, out);
            }
            out.push_str("</ol>");
            return;
        }
        Kind::Paragraph => "p",
        Kind::Quote => "blockquote",
        Kind::List(None) => "ul",
        Kind::Item => "li",
        Kind::Table(_) => "table",
        Kind::Row(_) => "tr",
        Kind::Cell => "td",
        Kind::Strong => "strong",
        Kind::Emphasis => "em",
    };
    out.push('<');
    out.push_str(tag);
    out.push('>');
    for child in &node.children {
        if matches!(node.kind, Kind::Row(true)) {
            out.push_str("<th>");
            for content in &child.children {
                storage(content, out);
            }
            out.push_str("</th>");
        } else {
            storage(child, out);
        }
    }
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}
