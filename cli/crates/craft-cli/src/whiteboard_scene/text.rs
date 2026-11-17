//! Plain scene text to and from the editor's Atlassian Document Format (ADF).
//!
//! One line per block: `# ` to `###### ` make headings, `- ` or `* ` bullet items, `1. ` numbered
//! items, an empty line an empty paragraph, anything else a paragraph. Inline formatting needs
//! `adf`. Decoding returns plain text only when encoding it again reproduces the same document.

use serde_json::{Value, json};

pub fn to_adf(text: &str) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some((level, rest)) = heading(line) {
            blocks.push(json!({"type":"heading","attrs":{"level":level},"content":inline(rest)}));
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            push_item(&mut blocks, "bulletList", rest);
        } else if let Some(rest) = ordered(line) {
            push_item(&mut blocks, "orderedList", rest);
        } else {
            blocks.push(json!({"type":"paragraph","content":inline(line)}));
        }
    }
    json!({"version":1,"type":"doc","content":blocks})
}

pub fn from_adf(document: &Value) -> Option<String> {
    let blocks = document.get("content")?.as_array()?;
    if blocks.is_empty() {
        return Some(String::new());
    }
    let mut lines = Vec::new();
    for block in blocks {
        match block.get("type")?.as_str()? {
            "paragraph" => lines.push(plain(block)?),
            "heading" => {
                let level = block.pointer("/attrs/level")?.as_u64()?;
                if !(1..=6).contains(&level) {
                    return None;
                }
                lines.push(format!("{} {}", "#".repeat(level as usize), plain(block)?));
            }
            kind @ ("bulletList" | "orderedList") => {
                for item in block.get("content")?.as_array()? {
                    let paragraphs = item.get("content")?.as_array()?;
                    let [paragraph] = paragraphs.as_slice() else {
                        return None;
                    };
                    if paragraph.get("type")?.as_str()? != "paragraph" {
                        return None;
                    }
                    let marker = if kind == "bulletList" { "- " } else { "1. " };
                    lines.push(format!("{marker}{}", plain(paragraph)?));
                }
            }
            _ => return None,
        }
    }
    let text = lines.join("\n");
    (to_adf(&text) == *document).then_some(text)
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    let rest = line.get(level..)?.strip_prefix(' ')?;
    (1..=6).contains(&level).then_some((level, rest))
}

fn ordered(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    (digits > 0).then(|| line[digits..].strip_prefix(". "))?
}

fn inline(text: &str) -> Value {
    if text.is_empty() {
        json!([])
    } else {
        json!([{"type":"text","text":text}])
    }
}

fn push_item(blocks: &mut Vec<Value>, kind: &str, text: &str) {
    let item = json!({"type":"listItem","content":[{"type":"paragraph","content":inline(text)}]});
    if let Some(last) = blocks.last_mut()
        && last["type"] == kind
        && let Some(items) = last["content"].as_array_mut()
    {
        items.push(item);
        return;
    }
    blocks.push(json!({"type":kind,"content":[item]}));
}

/// Text of a block whose children are unmarked text nodes.
fn plain(block: &Value) -> Option<String> {
    let mut text = String::new();
    for node in block.get("content")?.as_array()? {
        if node.get("type")?.as_str()? != "text" || node.get("marks").is_some() {
            return None;
        }
        text.push_str(node.get("text")?.as_str()?);
    }
    Some(text)
}

/// A scene-supplied ADF value must at least be a version 1 document with block content.
pub fn valid_document(document: &Value) -> bool {
    document.get("type") == Some(&json!("doc"))
        && document.get("version") == Some(&json!(1))
        && document.get("content").is_some_and(Value::is_array)
}
