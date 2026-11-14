use craft_core::contracts::Envelope;
use craft_core::error::ErrorCode;
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::response_schema;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
enum RichValue {
    Text(String),
    Structured { format: String, value: String },
}
#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RichRecord {
    id: String,
    body: RichValue,
    optional: Option<RichValue>,
}

#[test]
fn heterogeneous_values_are_selectable_whole_without_variant_dependent_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = response_schema::<RichRecord>()?;
    let contract = ProjectionContract {
        item_pointer: String::new(),
        always_returned: vec!["id".into()],
        default_selected: vec!["body".into()],
        mode: ProjectionMode::Local,
    };
    let selectable = contract.selectable_fields(&schema)?;
    assert!(selectable.contains(&"body".into()));
    assert!(!selectable.contains(&"body.format".into()));
    assert_eq!(
        contract
            .compile(&schema, &fields(&["body.format"]))
            .err()
            .ok_or("Accepted variant-dependent projection")?
            .code,
        ErrorCode::FieldUnknown
    );
    for body in [
        RichValue::Text("plain".into()),
        RichValue::Structured {
            format: "storage".into(),
            value: "<p>text</p>".into(),
        },
    ] {
        let data = serde_json::to_value(RichRecord {
            id: "document".into(),
            body,
            optional: None,
        })?;
        let projection = contract.compile(&schema, &fields(&["body", "optional"]))?;
        assert_eq!(projection.data(&data)?, data);
        let selected = projection.response_schema(&schema)?;
        assert_eq!(
            selected["properties"]["data"]["properties"]["body"],
            schema["properties"]["data"]["properties"]["body"]
        );
        assert!(
            jsonschema::validator_for(&selected)?.is_valid(&serde_json::to_value(
                Envelope::success("document.get", "request".into(), data)
            )?)
        );
    }
    Ok(())
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Revision {
    number: u64,
    author: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Label {
    name: String,
    description: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Record {
    id: String,
    title: Option<String>,
    revision: Option<Revision>,
    labels: Vec<Label>,
    body: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Listing {
    items: Vec<Record>,
    source: String,
}

fn contract() -> ProjectionContract {
    ProjectionContract {
        item_pointer: "/items".into(),
        always_returned: vec!["id".into()],
        default_selected: vec!["id".into(), "title".into()],
        mode: ProjectionMode::Local,
    }
}

fn fields(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).into()).collect()
}

#[test]
fn projects_children_and_array_elements_without_siblings() -> Result<(), Box<dyn std::error::Error>>
{
    let schema = response_schema::<Listing>()?;
    let projection = contract().compile(
        &schema,
        &fields(&["title", "revision.number", "labels.name"]),
    )?;
    let data = json!({"source":"cached_fetch","items":[{
        "id":"page-one","title":null,"revision":{"number":4,"author":"editor"},
        "labels":[{"name":"review","description":"unselected"}],"body":"not requested"
    },{"id":"page-two","revision":null,"labels":[]}]});
    let selected = projection.data(&data)?;
    assert_eq!(
        selected,
        json!({"source":"cached_fetch","items":[{
        "id":"page-one","title":null,"revision":{"number":4},"labels":[{"name":"review"}]
    },{"id":"page-two","revision":null,"labels":[]}]})
    );
    let selected_schema = projection.response_schema(&schema)?;
    let validator = jsonschema::validator_for(&selected_schema)?;
    let envelope =
        serde_json::to_value(Envelope::success("sample.list", "request".into(), selected))?;
    assert!(validator.is_valid(&envelope));
    let mut leaked = envelope.clone();
    leaked["data"]["items"][0]["revision"]["author"] = json!("editor");
    assert!(!validator.is_valid(&leaked));
    let mut missing_control = envelope;
    missing_control
        .as_object_mut()
        .ok_or("Expected envelope")?
        .remove("request_id");
    assert!(!validator.is_valid(&missing_control));
    Ok(())
}

#[test]
fn defaults_exclude_bodies_and_parent_selection_keeps_declared_children()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = response_schema::<Listing>()?;
    let data = json!({"source":"live","items":[{"id":"page-one","title":"Plan","body":"large","revision":{"number":3,"author":"editor"}}]});
    let defaults = contract().compile(&schema, &[])?.data(&data)?;
    assert_eq!(
        defaults,
        json!({"source":"live","items":[{"id":"page-one","title":"Plan"}]})
    );
    let parent = contract()
        .compile(&schema, &fields(&["revision"]))?
        .data(&data)?;
    assert_eq!(
        parent["items"][0],
        json!({"id":"page-one","revision":{"number":3,"author":"editor"}})
    );
    assert!(parent["items"][0].get("title").is_none());
    Ok(())
}

#[test]
fn refuses_unknown_dynamic_indexed_wildcard_and_duplicate_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = response_schema::<Listing>()?;
    let error = contract()
        .compile(&schema, &fields(&["revision.missing"]))
        .err()
        .ok_or("Expected unknown field")?;
    assert_eq!(error.code, ErrorCode::FieldUnknown);
    for paths in [
        vec!["labels.0.name"],
        vec!["labels.*"],
        vec!["body.child"],
        vec!["id", "id"],
        vec!["revision..number"],
    ] {
        assert!(contract().compile(&schema, &fields(&paths)).is_err());
    }
    Ok(())
}

#[test]
fn selected_schema_retains_error_variants_and_has_its_own_digest()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = response_schema::<Listing>()?;
    let selected = contract()
        .compile(&schema, &fields(&["title"]))?
        .response_schema(&schema)?;
    assert_eq!(
        schema["properties"]["error"],
        selected["properties"]["error"]
    );
    assert_eq!(schema["oneOf"], selected["oneOf"]);
    assert_ne!(
        craft_core::input::canonical_digest(&schema)?,
        craft_core::input::canonical_digest(&selected)?
    );
    let mut ambiguous = schema;
    ambiguous["properties"]["data"]["properties"]["items"]["items"] = json!({"anyOf":[{"type":"object","properties":{"id":{"type":"string"}}},{"type":"object","properties":{"id":{"type":"integer"}}}]});
    assert!(contract().compile(&ambiguous, &fields(&["id"])).is_err());
    Ok(())
}
