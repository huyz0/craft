//! Local conversion between an authored whiteboard scene and the whiteboard editor's clipboard
//! payload. No profile, credential or cloud service is consulted.
//!
//! Confluence exposes no API for whiteboard drawing content. The editor's copy and paste carry
//! elements as `<div id="canvas-clipboard" data-canvas-clipboard="BASE64(JSON array)">` HTML, so
//! a scene becomes board content when that HTML is pasted into an open whiteboard, and board
//! content becomes a scene when copied HTML is decoded.

mod base64;
pub mod check;
mod decode;
mod encode;
mod geometry;
pub mod layout;
mod preview;
pub mod scene;
mod text;
pub mod vocabulary;

#[cfg(test)]
mod tests;

use clap::{Args, Subcommand};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::canonical_digest;
use craft_core::projection::CompiledProjection;
use craft_core::registry::Registry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::dispatch::DispatchOutput;
use scene::Scene;

pub const ENCODE: &str = "whiteboard.encode";
pub const DECODE: &str = "whiteboard.decode";
pub const LAYOUT: &str = "whiteboard.layout";
pub const CHECK: &str = "whiteboard.check";
pub const PREVIEW: &str = "whiteboard.preview";

#[derive(Subcommand)]
pub enum WhiteboardAction {
    Encode(WhiteboardFileArgs),
    Decode(WhiteboardFileArgs),
    Layout(WhiteboardLayoutArgs),
    Check(WhiteboardFileArgs),
    Preview(WhiteboardFileArgs),
}

impl WhiteboardAction {
    pub fn command(&self) -> &'static str {
        match self {
            WhiteboardAction::Encode(_) => ENCODE,
            WhiteboardAction::Decode(_) => DECODE,
            WhiteboardAction::Layout(_) => LAYOUT,
            WhiteboardAction::Check(_) => CHECK,
            WhiteboardAction::Preview(_) => PREVIEW,
        }
    }

    pub fn file(&self) -> &str {
        match self {
            WhiteboardAction::Encode(args)
            | WhiteboardAction::Decode(args)
            | WhiteboardAction::Check(args)
            | WhiteboardAction::Preview(args) => &args.file,
            WhiteboardAction::Layout(args) => &args.file,
        }
    }
}

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WhiteboardLayoutArgs {
    #[arg(long, default_value = "", help = "Scene JSON, or - for stdin")]
    #[schemars(length(min = 1, max = 2048))]
    pub file: String,
    /// `layered` for flows and dependencies, `tree` for mind maps and hierarchies, `grid` for
    /// sticky boards (per section).
    #[arg(long, value_enum, default_value = "layered")]
    #[serde(default)]
    pub algorithm: layout::Algorithm,
    #[arg(long, value_enum, default_value = "right")]
    #[serde(default)]
    pub direction: layout::Direction,
    /// Gap between elements in pixels; a multiple of 8 keeps the 8 px grid.
    #[arg(long, default_value_t = 80)]
    #[serde(default = "default_gap")]
    #[schemars(range(min = 8, max = 400))]
    pub gap: u16,
    /// Grid columns (default 2 inside sections, otherwise the square root of the count).
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 50))]
    pub columns: Option<u16>,
}

fn default_gap() -> u16 {
    80
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewData {
    pub element_count: usize,
    /// Approximate rendering for review; the board's fonts, icons and routing differ.
    pub svg: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutData {
    pub moved_count: usize,
    /// Checks run on the laid-out scene.
    pub report: check::Report,
    pub scene: Scene,
}

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WhiteboardFileArgs {
    #[arg(
        long,
        default_value = "",
        help = "Scene JSON (encode) or copied clipboard HTML (decode), or - for stdin"
    )]
    #[schemars(length(min = 1, max = 2048))]
    pub file: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EncodeData {
    pub element_count: usize,
    pub connector_count: usize,
    pub scene_digest: String,
    /// Paste this as `text/html` into an open whiteboard.
    pub clipboard_html: String,
    pub board_written: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecodeData {
    pub element_count: usize,
    pub raw_count: usize,
    pub private_fields_removed: usize,
    pub warnings: Vec<String>,
    pub scene: Scene,
}

pub fn execute(
    action: WhiteboardAction,
    projection: Option<CompiledProjection>,
    control: &InvocationControl,
) -> Result<DispatchOutput, CraftError> {
    control.check()?;
    let source = crate::dispatch::read_source(action.file(), control)?;
    let data = match action {
        WhiteboardAction::Encode(_) => encode_data(&source)?,
        WhiteboardAction::Check(_) => to_value(check::check(&parse_scene(&source)?.1))?,
        WhiteboardAction::Preview(_) => {
            let (_, scene) = parse_scene(&source)?;
            encode::encode(&scene)?;
            to_value(PreviewData {
                element_count: scene.elements.len(),
                svg: preview::svg(&scene),
            })?
        }
        WhiteboardAction::Layout(args) => {
            let (_, scene) = parse_scene(&source)?;
            encode::encode(&scene)?;
            if !(8..=400).contains(&args.gap)
                || args.columns.is_some_and(|c| !(1..=50).contains(&c))
            {
                return Err(CraftError::field("/gap", "layout_option_out_of_range"));
            }
            let laid = layout::layout(
                &scene,
                &layout::Options {
                    algorithm: args.algorithm,
                    direction: args.direction,
                    gap: f64::from(args.gap),
                    columns: args.columns.map(usize::from),
                },
            );
            let moved_count = scene
                .elements
                .iter()
                .zip(&laid.elements)
                .filter(|(before, after)| before != after)
                .count();
            to_value(LayoutData {
                moved_count,
                report: check::check(&laid),
                scene: laid,
            })?
        }
        WhiteboardAction::Decode(_) => {
            let decoded = decode::decode(decode::records(&source)?);
            serde_json::to_value(DecodeData {
                element_count: decoded.scene.elements.len(),
                raw_count: decoded.raw,
                private_fields_removed: decoded.private_fields_removed,
                warnings: decoded.warnings,
                scene: decoded.scene,
            })
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode scene."))?
        }
    };
    control.check()?;
    let data = match projection {
        Some(projection) => projection.data(&data)?,
        None => data,
    };
    Ok(DispatchOutput { data, page: None })
}

fn to_value<T: Serialize>(data: T) -> Result<serde_json::Value, CraftError> {
    serde_json::to_value(data)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode whiteboard data."))
}

fn parse_scene(source: &[u8]) -> Result<(serde_json::Value, Scene), CraftError> {
    let value = craft_core::input::parse_object(source)?;
    // Serde names the offending variant or field, which is what an author needs to fix it.
    let scene: Scene = serde_json::from_value(value.clone()).map_err(|error| {
        CraftError::new(
            ErrorCode::InputInvalid,
            format!("Invalid whiteboard scene: {error}."),
        )
    })?;
    Ok((value, scene))
}

fn encode_data(source: &[u8]) -> Result<serde_json::Value, CraftError> {
    let (value, scene) = parse_scene(source)?;
    let encoded = encode::encode(&scene)?;
    serde_json::to_value(EncodeData {
        element_count: encoded.records.len(),
        connector_count: encoded.connectors,
        scene_digest: canonical_digest(&value)?,
        clipboard_html: clipboard_html(&encoded.records)?,
        board_written: false,
    })
    .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode clipboard data."))
}

pub fn clipboard_html(records: &[serde_json::Value]) -> Result<String, CraftError> {
    let json = serde_json::to_vec(records)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode clipboard records."))?;
    Ok(format!(
        "<div id=\"canvas-clipboard\" data-canvas-clipboard=\"{}\"></div>",
        base64::encode(&json)
    ))
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let example: Scene = serde_json::from_value(json!({
        "format": "craft.whiteboard.scene.v1",
        "elements": [
            {"kind":"shape","id":"start","shape":"start_end","x":0,"y":0,"w":180,"h":80,"fill":"green-subtle","text":"Start"},
            {"kind":"sticky","id":"idea","x":300,"y":-32,"text":"Idea"},
            {"kind":"connector","from":"start","to":"idea"}
        ]
    }))
    .map_err(|_| CraftError::new(ErrorCode::Internal, "Invalid whiteboard example."))?;
    let encoded = encode::encode(&example)?;
    crate::catalog::register::<WhiteboardFileArgs, EncodeData>(
        registry,
        ENCODE,
        "Encode a whiteboard scene file as editor clipboard HTML; no network or board write.",
        json!({"file":"scene.json"}),
        serde_json::to_value(EncodeData {
            element_count: encoded.records.len(),
            connector_count: encoded.connectors,
            scene_digest: "sha256:example".into(),
            clipboard_html: clipboard_html(&encoded.records)?,
            board_written: false,
        })
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode example."))?,
    )?;
    let decoded = decode::decode(encoded.records);
    crate::catalog::register::<WhiteboardFileArgs, DecodeData>(
        registry,
        DECODE,
        "Decode copied whiteboard clipboard HTML into a scene; no network.",
        json!({"file":"clipboard.html"}),
        serde_json::to_value(DecodeData {
            element_count: decoded.scene.elements.len(),
            raw_count: decoded.raw,
            private_fields_removed: decoded.private_fields_removed,
            warnings: decoded.warnings,
            scene: decoded.scene,
        })
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode example."))?,
    )?;
    let laid = layout::layout(
        &example,
        &layout::Options {
            algorithm: layout::Algorithm::Layered,
            direction: layout::Direction::Right,
            gap: 80.0,
            columns: None,
        },
    );
    crate::catalog::register::<WhiteboardLayoutArgs, LayoutData>(
        registry,
        LAYOUT,
        "Lay out a whiteboard scene file deterministically (layered, tree or grid); no network.",
        json!({"file":"scene.json","algorithm":"layered"}),
        to_value(LayoutData {
            moved_count: 2,
            report: check::check(&laid),
            scene: laid,
        })?,
    )?;
    crate::catalog::register::<WhiteboardFileArgs, check::Report>(
        registry,
        CHECK,
        "Check a whiteboard scene file for overlaps, crossings, spacing, contrast and fit.",
        json!({"file":"scene.json"}),
        to_value(check::check(&example))?,
    )?;
    crate::catalog::register::<WhiteboardFileArgs, PreviewData>(
        registry,
        PREVIEW,
        "Render a whiteboard scene file as an approximate SVG for review; no network.",
        json!({"file":"scene.json"}),
        to_value(PreviewData {
            element_count: example.elements.len(),
            svg: preview::svg(&example),
        })?,
    )
}
