//! Deterministic layout and readability checks for a scene. Rules follow the Atlassian Design
//! System where it applies: an 8 px spacing grid, WCAG AA text contrast (4.5:1) and semantic use
//! of a small number of accent colors.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::geometry::*;
use super::scene::*;
use super::text;
use super::vocabulary::*;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    /// Scene element ids, or `#index` for elements without an id.
    pub elements: Vec<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub warning_count: usize,
    pub info_count: usize,
    /// 100 minus 10 per warning and 2 per note, floored at 0.
    pub score: u32,
    pub findings: Vec<Finding>,
}

const GRID: f64 = 8.0;
/// Approximate advance of the board's body font (Atlassian Sans, 14 px) per character.
const GLYPH: f64 = 7.4;
const LINE: f64 = 20.0;
const PADDING: f64 = 16.0;

pub fn check(scene: &Scene) -> Report {
    let mut findings = Vec::new();
    let name = |index: usize| -> String {
        scene.elements[index]
            .id()
            .map_or_else(|| format!("#{index}"), str::to_owned)
    };
    let mut push = |code: &str, severity, elements: Vec<String>, message: String| {
        findings.push(Finding {
            code: code.into(),
            severity,
            elements,
            message,
        })
    };
    let boxes: Vec<(usize, Rect)> = scene
        .elements
        .iter()
        .enumerate()
        .filter(|(_, element)| is_node(element) || matches!(element, Element::Text(_)))
        .filter_map(|(index, element)| rect(element).map(|bounds| (index, bounds)))
        .collect();
    let sections: Vec<(usize, Rect)> = scene
        .elements
        .iter()
        .enumerate()
        .filter(|(_, element)| matches!(element, Element::Section(_)))
        .filter_map(|(index, element)| rect(element).map(|bounds| (index, bounds)))
        .collect();
    let members = section_members(scene);

    for (a, (i, first)) in boxes.iter().enumerate() {
        for (j, second) in boxes.iter().skip(a + 1) {
            if first.overlap(second) > 0.0 {
                push(
                    "overlap",
                    Severity::Warning,
                    vec![name(*i), name(*j)],
                    "Elements overlap; move them apart or run whiteboard layout.".into(),
                );
            }
        }
        for (s, area) in &sections {
            if area.overlap(first) > 0.0 && !area.contains(first) {
                push("section_boundary", Severity::Warning, vec![name(*i), name(*s)],
                    "Element straddles a section edge, so it does not belong to the section; move it fully inside or outside.".into());
            }
        }
    }
    for (a, (i, first)) in sections.iter().enumerate() {
        for (j, second) in sections.iter().skip(a + 1) {
            if first.overlap(second) > 0.0 {
                push(
                    "section_overlap",
                    Severity::Warning,
                    vec![name(*i), name(*j)],
                    "Sections overlap; sections cannot nest, so keep them side by side.".into(),
                );
            }
        }
        if !members.values().any(|section| section == i) {
            push(
                "empty_section",
                Severity::Info,
                vec![name(*i)],
                "Section has no elements inside it.".into(),
            );
        }
    }

    let segments = segments(scene);
    for (a, first) in segments.iter().enumerate() {
        for second in segments.iter().skip(a + 1) {
            let shared = [first.from, first.to]
                .iter()
                .any(|end| [second.from, second.to].contains(end));
            if !shared && crosses((first.start, first.end), (second.start, second.end)) {
                push(
                    "crossing",
                    Severity::Info,
                    vec![name(first.index), name(second.index)],
                    "Connectors cross; reorder elements, change sides, or run whiteboard layout."
                        .into(),
                );
            }
        }
        for (i, area) in &boxes {
            if *i != first.from && *i != first.to && passes_through((first.start, first.end), area)
            {
                push("through_element", Severity::Warning, vec![name(first.index), name(*i)],
                    "Connector runs through an element that is not one of its ends; move the element or route around it.".into());
            }
        }
    }
    let mut anchors: BTreeMap<(usize, i64, i64), Vec<usize>> = BTreeMap::new();
    for segment in &segments {
        for (element, point) in [
            (segment.from, segment.from_anchor),
            (segment.to, segment.to_anchor),
        ] {
            let key = (
                element,
                (point.0 * 1000.0).round() as i64,
                (point.1 * 1000.0).round() as i64,
            );
            anchors.entry(key).or_default().push(segment.index);
        }
    }
    for ((element, _, _), connectors) in &anchors {
        if connectors.len() > 1 {
            let mut ids: Vec<String> = connectors.iter().map(|index| name(*index)).collect();
            ids.insert(0, name(*element));
            push("shared_anchor", Severity::Warning, ids,
                "Several connectors meet this element at the same point and draw on top of each other; spread them with unit points such as {\"x\":0.25,\"y\":1} and {\"x\":0.75,\"y\":1}.".into());
        }
    }

    let labelled: BTreeSet<&str> = scene
        .elements
        .iter()
        .filter_map(|element| match element {
            Element::Label(label) => Some(label.on.as_str()),
            _ => None,
        })
        .collect();
    for segment in &segments {
        let midpoint = (
            (segment.start.0 + segment.end.0) / 2.0,
            (segment.start.1 + segment.end.1) / 2.0,
        );
        for (i, area) in &boxes {
            if matches!(scene.elements[*i], Element::Text(_)) {
                let (cx, cy) = area.center();
                if (cx - midpoint.0).abs() < 60.0 && (cy - midpoint.1).abs() < 40.0 {
                    push(
                        "floating_label",
                        Severity::Info,
                        vec![name(*i), name(segment.index)],
                        "Text sits on a connector; use a label element so it moves with the line."
                            .into(),
                    );
                }
            }
        }
        if let Element::Shape(shape) = &scene.elements[segment.from]
            && matches!(shape.shape, ShapeKind::Diamond | ShapeKind::Decision)
            && scene.elements[segment.index]
                .id()
                .is_none_or(|id| !labelled.contains(id))
        {
            push(
                "unlabelled_branch",
                Severity::Info,
                vec![name(segment.index)],
                "A decision exit has no label; add one such as Yes or No.".into(),
            );
        }
    }

    let mut fills = BTreeSet::new();
    for (index, element) in scene.elements.iter().enumerate() {
        let (fill, content, scale, inner) = match element {
            Element::Shape(item) => (
                item.filled
                    .unwrap_or(true)
                    .then_some(item.fill.unwrap_or(Color::GraySubtle)),
                (item.text.as_deref(), item.adf.as_ref()),
                item.font_scale,
                Some((item.w, item.h)),
            ),
            Element::Sticky(item) => (
                Some(item.color.unwrap_or(Color::YellowSubtle)),
                (item.text.as_deref(), item.adf.as_ref()),
                item.font_scale,
                Some((item.w.unwrap_or(STICKY_SIZE), f64::INFINITY)),
            ),
            Element::Text(item) => (
                None,
                (item.text.as_deref(), item.adf.as_ref()),
                item.font_scale,
                None,
            ),
            Element::Label(item) => (
                None,
                (item.text.as_deref(), item.adf.as_ref()),
                item.font_scale,
                None,
            ),
            _ => continue,
        };
        if let Some(fill) = fill {
            fills.insert(fill.name());
        }
        let scale = scale.unwrap_or(1.0);
        if scale < 0.75 {
            push(
                "small_text",
                Severity::Warning,
                vec![name(index)],
                "Text below 0.75 scale is hard to read at normal zoom.".into(),
            );
        }
        let lines = text_lines(content.0, content.1);
        if lines.is_empty() {
            continue;
        }
        if let (Some(fill), true) = (
            fill,
            matches!(element, Element::Shape(_) | Element::Sticky(_)),
        ) {
            let ratio = contrast(Color::DefaultText.code(), fill.code());
            if ratio < 4.5 {
                push(
                    "low_contrast",
                    Severity::Warning,
                    vec![name(index)],
                    format!(
                        "Body text on {} has contrast {ratio:.1}:1, under the 4.5:1 minimum; use a subtle or plain fill.",
                        fill.name()
                    ),
                );
            }
        }
        if let Some((w, h)) = inner {
            let width = (w - PADDING).max(GLYPH);
            let needed: f64 = lines
                .iter()
                .map(|(chars, size)| {
                    (chars * GLYPH * scale * size / width).ceil().max(1.0) * LINE * scale * size
                })
                .sum();
            let longest = lines.iter().map(|(chars, _)| *chars).fold(0.0, f64::max);
            if needed + PADDING > h || (longest > 0.0 && longest.min(14.0) * GLYPH * scale > width)
            {
                push(
                    "text_overflow",
                    Severity::Warning,
                    vec![name(index)],
                    format!(
                        "Text needs about {:.0} px of height in a {:.0} x {:.0} box; enlarge the element or shorten the text.",
                        needed + PADDING,
                        w,
                        h
                    ),
                );
            }
        }
    }
    if fills.len() > 6 {
        push(
            "too_many_colors",
            Severity::Info,
            vec![],
            format!(
                "{} fill colors are in use; keep to about five and explain them in a legend.",
                fills.len()
            ),
        );
    }

    let nodes: Vec<(usize, Rect)> = boxes
        .iter()
        .copied()
        .filter(|(index, _)| is_node(&scene.elements[*index]))
        .collect();
    let off_grid: Vec<String> = nodes
        .iter()
        .chain(sections.iter())
        .filter(|(_, r)| {
            [r.x, r.y, r.w, r.h]
                .iter()
                .any(|value| (value / GRID).fract().abs() > 1e-6)
        })
        .map(|(index, _)| name(*index))
        .collect();
    if !off_grid.is_empty() {
        push(
            "off_grid",
            Severity::Info,
            off_grid,
            "Positions and sizes are not on the 8 px grid; round them so edges align.".into(),
        );
    }
    for horizontal in [true, false] {
        let mut lanes: BTreeMap<i64, Vec<Rect>> = BTreeMap::new();
        let mut lane_ids: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (index, area) in &nodes {
            let (cx, cy) = area.center();
            let key = ((if horizontal { cy } else { cx }) / 4.0).round() as i64;
            lanes.entry(key).or_default().push(*area);
            lane_ids.entry(key).or_default().push(*index);
        }
        for (key, mut lane) in lanes {
            if lane.len() < 3 {
                continue;
            }
            lane.sort_by(|a, b| {
                if horizontal {
                    a.x.total_cmp(&b.x)
                } else {
                    a.y.total_cmp(&b.y)
                }
            });
            let gaps: Vec<f64> = lane
                .windows(2)
                .map(|pair| {
                    if horizontal {
                        pair[1].x - pair[0].right()
                    } else {
                        pair[1].y - pair[0].bottom()
                    }
                })
                .collect();
            let pitches: Vec<f64> = lane
                .windows(2)
                .map(|pair| {
                    if horizontal {
                        pair[1].center().0 - pair[0].center().0
                    } else {
                        pair[1].center().1 - pair[0].center().1
                    }
                })
                .collect();
            let spread = |values: &[f64]| {
                let (low, high) = values
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
                (low, high, high - low)
            };
            let (low, high, gap_spread) = spread(&gaps);
            // Even edge gaps or an even center pitch both read as regular spacing.
            if gap_spread > 16.0 && spread(&pitches).2 > 16.0 {
                let ids = lane_ids[&key].iter().map(|index| name(*index)).collect();
                push(
                    "uneven_spacing",
                    Severity::Info,
                    ids,
                    format!(
                        "Gaps in this {} vary from {low:.0} to {high:.0} px; use one spacing value.",
                        if horizontal { "row" } else { "column" }
                    ),
                );
            }
        }
    }

    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.code.cmp(&b.code))
            .then_with(|| a.elements.cmp(&b.elements))
    });
    let warning_count = findings
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .count();
    let info_count = findings.len() - warning_count;
    let score = 100u32.saturating_sub((warning_count * 10 + info_count * 2).min(100) as u32);
    Report {
        warning_count,
        info_count,
        score,
        findings,
    }
}

/// Characters and relative size of each rendered line of text.
fn text_lines(plain: Option<&str>, adf: Option<&Value>) -> Vec<(f64, f64)> {
    let document = match (plain, adf) {
        (Some(plain), _) => text::to_adf(plain),
        (None, Some(adf)) => adf.clone(),
        (None, None) => return Vec::new(),
    };
    let mut lines = Vec::new();
    collect(&document, 1.0, &mut lines);
    lines
}

fn collect(node: &Value, size: f64, lines: &mut Vec<(f64, f64)>) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "paragraph" | "heading" => {
            let size = if kind == "heading" {
                match node.pointer("/attrs/level").and_then(Value::as_u64) {
                    Some(1) => 2.0,
                    Some(2) => 1.7,
                    Some(3) => 1.4,
                    Some(4) => 1.15,
                    _ => 1.0,
                }
            } else {
                size
            };
            let chars: usize = node
                .get("content")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("text").and_then(Value::as_str))
                        .map(|text| text.chars().count())
                        .sum()
                })
                .unwrap_or(0);
            if chars > 0 {
                lines.push((chars as f64, size));
            }
        }
        _ => {
            for child in node
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                collect(child, size, lines);
            }
        }
    }
}

/// WCAG relative-luminance contrast ratio.
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
        let channel = |value: u8| {
            let c = f64::from(value) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }
    let (l1, l2) = (luminance(a), luminance(b));
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}
