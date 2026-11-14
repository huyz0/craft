use super::*;

fn checked(children: Vec<Node>) -> Result<Document, ContentError> {
    let mut root = Node::new(Kind::Root, 0);
    root.children = children;
    super::import::check_tree(root)
}

pub(crate) fn literal(text: &str) -> Result<Document, ContentError> {
    let mut paragraph = Node::new(Kind::Paragraph, 0);
    if !text.is_empty() {
        paragraph
            .children
            .push(Node::new(Kind::Text(text.into()), 0));
    }
    checked(vec![paragraph])
}

pub(crate) fn heading(level: u8, text: &str) -> Result<Document, ContentError> {
    if !(1..=6).contains(&level) {
        return Err(failure(0, "invalid_heading_level"));
    }
    let mut heading = Node::new(Kind::Heading(level), 0);
    heading.children.push(Node::new(Kind::Text(text.into()), 0));
    checked(vec![heading])
}

pub(crate) fn join(documents: &[Document]) -> Result<Document, ContentError> {
    checked(
        documents
            .iter()
            .flat_map(|document| document.root.children.clone())
            .collect(),
    )
}

pub(crate) fn paragraph(document: &Document) -> bool {
    document.root.children.len() == 1 && matches!(document.root.children[0].kind, Kind::Paragraph)
}

pub(crate) fn has_text(document: &Document) -> bool {
    fn any(node: &Node) -> bool {
        match &node.kind {
            Kind::Text(text) | Kind::Code(text) => !text.trim().is_empty(),
            _ => node.children.iter().any(any),
        }
    }
    any(&document.root)
}

pub(crate) fn headings(document: &Document) -> Result<Vec<(u8, String)>, ContentError> {
    document
        .root
        .children
        .iter()
        .filter_map(|node| match node.kind {
            Kind::Heading(level) => Some(literal_heading(node).map(|text| (level, text))),
            _ => None,
        })
        .collect()
}

fn literal_heading(node: &Node) -> Result<String, ContentError> {
    let mut root = Node::new(Kind::Root, node.offset);
    root.children.push(node.clone());
    Document { root }.plain_text()
}

pub(crate) fn table(headers: &[&str], rows: Vec<Vec<Document>>) -> Result<Document, ContentError> {
    let mut table = Node::new(Kind::Table(headers.len()), 0);
    let header = headers
        .iter()
        .map(|text| literal(text))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, row) in std::iter::once(header).chain(rows).enumerate() {
        let mut node = Node::new(Kind::Row(index == 0), 0);
        for value in row {
            if !paragraph(&value) {
                return Err(failure(0, "expected_inline_table_value"));
            }
            let mut cell = Node::new(Kind::Cell, 0);
            cell.children = value.root.children[0].children.clone();
            node.children.push(cell);
        }
        table.children.push(node);
    }
    checked(vec![table])
}

pub(crate) fn table_rows(document: &Document) -> Result<Vec<Vec<Document>>, ContentError> {
    if document.root.children.len() != 1 {
        return Err(failure(0, "expected_single_table"));
    }
    let table = &document.root.children[0];
    if !matches!(table.kind, Kind::Table(_)) {
        return Err(table.error("expected_single_table"));
    }
    let mut rows = Vec::new();
    for (index, row) in table.children.iter().enumerate() {
        if !matches!(row.kind,Kind::Row(header) if header==(index==0)) {
            return Err(row.error("ambiguous_table_header"));
        }
        let mut values = Vec::new();
        for cell in &row.children {
            let mut paragraph = Node::new(Kind::Paragraph, cell.offset);
            paragraph.path = cell.path.clone();
            paragraph.children = cell.children.clone();
            values.push(checked(vec![paragraph])?);
        }
        rows.push(values);
    }
    Ok(rows)
}

pub(crate) fn bullets(items: &[Document]) -> Result<Document, ContentError> {
    let mut list = Node::new(Kind::List(None), 0);
    for item in items {
        if !paragraph(item) {
            return Err(failure(0, "expected_paragraph_criterion"));
        }
        let mut node = Node::new(Kind::Item, 0);
        node.children = item.root.children.clone();
        list.children.push(node);
    }
    checked(vec![list])
}

pub(crate) fn bullet_items(document: &Document) -> Result<Vec<Document>, ContentError> {
    if document.root.children.len() != 1
        || !matches!(document.root.children[0].kind, Kind::List(None))
    {
        return Err(failure(0, "expected_acceptance_list"));
    }
    document.root.children[0]
        .children
        .iter()
        .map(|item| {
            if item.children.len() != 1 || !matches!(item.children[0].kind, Kind::Paragraph) {
                return Err(item.error("expected_paragraph_criterion"));
            }
            checked(item.children.clone())
        })
        .collect()
}
