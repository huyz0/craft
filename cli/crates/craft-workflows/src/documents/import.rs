use std::collections::{BTreeMap, BTreeSet};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event as XmlEvent};

use super::*;

fn at(path: &str, reason: &'static str) -> ContentError {
    ContentError {
        offset: 0,
        path: Some(path.into()),
        reason,
    }
}

fn valid_characters(text: &str) -> bool {
    invalid_character(text).is_none()
}

fn invalid_character(text: &str) -> Option<usize> {
    text.char_indices().find_map(|(offset,character)|(!matches!(character as u32,0x9|0xa|0xd|0x20..=0xd7ff|0xe000..=0xfffd|0x10000..=0x10ffff)).then_some(offset))
}

pub(super) fn check_tree(mut root: Node) -> Result<Document, ContentError> {
    fn visit(
        node: &mut Node,
        depth: usize,
        nodes: &mut usize,
        text: &mut usize,
    ) -> Result<(), ContentError> {
        node.finish()?;
        *nodes += 1;
        if *nodes > MAX_EVENTS {
            return Err(node.error("node_limit"));
        }
        if depth >= MAX_DEPTH {
            return Err(node.error("depth_limit"));
        }
        let value = match &node.kind {
            Kind::Text(value) | Kind::Code(value) | Kind::Link(value) => Some(value.as_str()),
            Kind::CodeBlock(language) => language.as_deref(),
            _ => None,
        };
        if let Some(value) = value {
            *text += value.len();
            if *text > MAX_INPUT_BYTES {
                return Err(node.error("text_byte_limit"));
            }
            if !valid_characters(value) {
                return Err(node.error("invalid_document_character"));
            }
        }
        for child in &mut node.children {
            visit(child, depth + 1, nodes, text)?;
        }
        Ok(())
    }
    visit(&mut root, 0, &mut 0, &mut 0)?;
    Ok(Document { root })
}

enum XmlChild {
    Text(String, usize),
    Element(Element),
}
struct Element {
    name: String,
    attrs: BTreeMap<String, String>,
    children: Vec<XmlChild>,
    offset: usize,
}

fn element(start: &BytesStart<'_>, offset: usize) -> Result<Element, ContentError> {
    let name = std::str::from_utf8(start.name().as_ref())
        .map_err(|_| failure(offset, "invalid_storage"))?
        .to_owned();
    let mut attrs = BTreeMap::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|_| failure(offset, "invalid_storage_attribute"))?;
        if attrs.len() >= 32 {
            return Err(failure(offset, "attribute_limit"));
        }
        let key = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|_| failure(offset, "invalid_storage_attribute"))?
            .to_owned();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| failure(offset, "invalid_storage_attribute"))?;
        if value.len() > 4096 || !valid_characters(&value) {
            return Err(failure(offset, "invalid_storage_attribute"));
        }
        if attrs.insert(key, value.into_owned()).is_some() {
            return Err(failure(offset, "duplicate_storage_attribute"));
        }
    }
    Ok(Element {
        name,
        attrs,
        children: Vec::new(),
        offset,
    })
}

fn attributes(element: &Element, allowed: &[&str]) -> Result<(), ContentError> {
    if element
        .attrs
        .keys()
        .any(|key| !allowed.contains(&key.as_str()))
    {
        return Err(failure(element.offset, "unsupported_storage_attribute"));
    }
    Ok(())
}

fn elements(children: &[XmlChild]) -> Result<Vec<&Element>, ContentError> {
    children
        .iter()
        .filter_map(|child| match child {
            XmlChild::Element(element) => Some(Ok(element)),
            XmlChild::Text(text, _) if text.trim().is_empty() => None,
            XmlChild::Text(_, offset) => Some(Err(failure(*offset, "ambiguous_storage_text"))),
        })
        .collect()
}

fn plain(children: &[XmlChild], offset: usize) -> Result<String, ContentError> {
    let mut output = String::new();
    for child in children {
        match child {
            XmlChild::Text(text, _) => output.push_str(text),
            _ => return Err(failure(offset, "unsupported_plain_text_body")),
        }
    }
    Ok(output)
}

fn inlines(children: &[XmlChild]) -> Result<Vec<Node>, ContentError> {
    let mut output = Vec::new();
    for child in children {
        let node = match child {
            XmlChild::Text(text, offset) => Node::new(Kind::Text(text.clone()), *offset),
            XmlChild::Element(element) => storage_node(element)?,
        };
        if !node.inline() {
            return Err(node.error("unsupported_inline_structure"));
        }
        if let Kind::Text(text) = &node.kind
            && let Some(Node {
                kind: Kind::Text(previous),
                ..
            }) = output.last_mut()
        {
            previous.push_str(text);
        } else {
            output.push(node);
        }
    }
    Ok(output)
}

fn blocks(children: &[XmlChild]) -> Result<Vec<Node>, ContentError> {
    elements(children)?.into_iter().map(storage_node).collect()
}

fn language(value: &str, offset: usize) -> Result<Option<String>, ContentError> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+.#-".contains(&byte))
    {
        return Err(failure(offset, "unsupported_code_info"));
    }
    Ok(Some(value.into()))
}

fn code_macro(element: &Element) -> Result<Node, ContentError> {
    attributes(
        element,
        &["ac:name", "ac:schema-version", "ac:macro-id", "ac:local-id"],
    )?;
    if element.attrs.get("ac:name").map(String::as_str) != Some("code") {
        return Err(failure(element.offset, "unsupported_macro"));
    }
    if element
        .attrs
        .get("ac:schema-version")
        .is_some_and(|value| value != "1")
    {
        return Err(failure(element.offset, "unsupported_macro_version"));
    }
    let mut body = None;
    let mut info = None;
    let mut parameter = false;
    for child in elements(&element.children)? {
        match child.name.as_str() {
            "ac:parameter" if !parameter => {
                attributes(child, &["ac:name"])?;
                if child.attrs.get("ac:name").map(String::as_str) != Some("language") {
                    return Err(failure(child.offset, "unsupported_macro_parameter"));
                }
                info = language(&plain(&child.children, child.offset)?, child.offset)?;
                parameter = true;
            }
            "ac:plain-text-body" if body.is_none() => {
                attributes(child, &[])?;
                body = Some(plain(&child.children, child.offset)?);
            }
            _ => return Err(failure(child.offset, "ambiguous_code_macro")),
        }
    }
    let body = body.ok_or_else(|| failure(element.offset, "missing_code_body"))?;
    let mut node = Node::new(Kind::CodeBlock(info), element.offset);
    if !body.is_empty() {
        node.children
            .push(Node::new(Kind::Text(body), element.offset));
    }
    Ok(node)
}

fn storage_node(element: &Element) -> Result<Node, ContentError> {
    let offset = element.offset;
    if element.name == "ac:structured-macro" {
        return code_macro(element);
    }
    let allowed = match element.name.as_str() {
        "a" => vec!["href"],
        "ol" => vec!["start"],
        "td" | "th" => vec!["colspan", "rowspan"],
        _ => Vec::new(),
    };
    attributes(element, &allowed)?;
    let kind = match element.name.as_str() {
        "p" => Kind::Paragraph,
        "blockquote" => Kind::Quote,
        "ul" => Kind::List(None),
        "li" => Kind::Item,
        "ol" => {
            let start = element
                .attrs
                .get("start")
                .map_or(Ok(1), |value| value.parse::<u64>())
                .map_err(|_| failure(offset, "invalid_list_start"))?;
            if !(1..=999_999_999).contains(&start) {
                return Err(failure(offset, "invalid_list_start"));
            }
            Kind::List(Some(start))
        }
        "h1" => Kind::Heading(1),
        "h2" => Kind::Heading(2),
        "h3" => Kind::Heading(3),
        "h4" => Kind::Heading(4),
        "h5" => Kind::Heading(5),
        "h6" => Kind::Heading(6),
        "strong" | "b" => Kind::Strong,
        "em" | "i" => Kind::Emphasis,
        "a" => {
            let href = element
                .attrs
                .get("href")
                .filter(|value| safe_link(value))
                .ok_or_else(|| failure(offset, "unsupported_link"))?;
            Kind::Link(href.clone())
        }
        "code" | "tt" => {
            return Ok(Node::new(
                Kind::Code(plain(&element.children, offset)?),
                offset,
            ));
        }
        "pre" => {
            let mut node = Node::new(Kind::CodeBlock(None), offset);
            node.children.push(Node::new(
                Kind::Text(plain(&element.children, offset)?),
                offset,
            ));
            return Ok(node);
        }
        "br" => Kind::Break,
        "hr" => Kind::Rule,
        "table" => {
            let mut rows = Vec::new();
            for child in elements(&element.children)? {
                if matches!(child.name.as_str(), "tbody" | "thead") {
                    attributes(child, &[])?;
                    for row in elements(&child.children)? {
                        rows.push(storage_node(row)?);
                    }
                } else {
                    rows.push(storage_node(child)?);
                }
            }
            let columns = rows.first().map_or(0, |row| row.children.len());
            if columns == 0 {
                return Err(failure(offset, "empty_table"));
            }
            let mut node = Node::new(Kind::Table(columns), offset);
            node.children = rows;
            return Ok(node);
        }
        "tr" => {
            let cells = elements(&element.children)?;
            let header = cells.first().is_some_and(|cell| cell.name == "th");
            if cells
                .iter()
                .any(|cell| cell.name != if header { "th" } else { "td" })
            {
                return Err(failure(offset, "mixed_table_header"));
            }
            let mut node = Node::new(Kind::Row(header), offset);
            node.children = cells
                .into_iter()
                .map(storage_node)
                .collect::<Result<_, _>>()?;
            return Ok(node);
        }
        "td" | "th" => {
            if element.attrs.values().any(|value| value != "1") {
                return Err(failure(offset, "merged_table_cell"));
            }
            let mut node = Node::new(Kind::Cell, offset);
            let children = elements(&element.children);
            node.children = if let Ok(children) = children
                && children.len() == 1
                && children[0].name == "p"
            {
                attributes(children[0], &[])?;
                inlines(&children[0].children)?
            } else {
                inlines(&element.children)?
            };
            return Ok(node);
        }
        _ => return Err(failure(offset, "unsupported_storage_element")),
    };
    let mut node = Node::new(kind, offset);
    node.children = match node.kind {
        Kind::Quote => blocks(&element.children)?,
        Kind::List(_) => blocks(&element.children)?,
        Kind::Item => {
            let mut children = Vec::new();
            for child in &element.children {
                match child {
                    XmlChild::Text(text, offset) => {
                        children.push(Node::new(Kind::Text(text.clone()), *offset))
                    }
                    XmlChild::Element(element) => children.push(storage_node(element)?),
                }
            }
            children
        }
        Kind::Break | Kind::Rule if element.children.is_empty() => Vec::new(),
        Kind::Break | Kind::Rule => return Err(failure(offset, "unsupported_void_content")),
        _ => inlines(&element.children)?,
    };
    Ok(node)
}

impl Document {
    /// Imports a storage fragment without regex editing, entity expansion or macro execution.
    pub fn from_storage(source: &str) -> Result<Self, ContentError> {
        if source.len() > MAX_OUTPUT_BYTES {
            return Err(failure(0, "input_byte_limit"));
        }
        if let Some(offset) = invalid_character(source) {
            return Err(failure(offset, "invalid_document_character"));
        }
        let mut reader = Reader::from_str(source);
        reader.config_mut().check_comments = true;
        let mut stack = vec![Element {
            name: String::new(),
            attrs: BTreeMap::new(),
            children: Vec::new(),
            offset: 0,
        }];
        let mut events = 0;
        loop {
            let offset = usize::try_from(reader.buffer_position())
                .map_err(|_| failure(0, "input_byte_limit"))?;
            let event = reader
                .read_event()
                .map_err(|_| failure(offset, "invalid_storage"))?;
            if matches!(event, XmlEvent::Eof) {
                break;
            }
            events += 1;
            if events > MAX_EVENTS {
                return Err(failure(offset, "event_limit"));
            }
            let child = match event {
                XmlEvent::Start(start) => {
                    if stack.len() >= MAX_DEPTH {
                        return Err(failure(offset, "depth_limit"));
                    }
                    stack.push(element(&start, offset)?);
                    continue;
                }
                XmlEvent::Empty(start) => XmlChild::Element(element(&start, offset)?),
                XmlEvent::End(_) => {
                    if stack.len() < 2 {
                        return Err(failure(offset, "invalid_storage"));
                    }
                    XmlChild::Element(
                        stack
                            .pop()
                            .ok_or_else(|| failure(offset, "invalid_storage"))?,
                    )
                }
                XmlEvent::Text(text) => XmlChild::Text(
                    text.xml10_content()
                        .map_err(|_| failure(offset, "invalid_storage_text"))?
                        .into_owned(),
                    offset,
                ),
                XmlEvent::CData(text) => XmlChild::Text(
                    text.xml10_content()
                        .map_err(|_| failure(offset, "invalid_storage_text"))?
                        .into_owned(),
                    offset,
                ),
                XmlEvent::GeneralRef(reference) => {
                    let text = if let Some(character) = reference
                        .resolve_char_ref()
                        .map_err(|_| failure(offset, "invalid_storage_entity"))?
                    {
                        character.to_string()
                    } else {
                        match reference
                            .decode()
                            .map_err(|_| failure(offset, "invalid_storage_entity"))?
                            .as_ref()
                        {
                            "amp" => "&",
                            "lt" => "<",
                            "gt" => ">",
                            "quot" => "\"",
                            "apos" => "'",
                            "nbsp" => "\u{a0}",
                            _ => return Err(failure(offset, "unsupported_storage_entity")),
                        }
                        .into()
                    };
                    XmlChild::Text(text, offset)
                }
                _ => return Err(failure(offset, "unsupported_storage_declaration")),
            };
            if let XmlChild::Text(text, offset) = &child
                && !valid_characters(text)
            {
                return Err(failure(*offset, "invalid_document_character"));
            }
            stack
                .last_mut()
                .ok_or_else(|| failure(offset, "invalid_storage"))?
                .children
                .push(child);
        }
        if stack.len() != 1 {
            return Err(failure(source.len(), "invalid_storage"));
        }
        let root = stack.pop().ok_or_else(|| failure(0, "invalid_storage"))?;
        let mut node = Node::new(Kind::Root, 0);
        node.children = blocks(&root.children)?;
        check_tree(node)
    }
}

fn fields(value: &Value, allowed: &[&str], path: &str) -> Result<(), ContentError> {
    let object = value
        .as_object()
        .ok_or_else(|| at(path, "expected_adf_object"))?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(at(path, "unsupported_adf_property"));
    }
    Ok(())
}

fn contents<'a>(value: &'a Value, path: &str) -> Result<&'a [Value], ContentError> {
    match value.get("content") {
        None => Ok(&[]),
        Some(Value::Array(content)) => Ok(content),
        _ => Err(at(path, "expected_adf_content")),
    }
}

fn children(value: &Value, path: &str, depth: usize) -> Result<Vec<Node>, ContentError> {
    contents(value, path)?
        .iter()
        .enumerate()
        .map(|(index, child)| adf_node(child, &format!("{path}/content/{index}"), depth + 1))
        .collect()
}

fn adf_node(value: &Value, path: &str, depth: usize) -> Result<Node, ContentError> {
    let mut node = adf_node_value(value, path, depth)?;
    node.path = Some(path.into());
    Ok(node)
}

fn adf_node_value(value: &Value, path: &str, depth: usize) -> Result<Node, ContentError> {
    if depth >= MAX_DEPTH {
        return Err(at(path, "depth_limit"));
    }
    let name = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| at(path, "missing_adf_type"))?;
    if name == "text" {
        return adf_text(value, path);
    }
    fields(
        value,
        if name == "doc" {
            &["type", "version", "content"]
        } else {
            &["type", "attrs", "content"]
        },
        path,
    )?;
    let attrs = value.get("attrs").cloned().unwrap_or_else(|| json!({}));
    let kind = match name {
        "doc" if value.get("version") == Some(&json!(1)) && depth == 0 => {
            if value.get("content").is_none() {
                return Err(at(path, "missing_adf_content"));
            }
            fields(&attrs, &[], path)?;
            Kind::Root
        }
        "paragraph" => {
            fields(&attrs, &[], path)?;
            Kind::Paragraph
        }
        "heading" => {
            fields(&attrs, &["level"], path)?;
            let level = attrs["level"]
                .as_u64()
                .filter(|level| (1..=6).contains(level))
                .ok_or_else(|| at(path, "invalid_heading_level"))?;
            Kind::Heading(level as u8)
        }
        "blockquote" => {
            fields(&attrs, &[], path)?;
            Kind::Quote
        }
        "bulletList" => {
            fields(&attrs, &[], path)?;
            Kind::List(None)
        }
        "orderedList" => {
            fields(&attrs, &["order"], path)?;
            let start = attrs
                .get("order")
                .map_or(Some(1), Value::as_u64)
                .filter(|number| (1..=999_999_999).contains(number))
                .ok_or_else(|| at(path, "invalid_list_start"))?;
            Kind::List(Some(start))
        }
        "listItem" => {
            fields(&attrs, &[], path)?;
            if contents(value, path)?.is_empty() {
                return Err(at(path, "missing_adf_content"));
            }
            Kind::Item
        }
        "hardBreak" | "rule" => {
            fields(&attrs, &[], path)?;
            if value.get("content").is_some() {
                return Err(at(path, "unsupported_void_content"));
            }
            if name == "rule" {
                Kind::Rule
            } else {
                Kind::Break
            }
        }
        "codeBlock" => {
            fields(&attrs, &["language"], path)?;
            let info = match attrs.get("language") {
                None => None,
                Some(Value::String(info)) => {
                    language(info, 0).map_err(|error| at(path, error.reason))?
                }
                _ => return Err(at(path, "unsupported_code_info")),
            };
            Kind::CodeBlock(info)
        }
        "table" => {
            fields(&attrs, &["layout", "isNumberColumnEnabled"], path)?;
            if attrs
                .get("layout")
                .is_some_and(|layout| layout != "default")
                || attrs
                    .get("isNumberColumnEnabled")
                    .is_some_and(|flag| flag != false)
            {
                return Err(at(path, "unsupported_table_layout"));
            }
            let rows = children(value, path, depth)?;
            let columns = rows.first().map_or(0, |row| row.children.len());
            if columns == 0 {
                return Err(at(path, "empty_table"));
            }
            let mut node = Node::new(Kind::Table(columns), 0);
            node.children = rows;
            node.finish().map_err(|error| {
                if error.path.is_some() {
                    error
                } else {
                    at(path, error.reason)
                }
            })?;
            return Ok(node);
        }
        "tableRow" => {
            fields(&attrs, &[], path)?;
            let cells = contents(value, path)?;
            let header = cells
                .first()
                .is_some_and(|cell| cell["type"] == "tableHeader");
            if cells
                .iter()
                .any(|cell| cell["type"] != if header { "tableHeader" } else { "tableCell" })
            {
                return Err(at(path, "mixed_table_header"));
            }
            Kind::Row(header)
        }
        "tableCell" | "tableHeader" => {
            fields(&attrs, &["colspan", "rowspan", "colwidth"], path)?;
            if ["colspan", "rowspan"]
                .iter()
                .any(|key| attrs.get(*key).is_some_and(|value| value != 1))
                || attrs.get("colwidth").is_some_and(|width| !width.is_null())
            {
                return Err(at(path, "merged_or_sized_table_cell"));
            }
            let mut content = children(value, path, depth)?;
            if content.len() != 1 || !matches!(content[0].kind, Kind::Paragraph) {
                return Err(at(path, "unsupported_cell_structure"));
            }
            let mut node = Node::new(Kind::Cell, 0);
            node.children = std::mem::take(&mut content[0].children);
            return Ok(node);
        }
        _ => return Err(at(path, "unsupported_adf_node")),
    };
    let mut node = Node::new(kind, 0);
    node.children = children(value, path, depth)?;
    if matches!(node.kind, Kind::Item) && node.children.iter().any(Node::inline) {
        return Err(at(path, "unsupported_list_content"));
    }
    node.finish().map_err(|error| {
        if error.path.is_some() {
            error
        } else {
            at(path, error.reason)
        }
    })?;
    Ok(node)
}

fn adf_text(value: &Value, path: &str) -> Result<Node, ContentError> {
    fields(value, &["type", "text", "marks"], path)?;
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| at(path, "invalid_adf_text"))?;
    if !valid_characters(text) {
        return Err(at(path, "invalid_document_character"));
    }
    let marks = match value.get("marks") {
        None => &[][..],
        Some(Value::Array(marks)) => marks.as_slice(),
        _ => return Err(at(path, "invalid_adf_marks")),
    };
    let mut seen = BTreeSet::new();
    let mut wrappers = Vec::new();
    let mut code = false;
    for (index, mark) in marks.iter().enumerate() {
        let path = format!("{path}/marks/{index}");
        fields(mark, &["type", "attrs"], &path)?;
        let name = mark["type"]
            .as_str()
            .ok_or_else(|| at(&path, "invalid_adf_mark"))?;
        if !seen.insert(name) {
            return Err(at(&path, "duplicate_adf_mark"));
        }
        let attrs = mark.get("attrs").cloned().unwrap_or_else(|| json!({}));
        let kind = match name {
            "strong" => {
                fields(&attrs, &[], &path)?;
                Kind::Strong
            }
            "em" => {
                fields(&attrs, &[], &path)?;
                Kind::Emphasis
            }
            "link" => {
                fields(&attrs, &["href"], &path)?;
                Kind::Link(
                    attrs["href"]
                        .as_str()
                        .filter(|href| safe_link(href))
                        .ok_or_else(|| at(&path, "unsupported_link"))?
                        .into(),
                )
            }
            "code" => {
                fields(&attrs, &[], &path)?;
                code = true;
                continue;
            }
            _ => return Err(at(&path, "unsupported_adf_mark")),
        };
        wrappers.push(kind);
    }
    if code && !wrappers.is_empty() {
        return Err(at(path, "unsupported_formatted_code"));
    }
    let mut node = Node::new(
        if code {
            Kind::Code(text.into())
        } else {
            Kind::Text(text.into())
        },
        0,
    );
    for kind in wrappers.into_iter().rev() {
        let mut parent = Node::new(kind, 0);
        parent.children.push(node);
        node = parent;
    }
    Ok(node)
}

impl Document {
    /// Imports strict ADF JSON, refusing unsupported nodes instead of dropping them.
    pub fn from_adf(source: &[u8]) -> Result<Self, ContentError> {
        let value = craft_core::input::parse_json(source, MAX_OUTPUT_BYTES)
            .map_err(|_| at("", "invalid_adf_json"))?;
        let root = adf_node(&value, "", 0)?;
        if !matches!(root.kind, Kind::Root) {
            return Err(at("", "expected_adf_document"));
        }
        check_tree(root)
    }
}
