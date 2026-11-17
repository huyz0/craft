//! The authored scene: top-left boxes in canvas pixels, named palette colors and id references.
//!
//! Coordinates are relative: the editor places a pasted group at the pointer, so only the
//! positions of elements relative to each other matter. The y axis points down.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::vocabulary::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum SceneFormat {
    #[serde(rename = "craft.whiteboard.scene.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub format: SceneFormat,
    /// Paint order: later elements are drawn above earlier ones.
    pub elements: Vec<Element>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Element {
    Shape(Shape),
    Sticky(Sticky),
    Text(TextBox),
    Section(Section),
    Connector(Connector),
    Line(Line),
    Stamp(Stamp),
    Link(Link),
    Label(Label),
    Group(Group),
    /// An editor element the scene vocabulary does not model, kept verbatim.
    Raw(Raw),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub shape: ShapeKind,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Default `gray-subtle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Color>,
    /// `false` draws the outline only. Default `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adf: Option<Value>,
    /// Default `center`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    /// Default `middle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valign: Option<VerticalAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f64>,
    /// Degrees clockwise, at most 360 either way. Default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sticky {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub x: f64,
    pub y: f64,
    /// Default 144. The editor grows a sticky downward to fit its text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    /// One of the eight sticky colors. Default `yellow-subtle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adf: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valign: Option<VerticalAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextBox {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub x: f64,
    pub y: f64,
    /// Wrapping width. Height follows the text.
    pub w: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adf: Option<Value>,
    /// Default `default-text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    /// Default `left`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Section {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Default `white`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<bool>,
}

/// Where a connector meets an element: a named side or a point in the element's unit box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Side {
    Named(SideName),
    Point(UnitPoint),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideName {
    /// Facing sides, chosen from the two elements' centers.
    Auto,
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnitPoint {
    /// 0 is the left edge, 1 the right edge.
    pub x: f64,
    /// 0 is the top edge, 1 the bottom edge.
    pub y: f64,
}

/// A line bound to two elements; it follows them when they move.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Connector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_cap: Option<Cap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_cap: Option<Cap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<Dash>,
}

/// A free line between two canvas points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Line {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_cap: Option<Cap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_cap: Option<Cap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<Dash>,
}

/// A stamp either free at (`x`, `y`) or pinned to the element named by `on`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub sprite: Sprite,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// Square side. Default 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
}

/// A Smart Link card. The editor resolves the URL for the viewer when the board renders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub url: String,
    pub x: f64,
    pub y: f64,
    /// Default 280 by 306.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
}

/// Text carried by a connector or line, drawn on the path and moving with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Label {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The connector or line id.
    pub on: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adf: Option<Value>,
    /// Position along the path, 0 at the start and 1 at the end. Default 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f64>,
}

/// Elements selected, moved and resized together (the editor's Group elements, Ctrl+G).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Group {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Raw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The editor record, with author identities and resolved resource identifiers removed.
    pub element: Map<String, Value>,
}

impl Element {
    pub fn id(&self) -> Option<&str> {
        match self {
            Element::Shape(item) => item.id.as_deref(),
            Element::Sticky(item) => item.id.as_deref(),
            Element::Text(item) => item.id.as_deref(),
            Element::Section(item) => item.id.as_deref(),
            Element::Connector(item) => item.id.as_deref(),
            Element::Line(item) => item.id.as_deref(),
            Element::Stamp(item) => item.id.as_deref(),
            Element::Link(item) => item.id.as_deref(),
            Element::Label(item) => item.id.as_deref(),
            Element::Group(item) => item.id.as_deref(),
            Element::Raw(item) => item.id.as_deref(),
        }
    }

    /// Top-left box of elements a connector or stamp can attach to.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        match self {
            Element::Shape(item) => Some((item.x, item.y, item.w, item.h)),
            Element::Sticky(item) => Some((
                item.x,
                item.y,
                item.w.unwrap_or(STICKY_SIZE),
                item.h.unwrap_or(STICKY_SIZE),
            )),
            Element::Text(item) => Some((item.x, item.y, item.w, TEXT_LINE_HEIGHT)),
            Element::Section(item) => Some((item.x, item.y, item.w, item.h)),
            Element::Link(item) => Some((
                item.x,
                item.y,
                item.w.unwrap_or(LINK_WIDTH),
                item.h.unwrap_or(LINK_HEIGHT),
            )),
            _ => None,
        }
    }
}

pub const STICKY_SIZE: f64 = 144.0;
pub const STAMP_SIZE: f64 = 60.0;
pub const TEXT_LINE_HEIGHT: f64 = 38.0;
pub const LINK_WIDTH: f64 = 280.0;
pub const LINK_HEIGHT: f64 = 306.0;
