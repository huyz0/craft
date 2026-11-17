//! Clipboard records to a scene.
//!
//! Every record becomes exactly one scene element in the same order, so index references held
//! by raw records (mind maps, attached comments) stay valid when the scene is encoded again.
//! Records the vocabulary cannot express without loss stay raw. Author account IDs and resolved
//! resource identifiers are removed from every record.

use craft_core::error::{CraftError, ErrorCode};
use serde_json::{Map, Value};

use super::base64;
use super::scene::*;
use super::text;
use super::vocabulary::*;

pub struct Decoded {
    pub scene: Scene,
    pub raw: usize,
    pub private_fields_removed: usize,
    pub warnings: Vec<String>,
}

const PRIVATE_KEYS: &[&str] = &[
    "createdBy",
    "authorIds",
    "lastEditedBy",
    "ownerId",
    "accountId",
];

/// Accepts the copied HTML, the bare base64 attribute value or the decoded JSON array.
pub fn records(source: &[u8]) -> Result<Vec<Value>, CraftError> {
    let invalid = || {
        CraftError::new(
            ErrorCode::InputInvalid,
            "Input is not a whiteboard clipboard payload.",
        )
    };
    let text = std::str::from_utf8(source).map_err(|_| invalid())?.trim();
    let json = if text.starts_with('[') {
        text.to_owned()
    } else {
        let encoded = match text.find("data-canvas-clipboard=\"") {
            Some(start) => {
                let rest = &text[start + "data-canvas-clipboard=\"".len()..];
                &rest[..rest.find('"').ok_or_else(invalid)?]
            }
            None => text,
        };
        String::from_utf8(base64::decode(encoded).ok_or_else(invalid)?).map_err(|_| invalid())?
    };
    match serde_json::from_str(&json).map_err(|_| invalid())? {
        Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_object) => Ok(items),
        _ => Err(invalid()),
    }
}

pub fn decode(records: Vec<Value>) -> Decoded {
    let mut decoded = Decoded {
        scene: Scene {
            format: SceneFormat::V1,
            elements: Vec::with_capacity(records.len()),
        },
        raw: 0,
        private_fields_removed: 0,
        warnings: Vec::new(),
    };
    let count = records.len();
    for (index, mut record) in records.into_iter().enumerate() {
        decoded.private_fields_removed += scrub(&mut record);
        let Value::Object(map) = record else { continue };
        let id = Some(format!("e{}", index + 1));
        let element = typed(&map, id.clone(), count, &mut decoded.warnings).unwrap_or_else(|| {
            decoded.raw += 1;
            Element::Raw(Raw { id, element: map })
        });
        decoded.scene.elements.push(element);
    }
    decoded
}

fn typed(
    map: &Map<String, Value>,
    id: Option<String>,
    count: usize,
    warnings: &mut Vec<String>,
) -> Option<Element> {
    let reference = |key: &str| -> Option<String> {
        let index = usize::try_from(map.get(key)?.as_i64()?).ok()?;
        (index < count).then(|| format!("e{}", index + 1))
    };
    let kind = map.get("type")?.as_str()?;
    Some(match kind {
        "shape" => {
            let (x, y, w, h) = basis_box(map)?;
            let (text, adf) = content(map)?;
            Element::Shape(Shape {
                id,
                shape: ShapeKind::from_code(u16::try_from(map.get("shape")?.as_u64()?).ok()?)?,
                x,
                y,
                w,
                h,
                fill: not_default(color(map.get("color")?)?, Color::GraySubtle),
                filled: not_default(map.get("fillEnabled")?.as_bool()?, true),
                text,
                adf,
                align: not_default(align(map)?, Align::Center),
                valign: not_default(vertical(map)?, VerticalAlign::Middle),
                font_scale: scale(map)?,
                rotation: turn(map)?,
            })
        }
        "sticky" => {
            let (x, y, w, h) = basis_box(map)?;
            let sticky = color(map.get("color")?)?;
            if !sticky.is_sticky() {
                return None;
            }
            let (text, adf) = content(map)?;
            Element::Sticky(Sticky {
                id,
                x,
                y,
                w: not_default(w, STICKY_SIZE),
                h: not_default(h, STICKY_SIZE),
                color: Some(sticky),
                text,
                adf,
                align: not_default(align(map)?, Align::Center),
                valign: not_default(vertical(map)?, VerticalAlign::Middle),
                font_scale: scale(map)?,
                rotation: turn(map)?,
            })
        }
        "text" => {
            let (x, y, basis_w, _) = basis_box(map)?;
            // A flexible-width text grows from its basis box; the rendered width is `size`.
            let flexible = map
                .get("allowFlexibleWidth")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let rendered = vector(map.get("size")?)?.0;
            let w = if flexible && rendered > basis_w {
                rendered
            } else {
                basis_w
            };
            let (text, adf) = content(map)?;
            Element::Text(TextBox {
                id,
                x,
                y,
                w: round(w),
                text,
                adf,
                color: not_default(color(map.get("color")?)?, Color::DefaultText),
                align: not_default(align(map)?, Align::Left),
                font_scale: scale(map)?,
                rotation: turn(map)?,
            })
        }
        "section" => {
            let (cx, cy) = vector(map.get("position")?)?;
            let (w, h) = vector(map.get("size")?)?;
            let title = map.get("title")?.as_str()?.to_owned();
            Element::Section(Section {
                id,
                x: round(cx - w / 2.0),
                y: round(cy - h / 2.0),
                w: round(w),
                h: round(h),
                title: (!title.is_empty()).then_some(title),
                color: not_default(color(map.get("color")?)?, Color::White),
                shadow: not_default(map.get("hasDropShadow")?.as_bool()?, false),
            })
        }
        "connector" | "path" => {
            if map
                .get("segments")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
            {
                warnings.push(format!(
                    "{}: custom_route_points_dropped",
                    id.as_deref().unwrap_or("")
                ));
            }
            let route = Route::from_code(code(map, "presentation")?)?;
            let start_cap = Cap::from_code(code(map, "startCap")?)?;
            let end_cap = Cap::from_code(code(map, "endCap")?)?;
            let stroke = Stroke::from_code(code(map, "stroke")?)?;
            let dash = Dash::from_code(code(map, "strokeStyle")?)?;
            let line_color = color(map.get("color")?)?;
            match (kind, reference("sourceIndex"), reference("targetIndex")) {
                ("connector", Some(from), Some(to)) => Element::Connector(Connector {
                    id,
                    from,
                    to,
                    from_side: side(map.get("sourceAnchor")?)?,
                    to_side: side(map.get("targetAnchor")?)?,
                    route: not_default(route, Route::Elbow),
                    start_cap: not_default(start_cap, Cap::None),
                    end_cap: not_default(end_cap, Cap::Arrow),
                    color: not_default(line_color, Color::GrayBold),
                    stroke: not_default(stroke, Stroke::Thin),
                    dash: not_default(dash, Dash::Solid),
                }),
                _ => {
                    let (x1, y1) = pair(map.get("start")?)?;
                    let (x2, y2) = pair(map.get("end")?)?;
                    Element::Line(Line {
                        id,
                        x1,
                        y1,
                        x2,
                        y2,
                        route: not_default(route, Route::Straight),
                        start_cap: not_default(start_cap, Cap::None),
                        end_cap: not_default(end_cap, Cap::Arrow),
                        color: not_default(line_color, Color::GrayBold),
                        stroke: not_default(stroke, Stroke::Thin),
                        dash: not_default(dash, Dash::Solid),
                    })
                }
            }
        }
        "stamp" => {
            let sprite = Sprite::ALL
                .iter()
                .copied()
                .find(|item| Some(item.code()) == map.get("spriteId").and_then(Value::as_str))?;
            let (size, _) = vector(map.get("size")?)?;
            let on = map.get("attachedTo").and_then(|attached| {
                let index = usize::try_from(attached.get("parentIndex")?.as_u64()?).ok()?;
                (index < count).then(|| format!("e{}", index + 1))
            });
            let (cx, cy) = vector(map.get("position")?)?;
            let free = on.is_none();
            Element::Stamp(Stamp {
                id,
                sprite,
                on,
                x: free.then(|| round(cx - size / 2.0)),
                y: free.then(|| round(cy - size / 2.0)),
                size: not_default(round(size), STAMP_SIZE),
            })
        }
        "smartLink" => {
            let (cx, cy) = vector(map.get("position")?)?;
            let (w, h) = vector(map.get("size")?)?;
            Element::Link(Link {
                id,
                url: map.get("url")?.as_str()?.to_owned(),
                x: round(cx - w / 2.0),
                y: round(cy - h / 2.0),
                w: not_default(round(w), LINK_WIDTH),
                h: not_default(round(h), LINK_HEIGHT),
            })
        }
        _ => return None,
    })
}

/// Removes account identities and resolved resource identifiers anywhere in a record.
fn scrub(value: &mut Value) -> usize {
    match value {
        Value::Object(map) => {
            let mut removed = 0;
            for key in PRIVATE_KEYS {
                removed += usize::from(map.remove(*key).is_some());
            }
            if let Some(ari) = map.get_mut("ari")
                && ari.as_str().is_some_and(|text| !text.is_empty())
            {
                *ari = Value::String(String::new());
                removed += 1;
            }
            removed + map.values_mut().map(scrub).sum::<usize>()
        }
        Value::Array(items) => items.iter_mut().map(scrub).sum(),
        _ => 0,
    }
}

fn not_default<T: PartialEq>(value: T, default: T) -> Option<T> {
    (value != default).then_some(value)
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn vector(value: &Value) -> Option<(f64, f64)> {
    Some((value.get("x")?.as_f64()?, value.get("y")?.as_f64()?))
}

fn pair(value: &Value) -> Option<(f64, f64)> {
    let items = value.as_array()?;
    let [x, y] = items.as_slice() else {
        return None;
    };
    Some((round(x.as_f64()?), round(y.as_f64()?)))
}

fn code(map: &Map<String, Value>, key: &str) -> Option<u8> {
    u8::try_from(map.get(key)?.as_u64()?).ok()
}

fn color(value: &Value) -> Option<Color> {
    let channel = |key: &str| u8::try_from(value.get(key)?.as_u64()?).ok();
    Color::from_code((channel("x")?, channel("y")?, channel("z")?))
}

fn align(map: &Map<String, Value>) -> Option<Align> {
    let text = map.get("alignment")?.as_str()?;
    Align::ALL.iter().copied().find(|item| item.code() == text)
}

fn vertical(map: &Map<String, Value>) -> Option<VerticalAlign> {
    VerticalAlign::from_code(code(map, "verticalAlignment")?)
}

fn scale(map: &Map<String, Value>) -> Option<Option<f64>> {
    Some(not_default(map.get("fontScale")?.as_f64()?, 1.0))
}

fn turn(map: &Map<String, Value>) -> Option<Option<f64>> {
    Some(not_default(
        map.get("rotation").and_then(Value::as_f64).unwrap_or(0.0),
        0.0,
    ))
}

/// Top-left box from the center-based basis fields.
fn basis_box(map: &Map<String, Value>) -> Option<(f64, f64, f64, f64)> {
    let (cx, cy) = vector(map.get("basisPosition")?)?;
    let (w, h) = vector(map.get("basisSize")?)?;
    Some((round(cx - w / 2.0), round(cy - h / 2.0), round(w), round(h)))
}

fn content(map: &Map<String, Value>) -> Option<(Option<String>, Option<Value>)> {
    let document: Value = serde_json::from_str(map.get("text")?.as_str()?).ok()?;
    Some(match text::from_adf(&document) {
        Some(plain) if plain.is_empty() => (None, None),
        Some(plain) => (Some(plain), None),
        None => (None, Some(document)),
    })
}

fn side(value: &Value) -> Option<Option<Side>> {
    let point = (value.get("left")?.as_f64()?, value.get("top")?.as_f64()?);
    let named = [
        SideName::Top,
        SideName::Right,
        SideName::Bottom,
        SideName::Left,
    ]
    .into_iter()
    .find(|name| super::encode::unit(*name) == point);
    Some(Some(match named {
        Some(name) => Side::Named(name),
        None => Side::Point(UnitPoint {
            x: point.0,
            y: point.1,
        }),
    }))
}
