use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

use super::scene::*;
use super::vocabulary::*;
use super::{base64, clipboard_html, decode, encode, text};

fn scene(elements: Value) -> Result<Scene, serde_json::Error> {
    serde_json::from_value(json!({"format":"craft.whiteboard.scene.v1","elements":elements}))
}

fn rule(error: craft_core::error::CraftError) -> (String, String) {
    let issue = &error.details.issues[0];
    (issue.path.clone(), issue.rule.clone())
}

#[test]
fn base64_matches_the_standard_alphabet_and_padding() -> TestResult {
    for (plain, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
    ] {
        assert_eq!(base64::encode(plain.as_bytes()), encoded);
        assert_eq!(base64::decode(encoded).ok_or("decode")?, plain.as_bytes());
    }
    let unicode = "✅ Good 💡";
    assert_eq!(
        base64::decode(&base64::encode(unicode.as_bytes())).ok_or("decode")?,
        unicode.as_bytes()
    );
    assert!(base64::decode("not base64!").is_none());
    Ok(())
}

#[test]
fn plain_text_maps_to_headings_lists_and_paragraphs_and_back() -> TestResult {
    let source = "# Title\nBody line\n\n- one\n- two\n1. first\n1. second";
    let document = text::to_adf(source);
    assert_eq!(
        document["content"][0],
        json!({"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"Title"}]})
    );
    assert_eq!(
        document["content"][2],
        json!({"type":"paragraph","content":[]})
    );
    assert_eq!(document["content"][3]["type"], "bulletList");
    assert_eq!(
        document["content"][3]["content"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(document["content"][4]["type"], "orderedList");
    assert_eq!(text::from_adf(&document).as_deref(), Some(source));
    let marked = json!({"version":1,"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"bold","marks":[{"type":"strong"}]}]}]});
    assert_eq!(text::from_adf(&marked), None);
    let ambiguous = json!({"version":1,"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"- not a list"}]}]});
    assert_eq!(
        text::from_adf(&ambiguous),
        None,
        "re-encoding would change the block type"
    );
    Ok(())
}

#[test]
fn encode_places_elements_by_center_and_binds_connectors_by_index() -> TestResult {
    let scene = scene(json!([
        {"kind":"shape","id":"a","shape":"diamond","x":0,"y":0,"w":160,"h":160,"fill":"yellow","text":"OK?"},
        {"kind":"sticky","id":"b","x":400,"y":8,"color":"blue-subtle","text":"Yes"},
        {"kind":"connector","from":"a","to":"b","route":"curved","end_cap":"filled_arrow","dash":"dashed"},
        {"kind":"stamp","sprite":"rocket","on":"b"},
        {"kind":"section","x":-50,"y":-50,"w":700,"h":300,"title":"Decision"},
        {"kind":"line","x1":0,"y1":400,"x2":200,"y2":400,"stroke":"thick","dash":"dotted"},
        {"kind":"text","x":0,"y":500,"w":300,"text":"## Notes"},
        {"kind":"link","url":"https://example.com/browse/KEY-1","x":0,"y":600},
    ]))?;
    let encoded = encode::encode(&scene)?;
    let records = &encoded.records;
    assert_eq!(encoded.connectors, 1);
    assert_eq!(
        records[0]["basisPosition"],
        json!({"x":80.0,"y":80.0,"type":"Vector2"})
    );
    assert_eq!(records[0]["shape"], 4);
    assert_eq!(
        records[0]["color"],
        json!({"x":255,"y":222,"z":92,"type":"Vector3"})
    );
    assert_eq!(
        records[1]["basisPosition"],
        json!({"x":472.0,"y":80.0,"type":"Vector2"})
    );
    assert_eq!(records[2]["sourceIndex"], 0);
    assert_eq!(records[2]["targetIndex"], 1);
    assert_eq!(records[2]["sourceAnchor"], json!({"left":1.0,"top":0.5}));
    assert_eq!(records[2]["targetAnchor"], json!({"left":0.0,"top":0.5}));
    assert_eq!(
        (
            records[2]["presentation"].clone(),
            records[2]["endCap"].clone(),
            records[2]["strokeStyle"].clone()
        ),
        (json!(3), json!(3), json!(2))
    );
    assert_eq!(
        records[3]["attachedTo"],
        json!({"parentIndex":1,"offset":[132.0,12.0]})
    );
    assert_eq!(
        records[4]["position"],
        json!({"x":300.0,"y":100.0,"type":"Vector2"})
    );
    assert_eq!(records[5]["type"], "path");
    assert_eq!(records[5]["start"], json!([0.0, 400.0]));
    let heading: Value = serde_json::from_str(records[6]["text"].as_str().ok_or("text")?)?;
    assert_eq!(heading["content"][0]["attrs"]["level"], 2);
    assert_eq!(records[7]["type"], "smartLink");
    for record in records {
        assert!(record.get("createdBy").is_none());
    }
    let html = clipboard_html(records)?;
    assert!(html.starts_with("<div id=\"canvas-clipboard\" data-canvas-clipboard=\""));
    assert_eq!(&decode::records(html.as_bytes())?, records);
    Ok(())
}

#[test]
fn encode_rejects_invalid_scenes_before_producing_a_payload() -> TestResult {
    let cases = [
        (json!([]), "/elements", "between_1_and_5000_elements"),
        (
            json!([{"kind":"sticky","id":"a","x":0,"y":0},{"kind":"sticky","id":"a","x":0,"y":0}]),
            "/elements/1/id",
            "duplicate_element_id",
        ),
        (
            json!([{"kind":"sticky","id":"bad id","x":0,"y":0}]),
            "/elements/0/id",
            "invalid_element_id",
        ),
        (
            json!([{"kind":"sticky","id":"a","x":0,"y":0},{"kind":"connector","from":"a","to":"missing"}]),
            "/elements/1/to",
            "unknown_element_reference",
        ),
        (
            json!([{"kind":"sticky","id":"a","x":0,"y":0},{"kind":"connector","from":"a","to":"a"}]),
            "/elements/1/to",
            "connector_must_join_two_elements",
        ),
        (
            json!([{"kind":"sticky","x":0,"y":0,"color":"blue-bold"}]),
            "/elements/0/color",
            "not_a_sticky_color",
        ),
        (
            json!([{"kind":"shape","shape":"ellipse","x":0,"y":0,"w":0,"h":10}]),
            "/elements/0/w",
            "size_out_of_range",
        ),
        (
            json!([{"kind":"shape","shape":"ellipse","x":2000000,"y":0,"w":10,"h":10}]),
            "/elements/0/x",
            "coordinate_out_of_range",
        ),
        (
            json!([{"kind":"sticky","x":0,"y":0,"text":"a","adf":{"version":1,"type":"doc","content":[]}}]),
            "/elements/0/adf",
            "text_and_adf_are_exclusive",
        ),
        (
            json!([{"kind":"sticky","x":0,"y":0,"adf":{"type":"paragraph"}}]),
            "/elements/0/adf",
            "invalid_adf_document",
        ),
        (
            json!([{"kind":"stamp","sprite":"rocket"}]),
            "/elements/0/on",
            "stamp_needs_on_or_x_and_y",
        ),
        (
            json!([{"kind":"text","id":"t","x":0,"y":0,"w":100},{"kind":"stamp","sprite":"rocket","on":"t"}]),
            "/elements/1/on",
            "stamps_attach_to_shapes_or_stickies",
        ),
        (
            json!([{"kind":"link","url":"javascript:alert(1)","x":0,"y":0}]),
            "/elements/0/url",
            "invalid_link_url",
        ),
        (
            json!([{"kind":"raw","element":{"x":1}}]),
            "/elements/0/element",
            "raw_element_needs_type",
        ),
        (
            json!([{"kind":"sticky","id":"s","x":0,"y":0},{"kind":"line","id":"l","x1":0,"y1":0,"x2":1,"y2":1},{"kind":"connector","from":"s","to":"l"}]),
            "/elements/2/to",
            "reference_is_not_attachable",
        ),
    ];
    for (elements, path, expected) in cases {
        let error = encode::encode(&scene(elements.clone())?)
            .err()
            .unwrap_or_else(|| panic!("accepted {elements}"));
        assert_eq!(
            rule(error),
            (path.to_owned(), expected.to_owned()),
            "{elements}"
        );
    }
    for unknown in [
        json!({"format":"craft.whiteboard.scene.v1","elements":[{"kind":"shape","shape":"class","x":0,"y":0,"w":1,"h":1}]}),
        json!({"format":"craft.whiteboard.scene.v1","elements":[{"kind":"sticky","x":0,"y":0,"colour":"red-subtle"}]}),
        json!({"format":"craft.whiteboard.scene.v1","elements":[{"kind":"sticky","x":0,"y":0,"color":"#ff0000"}]}),
        json!({"format":"craft.whiteboard.scene.v2","elements":[]}),
        json!({"format":"craft.whiteboard.scene.v1","elements":[{"kind":"frame","x":0,"y":0}]}),
    ] {
        assert!(
            serde_json::from_value::<Scene>(unknown.clone()).is_err(),
            "{unknown}"
        );
    }
    Ok(())
}

/// Records shaped like the editor's own copy output, observed on a live test board.
fn editor_copy() -> Vec<Value> {
    let v2 = |x: f64, y: f64| json!({"x":x,"y":y,"type":"Vector2"});
    let v3 = |r: u8, g: u8, b: u8| json!({"x":r,"y":g,"z":b,"type":"Vector3"});
    vec![
        json!({"type":"shape","source":1,"position":v2(-387.7,367.6),"size":v2(0.0,0.0),"color":v3(255,222,92),"strokeStyle":1,
            "text":"{\"version\":1,\"type\":\"doc\",\"content\":[]}","shape":4,"fillEnabled":true,"fontScale":1,
            "basisSize":v2(178.0,178.0),"basisPosition":v2(-113.77,-71.09),"alignment":"center","verticalAlignment":1,"rotation":0}),
        json!({"type":"sticky","source":1,"position":v2(0.0,0.0),"size":v2(144.0,144.0),"color":v3(255,222,184),
            "text":"{\"version\":1,\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"Idea\"}]}]}",
            "fontScale":1,"basisSize":v2(144.0,144.0),"basisPosition":v2(300.0,0.0),"alignment":"center","verticalAlignment":1,"rotation":0,
            "createdBy":"557058:00000000-0000-0000-0000-000000000000","authorIds":["557058:00000000-0000-0000-0000-000000000000"],"isAuthorVisible":false}),
        json!({"type":"connector","source":1,"presentation":2,"segments":[],"start":[1.0,2.0],"end":[3.0,4.0],"position":v2(0.0,0.0),"size":v2(0.0,0.0),
            "sourceElement":"o-AslJ5HmP0iRRSzhTZm_","targetElement":"JuC9VER2KbuAWgbfZIr15","sourceAnchor":{"left":1,"top":0.5},"targetAnchor":{"left":0.25,"top":0.5},
            "startCap":1,"endCap":2,"color":v3(117,129,149),"stroke":1,"strokeStyle":1,"sourceIndex":0,"targetIndex":1}),
        json!({"type":"stamp","source":1,"position":v2(300.0,0.0),"size":v2(60.0,60.0),"rotation":0,"spriteId":"thumbs-up","attachedTo":{"parentIndex":1,"offset":[72,72]}}),
        json!({"type":"smartLink","ari":"ari:cloud:jira:00000000-0000-0000-0000-000000000000:issue/10003","source":1,"position":v2(0.0,500.0),"size":v2(280.0,306.0),
            "url":"https://example.atlassian.net/browse/KEY-1","kind":"block","supportedViews":["block"],"supportsPreview":true,"unfurledConnections":[],"unfurlState":"idle","rotation":0}),
        json!({"type":"mindmap","position":v2(120.9,1012.0),"size":v2(0.0,0.0),"rootId":"ps3M9zk2IIiagf5HaZeze","ordering":[1],"orientation":0,"source":1,"rootIndex":1}),
        json!({"type":"section","source":1,"position":v2(0.0,0.0),"size":v2(500.0,300.0),"color":v3(179,212,255),"title":"Off palette","titleWidth":142.75,"hasDropShadow":false,"rotation":0}),
    ]
}

#[test]
fn decode_maps_editor_records_removes_identities_and_keeps_unknown_records_raw() -> TestResult {
    let html = clipboard_html(&editor_copy())?;
    let decoded = decode::decode(decode::records(
        format!("<meta charset=\"utf-8\">{html}").as_bytes(),
    )?);
    let elements = &decoded.scene.elements;
    assert_eq!(elements.len(), 7);
    assert_eq!(decoded.raw, 2, "mind map and off-palette section stay raw");
    assert_eq!(
        decoded.private_fields_removed, 3,
        "two sticky identities and one resolved link ARI"
    );
    let Element::Shape(shape) = &elements[0] else {
        panic!("shape")
    };
    assert_eq!(
        (shape.shape, shape.x, shape.y, shape.w, shape.fill),
        (
            ShapeKind::Diamond,
            -202.77,
            -160.09,
            178.0,
            Some(Color::Yellow)
        )
    );
    assert_eq!((shape.text.clone(), shape.adf.clone()), (None, None));
    let Element::Sticky(sticky) = &elements[1] else {
        panic!("sticky")
    };
    assert_eq!(
        (sticky.x, sticky.text.as_deref(), sticky.color),
        (228.0, Some("Idea"), Some(Color::OrangeSubtle))
    );
    let Element::Connector(connector) = &elements[2] else {
        panic!("connector")
    };
    assert_eq!(
        (connector.from.as_str(), connector.to.as_str()),
        ("e1", "e2")
    );
    assert_eq!(connector.from_side, Some(Side::Named(SideName::Right)));
    assert_eq!(
        connector.to_side,
        Some(Side::Point(UnitPoint { x: 0.25, y: 0.5 }))
    );
    let Element::Stamp(stamp) = &elements[3] else {
        panic!("stamp")
    };
    assert_eq!((stamp.on.as_deref(), stamp.x), (Some("e2"), None));
    let Element::Link(link) = &elements[4] else {
        panic!("link")
    };
    assert_eq!((link.x, link.y), (-140.0, 347.0));
    let Element::Raw(mindmap) = &elements[5] else {
        panic!("raw mind map")
    };
    assert_eq!(mindmap.element["rootIndex"], 1);
    let serialized = serde_json::to_string(&decoded.scene)?;
    assert!(
        !serialized.contains("557058"),
        "account IDs must not survive decoding"
    );
    assert!(
        !serialized.contains("ari:cloud"),
        "resolved resource IDs must not survive decoding"
    );

    let again = encode::encode(&decoded.scene)?;
    assert_eq!(again.records.len(), 7);
    assert_eq!(again.records[2]["sourceIndex"], 0);
    assert_eq!(again.records[2]["targetIndex"], 1);
    assert_eq!(again.records[3]["attachedTo"]["parentIndex"], 1);
    assert_eq!(
        again.records[5],
        editor_copy()[5],
        "raw records pass through unchanged"
    );
    assert_eq!(
        decode::decode(again.records).scene,
        decoded.scene,
        "decode, encode, decode is stable"
    );
    Ok(())
}

#[test]
fn decode_rejects_payloads_that_are_not_clipboard_arrays() -> TestResult {
    for source in [
        "",
        "<div>no payload</div>",
        "[]",
        "[1,2]",
        "{\"type\":\"sticky\"}",
        "data-canvas-clipboard=\"!!!\"",
    ] {
        assert!(decode::records(source.as_bytes()).is_err(), "{source}");
    }
    let bare = base64::encode(br#"[{"type":"unknown"}]"#);
    let decoded = decode::decode(decode::records(bare.as_bytes())?);
    assert_eq!(decoded.raw, 1);
    Ok(())
}

#[test]
fn every_named_vocabulary_value_round_trips_through_its_code() -> TestResult {
    for color in Color::ALL {
        assert_eq!(Color::from_code(color.code()), Some(*color));
    }
    assert_eq!(
        Color::ALL.iter().filter(|color| color.is_sticky()).count(),
        8
    );
    for shape in ShapeKind::ALL {
        assert_eq!(ShapeKind::from_code(shape.code()), Some(*shape));
    }
    assert!(ShapeKind::from_code(82).is_none() && ShapeKind::from_code(83).is_none());
    assert_eq!(Cap::ALL.len(), 14);
    Ok(())
}

#[test]
fn labels_groups_and_rotation_use_the_editor_encodings() -> TestResult {
    let scene = scene(json!([
        {"kind":"shape","id":"a","shape":"rectangle","x":0,"y":0,"w":200,"h":100,"rotation":90},
        {"kind":"shape","id":"b","shape":"rectangle","x":400,"y":0,"w":200,"h":100},
        {"kind":"connector","id":"ab","from":"a","to":"b"},
        {"kind":"label","on":"ab","text":"HTTPS","at":0.25},
        {"kind":"group","members":["a","b"]},
    ]))?;
    let records = encode::encode(&scene)?.records;
    let rotation = records[0]["rotation"].as_f64().ok_or("rotation")?;
    assert!(
        (rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-9,
        "degrees become radians"
    );
    assert_eq!(records[3]["type"], "pathLabel");
    assert_eq!(
        (
            records[3]["sourcePathIndex"].clone(),
            records[3]["proportion"].clone()
        ),
        (json!(2), json!(0.25))
    );
    assert_eq!(records[4]["childrenIndices"], json!([0, 1]));
    assert_eq!(records[4]["children"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        records[4]["size"],
        json!({"x":600.0,"y":100.0,"type":"Vector2"})
    );
    assert_eq!(
        records[4]["position"],
        json!({"x":300.0,"y":50.0,"type":"Vector2"})
    );
    let decoded = decode::decode(records).scene;
    let Element::Shape(shape) = &decoded.elements[0] else {
        panic!("shape")
    };
    assert_eq!(shape.rotation, Some(90.0));
    let Element::Label(label) = &decoded.elements[3] else {
        panic!("label")
    };
    assert_eq!(
        (label.on.as_str(), label.text.as_deref(), label.at),
        ("e3", Some("HTTPS"), Some(0.25))
    );
    let Element::Group(group) = &decoded.elements[4] else {
        panic!("group")
    };
    assert_eq!(group.members, vec!["e1".to_owned(), "e2".to_owned()]);
    for (elements, path, expected) in [
        (
            json!([{"kind":"sticky","id":"s","x":0,"y":0},{"kind":"label","on":"s","text":"x"}]),
            "/elements/1/on",
            "labels_attach_to_connectors_or_lines",
        ),
        (
            json!([{"kind":"sticky","id":"s","x":0,"y":0},{"kind":"sticky","id":"t","x":0,"y":0},{"kind":"connector","id":"c","from":"s","to":"t"},{"kind":"label","on":"c","at":2}]),
            "/elements/3/at",
            "label_position_outside_path",
        ),
        (
            json!([{"kind":"sticky","id":"s","x":0,"y":0},{"kind":"group","members":["s"]}]),
            "/elements/1/members",
            "group_needs_two_members",
        ),
        (
            json!([{"kind":"sticky","id":"s","x":0,"y":0},{"kind":"group","members":["s","s"]}]),
            "/elements/1/members",
            "duplicate_group_member",
        ),
        (
            json!([{"kind":"shape","shape":"ellipse","x":0,"y":0,"w":1,"h":1,"rotation":400}]),
            "/elements/0/rotation",
            "rotation_out_of_range",
        ),
    ] {
        let error = encode::encode(&self::scene(elements.clone())?)
            .err()
            .ok_or("accepted")?;
        assert_eq!(
            rule(error),
            (path.to_owned(), expected.to_owned()),
            "{elements}"
        );
    }
    Ok(())
}

fn options(algorithm: super::layout::Algorithm) -> super::layout::Options {
    super::layout::Options {
        algorithm,
        direction: super::layout::Direction::Right,
        gap: 80.0,
        columns: None,
    }
}

fn codes(scene: &Scene) -> Vec<String> {
    super::check::check(scene)
        .findings
        .into_iter()
        .map(|finding| finding.code)
        .collect()
}

#[test]
fn layered_layout_is_deterministic_keeps_lanes_for_long_edges_and_checks_clean() -> TestResult {
    let shape = |id: &str| json!({"kind":"shape","id":id,"shape":"rounded_rectangle","x":0,"y":0,"w":200,"h":96,"text":id});
    let scene = scene(json!([
        shape("start"), shape("check"), shape("pay"), shape("wait"), shape("ship"),
        {"kind":"connector","id":"a","from":"start","to":"check"},
        {"kind":"connector","id":"b","from":"check","to":"pay"},
        {"kind":"label","on":"b","text":"Yes"},
        {"kind":"connector","id":"c","from":"check","to":"wait"},
        {"kind":"label","on":"c","text":"No"},
        {"kind":"connector","from":"wait","to":"pay"},
        {"kind":"connector","from":"pay","to":"ship"},
        {"kind":"connector","from":"ship","to":"start"},
    ]))?;
    let first = super::layout::layout(&scene, &options(super::layout::Algorithm::Layered));
    let second = super::layout::layout(&scene, &options(super::layout::Algorithm::Layered));
    assert_eq!(first, second, "same input, same output");
    let report = super::check::check(&first);
    assert_eq!(report.warning_count, 0, "{:?}", report.findings);
    let position = |id: &str| {
        first.elements.iter().find_map(|element| match element {
            Element::Shape(item) if item.id.as_deref() == Some(id) => Some((item.x, item.y)),
            _ => None,
        })
    };
    let (check, pay, wait) = (
        position("check").ok_or("check")?,
        position("pay").ok_or("pay")?,
        position("wait").ok_or("wait")?,
    );
    assert!(check.0 < wait.0 && wait.0 < pay.0, "layers follow the flow");
    assert_ne!(
        wait.1, pay.1,
        "the long edge check to pay keeps its own lane"
    );
    for element in &first.elements {
        if let Element::Shape(item) = element {
            assert_eq!(
                (item.x % 8.0, item.y % 8.0),
                (0.0, 0.0),
                "snapped to the 8 px grid"
            );
        }
    }
    encode::encode(&first)?;
    Ok(())
}

#[test]
fn grid_layout_fills_sections_and_tree_layout_puts_parents_before_children() -> TestResult {
    let sticky = |id: &str, x: i32| json!({"kind":"sticky","id":id,"x":x,"y":100,"text":id});
    let board = scene(json!([
        {"kind":"section","id":"good","x":0,"y":0,"w":1000,"h":400,"title":"Good"},
        {"kind":"section","id":"bad","x":1100,"y":0,"w":1000,"h":400,"title":"Bad"},
        sticky("g1", 10), sticky("g2", 400), sticky("g3", 700),
        sticky("b1", 1150), sticky("b2", 1500),
    ]))?;
    let laid = super::layout::layout(&board, &options(super::layout::Algorithm::Grid));
    let members = super::geometry::section_members(&laid);
    assert_eq!(members.len(), 5, "every sticky stays inside its section");
    assert!(
        !codes(&laid)
            .iter()
            .any(|code| code == "overlap" || code == "section_overlap")
    );

    let node =
        |id: &str| json!({"kind":"shape","id":id,"shape":"ellipse","x":0,"y":0,"w":160,"h":80});
    let map = scene(json!([
        node("root"), node("a"), node("b"), node("a1"),
        {"kind":"connector","from":"root","to":"a"},
        {"kind":"connector","from":"root","to":"b"},
        {"kind":"connector","from":"a","to":"a1"},
    ]))?;
    let tree = super::layout::layout(&map, &options(super::layout::Algorithm::Tree));
    let x = |id: &str| {
        tree.elements.iter().find_map(|element| match element {
            Element::Shape(item) if item.id.as_deref() == Some(id) => Some(item.x),
            _ => None,
        })
    };
    assert!(x("root") < x("a") && x("a") < x("a1"));
    assert_eq!(super::check::check(&tree).warning_count, 0);
    Ok(())
}

#[test]
fn check_reports_overlap_contrast_shared_anchors_and_overflow() -> TestResult {
    let board = scene(json!([
        {"kind":"shape","id":"a","shape":"rectangle","x":0,"y":0,"w":200,"h":100,"fill":"blue-boldest","text":"A"},
        {"kind":"shape","id":"b","shape":"rectangle","x":100,"y":50,"w":200,"h":100},
        {"kind":"shape","id":"c","shape":"rectangle","x":600,"y":0,"w":80,"h":40,"text":"This label is far too long to fit in such a small box"},
        {"kind":"shape","id":"d","shape":"rectangle","x":600,"y":300,"w":200,"h":100},
        {"kind":"connector","from":"c","to":"d","from_side":"bottom","to_side":"top"},
        {"kind":"connector","from":"c","to":"d","from_side":"bottom","to_side":"top"},
    ]))?;
    let found = codes(&board);
    for expected in ["overlap", "low_contrast", "shared_anchor", "text_overflow"] {
        assert!(
            found.iter().any(|code| code == expected),
            "{expected} in {found:?}"
        );
    }
    let report = super::check::check(&board);
    assert!(report.score < 100 && report.warning_count >= 4);
    Ok(())
}

#[test]
fn preview_draws_every_visible_kind_as_escaped_svg() -> TestResult {
    let board = scene(json!([
        {"kind":"section","x":0,"y":0,"w":600,"h":300,"title":"A & B"},
        {"kind":"shape","id":"a","shape":"diamond","x":40,"y":80,"w":160,"h":160,"text":"<ok?>"},
        {"kind":"sticky","id":"b","x":400,"y":80,"text":"Ship"},
        {"kind":"connector","id":"c","from":"a","to":"b","route":"curved","dash":"dashed"},
        {"kind":"label","on":"c","text":"Yes"},
        {"kind":"stamp","sprite":"rocket","on":"b"},
        {"kind":"line","x1":0,"y1":400,"x2":600,"y2":400},
        {"kind":"text","x":0,"y":440,"w":400,"text":"# Title"},
    ]))?;
    let svg = super::preview::svg(&board);
    assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
    for needle in [
        "A &amp; B",
        "&lt;ok?&gt;",
        "<polygon",
        "stroke-dasharray",
        "Yes",
        "rocket",
        "Title",
    ] {
        assert!(svg.contains(needle), "{needle}");
    }
    assert_eq!(svg, super::preview::svg(&board), "deterministic");
    Ok(())
}
