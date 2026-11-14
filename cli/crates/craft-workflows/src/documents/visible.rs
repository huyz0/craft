use std::collections::BTreeSet;

use super::*;

pub struct VisibleSection {
    pub level: u8,
    pub heading: String,
    pub body: Document,
    pub source_offset: usize,
    pub source_path: Option<String>,
    original_heading: Node,
}

impl VisibleSection {
    pub(crate) fn document(&self) -> Result<Document, ContentError> {
        let mut root = Node::new(Kind::Root, self.source_offset);
        root.children.push(self.original_heading.clone());
        root.children.extend(self.body.root.children.clone());
        super::import::check_tree(root)
    }
}

pub struct VisibleField {
    pub name: String,
    pub value: Document,
    pub source_offset: usize,
    pub source_path: Option<String>,
}

fn inline_text(node: &Node, out: &mut String) -> Result<(), ContentError> {
    match &node.kind {
        Kind::Text(text) | Kind::Code(text) => out.push_str(text),
        Kind::Break => out.push('\n'),
        Kind::Strong | Kind::Emphasis => {
            for child in &node.children {
                inline_text(child, out)?;
            }
        }
        _ => return Err(node.error("ambiguous_plain_text")),
    }
    Ok(())
}

fn label(node: &Node) -> Result<String, ContentError> {
    let mut text = String::new();
    for child in &node.children {
        inline_text(child, &mut text)?;
    }
    Ok(text)
}

impl Document {
    /// Splits visible heading sections without consulting hidden/local plan metadata.
    /// Preamble content is refused because dropping it would lose a human edit.
    pub fn sections(&self) -> Result<Vec<VisibleSection>, ContentError> {
        let mut sections: Vec<VisibleSection> = Vec::new();
        for node in &self.root.children {
            if let Kind::Heading(level) = node.kind {
                let heading = label(node)?;
                if heading.trim().is_empty() {
                    return Err(node.error("empty_section_heading"));
                }
                let mut root = Node::new(Kind::Root, node.offset);
                root.path = node.path.clone();
                sections.push(VisibleSection {
                    original_heading: node.clone(),
                    level,
                    heading,
                    body: Document { root },
                    source_offset: node.offset,
                    source_path: node.path.clone(),
                });
            } else {
                sections
                    .last_mut()
                    .ok_or_else(|| node.error("missing_section_heading"))?
                    .body
                    .root
                    .children
                    .push(node.clone());
            }
        }
        Ok(sections)
    }

    /// Reads one visible Field/Value table; duplicate names and extra blocks are ambiguous.
    /// Rich field values remain documents rather than being flattened to link labels.
    pub fn field_table(&self) -> Result<Vec<VisibleField>, ContentError> {
        if self.root.children.len() != 1 {
            return Err(self.root.error("ambiguous_field_table"));
        }
        let table = &self.root.children[0];
        if !matches!(table.kind, Kind::Table(2)) {
            return Err(table.error("expected_field_value_table"));
        }
        let header = table
            .children
            .first()
            .ok_or_else(|| table.error("expected_field_value_table"))?;
        if !matches!(header.kind, Kind::Row(true))
            || label(&header.children[0])?.trim() != "Field"
            || label(&header.children[1])?.trim() != "Value"
        {
            return Err(header.error("expected_field_value_header"));
        }
        let mut names = BTreeSet::new();
        let mut fields = Vec::new();
        for row in table.children.iter().skip(1) {
            if !matches!(row.kind, Kind::Row(false)) {
                return Err(row.error("ambiguous_field_row"));
            }
            let name = label(&row.children[0])?.trim().to_owned();
            if name.is_empty() || !names.insert(name.to_lowercase()) {
                return Err(row.error("ambiguous_field_name"));
            }
            let mut root = Node::new(Kind::Root, row.offset);
            root.path = row.path.clone();
            let mut paragraph = Node::new(Kind::Paragraph, row.offset);
            paragraph.path = row.path.clone();
            paragraph.children = row.children[1].children.clone();
            root.children.push(paragraph);
            fields.push(VisibleField {
                name,
                value: Document { root },
                source_offset: row.offset,
                source_path: row.path.clone(),
            });
        }
        Ok(fields)
    }

    /// Reads a single text block. Links and structural flattening require an explicit policy.
    pub fn plain_text(&self) -> Result<String, ContentError> {
        if self.root.children.len() != 1 {
            return Err(self.root.error("ambiguous_plain_text"));
        }
        let node = &self.root.children[0];
        if !matches!(
            node.kind,
            Kind::Paragraph | Kind::Heading(_) | Kind::CodeBlock(_)
        ) {
            return Err(node.error("ambiguous_plain_text"));
        }
        label(node)
    }
}
