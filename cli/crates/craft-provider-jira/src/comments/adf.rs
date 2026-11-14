//! Closed authoring types prevent silently stripping unsupported rich content.
use craft_core::error::CraftError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(test)]
mod tests;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DocumentKind {
    Doc,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    #[serde(rename = "type")]
    pub kind: DocumentKind,
    #[schemars(extend("const" = 1))]
    pub version: u8,
    #[schemars(length(min = 1, max = 2000))]
    pub content: Vec<Block>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Block {
    #[serde(rename = "paragraph")]
    Paragraph {
        #[serde(default)]
        content: Vec<Inline>,
    },
    #[serde(rename = "heading")]
    Heading {
        attrs: HeadingAttributes,
        #[serde(default)]
        content: Vec<Inline>,
    },
    #[serde(rename = "blockquote")]
    Blockquote { content: Vec<Block> },
    #[serde(rename = "bulletList")]
    BulletList { content: Vec<ListItem> },
    #[serde(rename = "orderedList")]
    OrderedList {
        attrs: OrderAttributes,
        content: Vec<ListItem>,
    },
    #[serde(rename = "codeBlock")]
    CodeBlock {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attrs: Option<LanguageAttributes>,
        #[serde(default)]
        content: Vec<CodeText>,
    },
    #[serde(rename = "rule")]
    Rule {},
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeadingAttributes {
    #[schemars(range(min = 1, max = 6))]
    pub level: u8,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderAttributes {
    #[schemars(range(min = 1, max = 2147483647))]
    pub order: u32,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageAttributes {
    #[schemars(length(min = 1, max = 128))]
    pub language: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ListItemKind {
    ListItem,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListItem {
    #[serde(rename = "type")]
    pub kind: ListItemKind,
    pub content: Vec<Block>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TextKind {
    Text,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeText {
    #[serde(rename = "type")]
    pub kind: TextKind,
    #[schemars(length(min = 1, max = 32768))]
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Inline {
    #[serde(rename = "text")]
    Text {
        #[schemars(length(min = 1, max = 32768))]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        marks: Vec<Mark>,
    },
    #[serde(rename = "hardBreak")]
    HardBreak {},
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Mark {
    #[serde(rename = "strong")]
    Strong {},
    #[serde(rename = "em")]
    Em {},
    #[serde(rename = "code")]
    Code {},
    #[serde(rename = "strike")]
    Strike {},
    #[serde(rename = "underline")]
    Underline {},
    #[serde(rename = "link")]
    Link { attrs: LinkAttributes },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkAttributes {
    #[schemars(length(min = 1, max = 2048))]
    pub href: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 512))]
    pub title: Option<String>,
}
fn invalid() -> CraftError {
    CraftError::field("/body", "unsupported_or_unbounded_comment_adf")
}
fn text(value: &str, max: usize, allow_lines: bool) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && !(allow_lines && matches!(c, '\n' | '\r' | '\t')))
}
impl Document {
    pub fn validate(&self) -> Result<(), CraftError> {
        if self.version != 1 || self.content.is_empty() || self.content.len() > 2000 {
            return Err(invalid());
        }
        let mut nodes = 1usize;
        blocks(&self.content, 1, &mut nodes)?;
        if serde_json::to_vec(self).map_err(|_| invalid())?.len() > super::BODY_MAX {
            return Err(invalid());
        }
        Ok(())
    }
}
fn count(nodes: &mut usize, extra: usize) -> Result<(), CraftError> {
    *nodes = nodes.checked_add(extra).ok_or_else(invalid)?;
    if *nodes > 10_000 {
        return Err(invalid());
    }
    Ok(())
}
fn blocks(values: &[Block], depth: usize, nodes: &mut usize) -> Result<(), CraftError> {
    if values.is_empty() || depth > 16 {
        return Err(invalid());
    }
    count(nodes, values.len())?;
    for block in values {
        match block {
            Block::Paragraph { content } => inlines(content, nodes)?,
            Block::Heading { attrs, content } => {
                if !(1..=6).contains(&attrs.level) {
                    return Err(invalid());
                }
                inlines(content, nodes)?;
            }
            Block::Blockquote { content } => blocks(content, depth + 1, nodes)?,
            Block::BulletList { content } => items(content, depth, nodes)?,
            Block::OrderedList { attrs, content } => {
                if !(1..=i32::MAX as u32).contains(&attrs.order) {
                    return Err(invalid());
                }
                items(content, depth, nodes)?;
            }
            Block::CodeBlock { attrs, content } => {
                if attrs
                    .as_ref()
                    .is_some_and(|a| !text(&a.language, 128, false))
                    || content.iter().any(|node| !text(&node.text, 32768, true))
                {
                    return Err(invalid());
                }
                count(nodes, content.len())?;
            }
            Block::Rule {} => {}
        }
    }
    Ok(())
}
fn items(values: &[ListItem], depth: usize, nodes: &mut usize) -> Result<(), CraftError> {
    if values.is_empty() {
        return Err(invalid());
    }
    count(nodes, values.len())?;
    for item in values {
        if !matches!(item.content.first(), Some(Block::Paragraph { .. }))
            || item
                .content
                .iter()
                .any(|b| matches!(b, Block::Heading { .. } | Block::Rule {}))
        {
            return Err(invalid());
        }
        blocks(&item.content, depth + 2, nodes)?;
    }
    Ok(())
}
fn inlines(values: &[Inline], nodes: &mut usize) -> Result<(), CraftError> {
    count(nodes, values.len())?;
    for value in values {
        if let Inline::Text { text: value, marks } = value {
            if !text(value, 32768, false) || marks.len() > 6 {
                return Err(invalid());
            }
            let mut seen = std::collections::BTreeSet::new();
            for mark in marks {
                let name = match mark {
                    Mark::Strong {} => "strong",
                    Mark::Em {} => "em",
                    Mark::Code {} => "code",
                    Mark::Strike {} => "strike",
                    Mark::Underline {} => "underline",
                    Mark::Link { attrs } => {
                        let rest = attrs
                            .href
                            .strip_prefix("https://")
                            .or_else(|| attrs.href.strip_prefix("http://"));
                        if !text(&attrs.href, 2048, false)
                            || attrs.href.chars().any(char::is_whitespace)
                            || !rest.is_some_and(|s| !s.is_empty() && !s.starts_with('/'))
                            || attrs
                                .title
                                .as_ref()
                                .is_some_and(|v| v.len() > 512 || v.chars().any(char::is_control))
                        {
                            return Err(invalid());
                        }
                        "link"
                    }
                };
                if !seen.insert(name) {
                    return Err(invalid());
                }
            }
            if seen.contains("code") && seen.iter().any(|m| !matches!(*m, "code" | "link")) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
