use super::*;
use serde_json::{Value, json};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn valid(value: Value) -> bool {
    serde_json::from_value::<Document>(value).is_ok_and(|doc| doc.validate().is_ok())
}
fn document(content: Value) -> Value {
    json!({"type":"doc","version":1,"content":content})
}
#[test]
fn supported_blocks_and_marks_preserve_rich_content_and_unicode() -> TestResult {
    let input = document(json!([
        {"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"設計 🛠"}]},
        {"type":"paragraph","content":[{"type":"text","text":"Important","marks":[{"type":"strong"},{"type":"em"},{"type":"underline"},{"type":"strike"}]},{"type":"hardBreak"},{"type":"text","text":"source","marks":[{"type":"link","attrs":{"href":"https://example.com/design?a=b","title":"Design"}},{"type":"code"}]}]},
        {"type":"blockquote","content":[{"type":"paragraph","content":[]}]},
        {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[]},{"type":"orderedList","attrs":{"order":3},"content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"Nested"}]}]}]}]}]},
        {"type":"codeBlock","attrs":{"language":"rust"},"content":[{"type":"text","text":"fn main() {\n\tprintln!(\"hello\");\n}"}]},
        {"type":"rule"}
    ]));
    let parsed: Document = serde_json::from_value(input.clone())?;
    parsed.validate()?;
    assert_eq!(serde_json::to_value(&parsed)?, input);
    assert!(
        jsonschema::validator_for(&craft_core::registry::schema_for::<Document>()?)?
            .is_valid(&input)
    );
    Ok(())
}
#[test]
fn unknown_nodes_attributes_and_illegal_content_fail_instead_of_being_dropped() {
    for node in [
        json!({"type":"paragraph","attrs":{"unsupported":true},"content":[]}),
        json!({"type":"rule","attrs":{"unsupported":true}}),
        json!({"type":"paragraph","content":[{"type":"hardBreak","attrs":{"unsupported":true}}]}),
        json!({"type":"codeBlock","content":[{"type":"text","text":"code","marks":[{"type":"strong"}]}]}),
        json!({"type":"bulletList","content":[{"type":"listItem","content":[{"type":"heading","attrs":{"level":1},"content":[]}]}]}),
        json!({"type":"orderedList","attrs":{"order":0},"content":[]}),
        json!({"type":"paragraph","content":[{"type":"text","text":"a","marks":[{"type":"strong","attrs":{"unsupported":true}}]}]}),
        json!({"type":"paragraph","content":[{"type":"text","text":"a","marks":[{"type":"strong"},{"type":"strong"}]}]}),
        json!({"type":"paragraph","content":[{"type":"text","text":"a","marks":[{"type":"strong"},{"type":"code"}]}]}),
        json!({"type":"mediaSingle","content":[]}),
    ] {
        assert!(!valid(document(json!([node]))), "{node}");
    }
}
#[test]
fn depth_node_text_and_encoded_byte_limits_reject_unbounded_documents() {
    assert!(!valid(document(json!([]))));
    assert!(!valid(document(
        json!([{"type":"paragraph","content":[{"type":"text","text":"x".repeat(32769)}]}])
    )));
    assert!(!valid(document(
        json!([{"type":"paragraph","content":[{"type":"text","text":"line\nbreak"}]}])
    )));
    let mut nested = json!({"type":"paragraph","content":[]});
    for _ in 0..16 {
        nested = json!({"type":"blockquote","content":[nested]});
    }
    assert!(!valid(document(json!([nested]))));
    let too_many = (0..10_000)
        .map(|_| json!({"type":"hardBreak"}))
        .collect::<Vec<_>>();
    assert!(!valid(document(
        json!([{"type":"paragraph","content":too_many}])
    )));
    let too_large = (0..10)
        .map(|_| json!({"type":"paragraph","content":[{"type":"text","text":"x".repeat(32768)}]}))
        .collect::<Vec<_>>();
    assert!(!valid(document(json!(too_large))));
}
