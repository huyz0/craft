//! Scene to clipboard records.
//!
//! Records mirror what the editor itself copies, with every field present: the editor stops
//! rendering the board when a pasted record lacks a field it expects. Shapes, stickies and text
//! are placed by `basisPosition` (their center) and `basisSize`; sections and stamps by
//! `position` (center) and `size`. Connectors bind by array index; the editor assigns element
//! IDs on paste.

use std::collections::HashMap;

use craft_core::error::CraftError;
use serde_json::{Value, json};

use super::scene::*;
use super::text;
use super::vocabulary::*;

const MAX_ELEMENTS: usize = 5000;
const MAX_COORDINATE: f64 = 1_000_000.0;
const MAX_EXTENT: f64 = 100_000.0;
const MAX_TEXT: usize = 20_000;

pub struct Encoded {
    pub records: Vec<Value>,
    pub connectors: usize,
}

pub fn encode(scene: &Scene) -> Result<Encoded, CraftError> {
    if scene.elements.is_empty() || scene.elements.len() > MAX_ELEMENTS {
        return Err(CraftError::field(
            "/elements",
            "between_1_and_5000_elements",
        ));
    }
    let mut ids = HashMap::new();
    for (index, element) in scene.elements.iter().enumerate() {
        if let Some(id) = element.id() {
            if !valid_id(id) {
                return Err(field(index, "id", "invalid_element_id"));
            }
            if ids.insert(id, index).is_some() {
                return Err(field(index, "id", "duplicate_element_id"));
            }
        }
    }
    let target = |index: usize, key: &str, id: &str| -> Result<usize, CraftError> {
        let found = *ids
            .get(id)
            .ok_or_else(|| field(index, key, "unknown_element_reference"))?;
        match scene.elements[found] {
            Element::Connector(_)
            | Element::Line(_)
            | Element::Stamp(_)
            | Element::Label(_)
            | Element::Group(_) => Err(field(index, key, "reference_is_not_attachable")),
            _ => Ok(found),
        }
    };
    for (index, element) in scene.elements.iter().enumerate() {
        if let Some(section) = element.section() {
            let named = ids.get(section).map(|found| &scene.elements[*found]);
            if !matches!(named, Some(Element::Section(_))) {
                return Err(field(index, "section", "unknown_section_reference"));
            }
        }
    }
    let mut records = Vec::with_capacity(scene.elements.len());
    let mut connectors = 0;
    for (index, element) in scene.elements.iter().enumerate() {
        let record = match element {
            Element::Shape(item) => {
                check_box(index, item.x, item.y, item.w, item.h)?;
                let (cx, cy) = center(item.x, item.y, item.w, item.h);
                json!({
                    "type":"shape","source":1,
                    "position":v2(cx,cy),"size":v2(item.w,item.h),
                    "color":v3(item.fill.unwrap_or(Color::GraySubtle)),
                    "strokeStyle":1,
                    "text":document(index, item.text.as_deref(), item.adf.as_ref())?,
                    "shape":item.shape.code(),
                    "fillEnabled":item.filled.unwrap_or(true),
                    "fontScale":font_scale(index, item.font_scale)?,
                    "basisSize":v2(item.w,item.h),"basisPosition":v2(cx,cy),
                    "alignment":item.align.unwrap_or(Align::Center).code(),
                    "verticalAlignment":item.valign.unwrap_or(VerticalAlign::Middle).code(),
                    "rotation":rotation(index, item.rotation)?,
                })
            }
            Element::Sticky(item) => {
                let (w, h) = (item.w.unwrap_or(STICKY_SIZE), item.h.unwrap_or(STICKY_SIZE));
                check_box(index, item.x, item.y, w, h)?;
                let color = item.color.unwrap_or(Color::YellowSubtle);
                if !color.is_sticky() {
                    return Err(field(index, "color", "not_a_sticky_color"));
                }
                let (cx, cy) = center(item.x, item.y, w, h);
                json!({
                    "type":"sticky","source":1,
                    "position":v2(cx,cy),"size":v2(w,h),"color":v3(color),
                    "text":document(index, item.text.as_deref(), item.adf.as_ref())?,
                    "fontScale":font_scale(index, item.font_scale)?,
                    "basisSize":v2(w,h),"basisPosition":v2(cx,cy),
                    "alignment":item.align.unwrap_or(Align::Center).code(),
                    "verticalAlignment":item.valign.unwrap_or(VerticalAlign::Middle).code(),
                    "rotation":rotation(index, item.rotation)?,
                    "isAuthorVisible":false,
                })
            }
            Element::Text(item) => {
                check_box(index, item.x, item.y, item.w, TEXT_LINE_HEIGHT)?;
                let (cx, cy) = center(item.x, item.y, item.w, TEXT_LINE_HEIGHT);
                json!({
                    "type":"text","source":1,
                    "position":v2(cx,cy),"size":v2(item.w,TEXT_LINE_HEIGHT),
                    "text":document(index, item.text.as_deref(), item.adf.as_ref())?,
                    "allowFlexibleWidth":false,
                    "color":v3(item.color.unwrap_or(Color::DefaultText)),
                    "fontScale":font_scale(index, item.font_scale)?,
                    "basisSize":v2(item.w,TEXT_LINE_HEIGHT),"basisPosition":v2(cx,cy),
                    "alignment":item.align.unwrap_or(Align::Left).code(),
                    "rotation":rotation(index, item.rotation)?,
                })
            }
            Element::Section(item) => {
                check_box(index, item.x, item.y, item.w, item.h)?;
                let title = item.title.clone().unwrap_or_default();
                if title.chars().count() > 255 {
                    return Err(field(index, "title", "title_too_long"));
                }
                let (cx, cy) = center(item.x, item.y, item.w, item.h);
                json!({
                    "type":"section","source":1,
                    "position":v2(cx,cy),"size":v2(item.w,item.h),
                    "color":v3(item.color.unwrap_or(Color::White)),
                    "title":title,"titleWidth":142.75,
                    "hasDropShadow":item.shadow.unwrap_or(false),"rotation":0,
                })
            }
            Element::Connector(item) => {
                connectors += 1;
                let source = target(index, "from", &item.from)?;
                let destination = target(index, "to", &item.to)?;
                if source == destination {
                    return Err(field(index, "to", "connector_must_join_two_elements"));
                }
                let from = bounds(&scene.elements[source]);
                let to = bounds(&scene.elements[destination]);
                let (auto_from, auto_to) = facing(from, to);
                json!({
                    "type":"connector","source":1,
                    "presentation":item.route.unwrap_or(Route::Elbow).code(),
                    "segments":[],"start":[0,0],"end":[0,0],
                    "position":v2(0.0,0.0),"size":v2(0.0,0.0),
                    "sourceAnchor":anchor(index, "from_side", item.from_side.as_ref(), auto_from)?,
                    "targetAnchor":anchor(index, "to_side", item.to_side.as_ref(), auto_to)?,
                    "startCap":item.start_cap.unwrap_or(Cap::None).code(),
                    "endCap":item.end_cap.unwrap_or(Cap::Arrow).code(),
                    "color":v3(item.color.unwrap_or(Color::GrayBold)),
                    "stroke":item.stroke.unwrap_or(Stroke::Thin).code(),
                    "strokeStyle":item.dash.unwrap_or(Dash::Solid).code(),
                    "sourceIndex":source,"targetIndex":destination,
                })
            }
            Element::Line(item) => {
                for (key, value) in [
                    ("x1", item.x1),
                    ("y1", item.y1),
                    ("x2", item.x2),
                    ("y2", item.y2),
                ] {
                    coordinate(index, key, value)?;
                }
                json!({
                    "type":"path","source":1,"segments":[],
                    "start":[item.x1,item.y1],"end":[item.x2,item.y2],
                    "position":v2(0.0,0.0),"size":v2(0.0,0.0),
                    "presentation":item.route.unwrap_or(Route::Straight).code(),
                    "startCap":item.start_cap.unwrap_or(Cap::None).code(),
                    "endCap":item.end_cap.unwrap_or(Cap::Arrow).code(),
                    "color":v3(item.color.unwrap_or(Color::GrayBold)),
                    "stroke":item.stroke.unwrap_or(Stroke::Thin).code(),
                    "strokeStyle":item.dash.unwrap_or(Dash::Solid).code(),
                })
            }
            Element::Stamp(item) => {
                let size = item.size.unwrap_or(STAMP_SIZE);
                extent(index, "size", size)?;
                match (&item.on, item.x, item.y) {
                    (Some(parent), None, None) => {
                        let parent_index = target(index, "on", parent)?;
                        if !matches!(
                            scene.elements[parent_index],
                            Element::Shape(_) | Element::Sticky(_)
                        ) {
                            return Err(field(index, "on", "stamps_attach_to_shapes_or_stickies"));
                        }
                        let (x, y, w, _) = bounds(&scene.elements[parent_index]);
                        // The offset is the stamp's center from the parent's top-left corner. The
                        // top-right corner keeps the parent's text readable.
                        let (ox, oy) = stamp_offset(w, size);
                        json!({
                            "type":"stamp","source":1,"position":v2(x + ox,y + oy),"size":v2(size,size),
                            "rotation":0,"spriteId":item.sprite.code(),
                            "attachedTo":{"parentIndex":parent_index,"offset":[ox, oy]},
                        })
                    }
                    (None, Some(x), Some(y)) => {
                        coordinate(index, "x", x)?;
                        coordinate(index, "y", y)?;
                        let (cx, cy) = center(x, y, size, size);
                        json!({
                            "type":"stamp","source":1,"position":v2(cx,cy),"size":v2(size,size),
                            "rotation":0,"spriteId":item.sprite.code(),
                        })
                    }
                    _ => return Err(field(index, "on", "stamp_needs_on_or_x_and_y")),
                }
            }
            Element::Link(item) => {
                let (w, h) = (item.w.unwrap_or(LINK_WIDTH), item.h.unwrap_or(LINK_HEIGHT));
                check_box(index, item.x, item.y, w, h)?;
                let scheme_ok = item.url.starts_with("https://") || item.url.starts_with("http://");
                if !scheme_ok || item.url.len() > 2048 || item.url.chars().any(char::is_whitespace)
                {
                    return Err(field(index, "url", "invalid_link_url"));
                }
                let (cx, cy) = center(item.x, item.y, w, h);
                json!({
                    "type":"smartLink","ari":"","source":1,
                    "position":v2(cx,cy),"size":v2(w,h),"url":item.url,
                    "kind":"block","supportedViews":["block"],"supportsPreview":true,
                    "unfurledConnections":[],"unfurlState":"idle","rotation":0,
                })
            }
            Element::Label(item) => {
                let connector = *ids
                    .get(item.on.as_str())
                    .ok_or_else(|| field(index, "on", "unknown_element_reference"))?;
                if !matches!(
                    scene.elements[connector],
                    Element::Connector(_) | Element::Line(_)
                ) {
                    return Err(field(index, "on", "labels_attach_to_connectors_or_lines"));
                }
                let at = item.at.unwrap_or(0.5);
                if !at.is_finite() || !(0.0..=1.0).contains(&at) {
                    return Err(field(index, "at", "label_position_outside_path"));
                }
                json!({
                    "type":"pathLabel","source":1,"sourcePathIndex":connector,"proportion":at,
                    "position":v2(0.0,0.0),"size":v2(0.0,0.0),
                    "color":v3(item.color.unwrap_or(Color::DefaultText)),
                    "text":document(index, item.text.as_deref(), item.adf.as_ref())?,
                    "fontScale":font_scale(index, item.font_scale)?,
                })
            }
            Element::Group(item) => {
                if item.members.len() < 2 {
                    return Err(field(index, "members", "group_needs_two_members"));
                }
                let mut members = Vec::with_capacity(item.members.len());
                let (mut left, mut top, mut right, mut bottom) =
                    (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                for id in &item.members {
                    let member = target(index, "members", id)?;
                    if members.contains(&member) {
                        return Err(field(index, "members", "duplicate_group_member"));
                    }
                    let (x, y, w, h) = scene.elements[member]
                        .bounds()
                        .ok_or_else(|| field(index, "members", "group_member_has_no_box"))?;
                    (left, top) = (left.min(x), top.min(y));
                    (right, bottom) = (right.max(x + w), bottom.max(y + h));
                    members.push(member);
                }
                // The editor maps `childrenIndices` to new IDs on paste; `children` only needs
                // one placeholder per member, and the group needs its own box.
                let placeholders: Vec<String> =
                    (0..members.len()).map(|n| format!("member-{n}")).collect();
                let (cx, cy) = center(left, top, right - left, bottom - top);
                json!({
                    "type":"group","source":1,
                    "position":v2(cx,cy),"size":v2(right - left,bottom - top),
                    "children":placeholders,"childrenIndices":members,
                })
            }
            Element::Raw(item) => {
                if !item.element.get("type").is_some_and(Value::is_string) {
                    return Err(field(index, "element", "raw_element_needs_type"));
                }
                Value::Object(item.element.clone())
            }
        };
        records.push(record);
    }
    Ok(Encoded {
        records,
        connectors,
    })
}

/// Center of an attached stamp relative to its parent's top-left corner.
pub fn stamp_offset(parent_width: f64, size: f64) -> (f64, f64) {
    (parent_width - size * 0.2, size * 0.2)
}

fn field(index: usize, key: &str, rule: &str) -> CraftError {
    CraftError::field(format!("/elements/{index}/{key}"), rule)
}

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn coordinate(index: usize, key: &str, value: f64) -> Result<(), CraftError> {
    if value.is_finite() && value.abs() <= MAX_COORDINATE {
        Ok(())
    } else {
        Err(field(index, key, "coordinate_out_of_range"))
    }
}

fn extent(index: usize, key: &str, value: f64) -> Result<(), CraftError> {
    if value.is_finite() && value > 0.0 && value <= MAX_EXTENT {
        Ok(())
    } else {
        Err(field(index, key, "size_out_of_range"))
    }
}

fn check_box(index: usize, x: f64, y: f64, w: f64, h: f64) -> Result<(), CraftError> {
    coordinate(index, "x", x)?;
    coordinate(index, "y", y)?;
    extent(index, "w", w)?;
    extent(index, "h", h)
}

fn font_scale(index: usize, value: Option<f64>) -> Result<f64, CraftError> {
    let value = value.unwrap_or(1.0);
    if value.is_finite() && (0.25..=10.0).contains(&value) {
        Ok(value)
    } else {
        Err(field(index, "font_scale", "font_scale_out_of_range"))
    }
}

/// Scene rotation is in degrees, clockwise; the editor stores radians.
fn rotation(index: usize, value: Option<f64>) -> Result<f64, CraftError> {
    let value = value.unwrap_or(0.0);
    if value.is_finite() && value.abs() <= 360.0 {
        Ok(value.to_radians())
    } else {
        Err(field(index, "rotation", "rotation_out_of_range"))
    }
}

/// The editor stores text as a serialized ADF document.
fn document(index: usize, plain: Option<&str>, adf: Option<&Value>) -> Result<String, CraftError> {
    let document = match (plain, adf) {
        (Some(_), Some(_)) => return Err(field(index, "adf", "text_and_adf_are_exclusive")),
        (Some(plain), None) if plain.chars().count() > MAX_TEXT => {
            return Err(field(index, "text", "text_too_long"));
        }
        (Some(plain), None) => text::to_adf(plain),
        (None, Some(adf)) if text::valid_document(adf) => adf.clone(),
        (None, Some(_)) => return Err(field(index, "adf", "invalid_adf_document")),
        (None, None) => text::to_adf(""),
    };
    serde_json::to_string(&document).map_err(|_| field(index, "text", "unencodable_text"))
}

fn center(x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    (x + w / 2.0, y + h / 2.0)
}

fn bounds(element: &Element) -> (f64, f64, f64, f64) {
    element.bounds().unwrap_or((0.0, 0.0, 0.0, 0.0))
}

/// Facing sides chosen along the axis with the larger center offset.
fn facing(from: (f64, f64, f64, f64), to: (f64, f64, f64, f64)) -> (SideName, SideName) {
    let (fx, fy) = center(from.0, from.1, from.2, from.3);
    let (tx, ty) = center(to.0, to.1, to.2, to.3);
    let (dx, dy) = (tx - fx, ty - fy);
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            (SideName::Right, SideName::Left)
        } else {
            (SideName::Left, SideName::Right)
        }
    } else if dy >= 0.0 {
        (SideName::Bottom, SideName::Top)
    } else {
        (SideName::Top, SideName::Bottom)
    }
}

fn anchor(
    index: usize,
    key: &str,
    side: Option<&Side>,
    auto: SideName,
) -> Result<Value, CraftError> {
    let (left, top) = match side {
        None | Some(Side::Named(SideName::Auto)) => unit(auto),
        Some(Side::Named(name)) => unit(*name),
        Some(Side::Point(point)) => {
            let in_box = |value: f64| value.is_finite() && (0.0..=1.0).contains(&value);
            if !in_box(point.x) || !in_box(point.y) {
                return Err(field(index, key, "anchor_outside_unit_box"));
            }
            (point.x, point.y)
        }
    };
    Ok(json!({"left":left,"top":top}))
}

pub fn unit(side: SideName) -> (f64, f64) {
    match side {
        SideName::Top => (0.5, 0.0),
        SideName::Right => (1.0, 0.5),
        SideName::Bottom => (0.5, 1.0),
        SideName::Left | SideName::Auto => (0.0, 0.5),
    }
}

fn v2(x: f64, y: f64) -> Value {
    json!({"x":x,"y":y,"type":"Vector2"})
}

fn v3(color: Color) -> Value {
    let (r, g, b) = color.code();
    json!({"x":r,"y":g,"z":b,"type":"Vector3"})
}
