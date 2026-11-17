//! Approximate SVG rendering of a scene, so an agent without a browser can look at a board
//! before pasting it. Shapes, colors, text, connectors and labels are drawn from the same
//! geometry the encoder and checks use; fonts, icons and editor routing are approximations.

use std::fmt::Write;

use serde_json::Value;

use super::geometry::*;
use super::scene::*;
use super::text;
use super::vocabulary::*;

const MARGIN: f64 = 40.0;

pub fn svg(scene: &Scene) -> String {
    let boxes: Vec<Rect> = scene
        .elements
        .iter()
        .filter_map(rect)
        .chain(scene.elements.iter().filter_map(|element| match element {
            Element::Line(line) => Some(Rect {
                x: line.x1.min(line.x2),
                y: line.y1.min(line.y2),
                w: (line.x2 - line.x1).abs(),
                h: (line.y2 - line.y1).abs(),
            }),
            Element::Stamp(stamp) => stamp.x.zip(stamp.y).map(|(x, y)| Rect {
                x,
                y,
                w: 60.0,
                h: 60.0,
            }),
            _ => None,
        }))
        .collect();
    let left = boxes.iter().map(|r| r.x).fold(f64::MAX, f64::min).min(0.0) - MARGIN;
    let top = boxes.iter().map(|r| r.y).fold(f64::MAX, f64::min).min(0.0) - MARGIN;
    let right = boxes
        .iter()
        .map(Rect::right)
        .fold(f64::MIN, f64::max)
        .max(0.0)
        + MARGIN;
    let bottom = boxes
        .iter()
        .map(Rect::bottom)
        .fold(f64::MIN, f64::max)
        .max(0.0)
        + MARGIN;
    let mut out = String::new();
    let _ = write!(
        out,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{left} {top} {} {}" width="{}" height="{}" font-family="Atlassian Sans, Segoe UI, Arial, sans-serif"><rect x="{left}" y="{top}" width="{}" height="{}" fill="#ffffff"/><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="context-stroke"/></marker></defs>"##,
        right - left,
        bottom - top,
        (right - left).round(),
        (bottom - top).round(),
        right - left,
        bottom - top
    );
    let ids = ids(scene);
    let segments = segments(scene);
    // Paint order: sections, boxes, connectors, labels and stamps, like the editor.
    for element in &scene.elements {
        if let Element::Section(section) = element {
            let fill = rgb(section.color.unwrap_or(Color::White));
            let _ = write!(
                out,
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="8" fill="{fill}" stroke="#c1c7d0"/>"##,
                section.x, section.y, section.w, section.h
            );
            if let Some(title) = &section.title {
                let _ = write!(
                    out,
                    r##"<text x="{}" y="{}" font-size="16" font-weight="600" fill="#172b4d">{}</text>"##,
                    section.x + 16.0,
                    section.y + 28.0,
                    escape(title)
                );
            }
        }
    }
    for element in &scene.elements {
        match element {
            Element::Shape(shape) => {
                let r = Rect {
                    x: shape.x,
                    y: shape.y,
                    w: shape.w,
                    h: shape.h,
                };
                let fill = if shape.filled.unwrap_or(true) {
                    rgb(shape.fill.unwrap_or(Color::GraySubtle))
                } else {
                    "none".into()
                };
                let rotate = rotation(shape.rotation, &r);
                let outline = r##"stroke="#8590a2" stroke-width="1""##;
                let body = match shape.shape {
                    ShapeKind::Ellipse
                    | ShapeKind::Or
                    | ShapeKind::SummingJunction
                    | ShapeKind::Connector
                    | ShapeKind::Start
                    | ShapeKind::End
                    | ShapeKind::FlowFinal => {
                        let (cx, cy) = r.center();
                        format!(
                            r#"<ellipse cx="{cx}" cy="{cy}" rx="{}" ry="{}" fill="{fill}" {outline}/>"#,
                            r.w / 2.0,
                            r.h / 2.0
                        )
                    }
                    ShapeKind::Diamond | ShapeKind::Decision | ShapeKind::Gateway => {
                        let (cx, cy) = r.center();
                        format!(
                            r#"<polygon points="{cx},{} {},{cy} {cx},{} {},{cy}" fill="{fill}" {outline}/>"#,
                            r.y,
                            r.right(),
                            r.bottom(),
                            r.x
                        )
                    }
                    ShapeKind::StartEnd => format!(
                        r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{fill}" {outline}/>"#,
                        r.x,
                        r.y,
                        r.w,
                        r.h,
                        r.h / 2.0
                    ),
                    ShapeKind::SharpRectangle | ShapeKind::Rectangle | ShapeKind::Process => {
                        format!(
                            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}" {outline}/>"#,
                            r.x, r.y, r.w, r.h
                        )
                    }
                    ShapeKind::UpTriangle => format!(
                        r#"<polygon points="{},{} {},{} {},{}" fill="{fill}" {outline}/>"#,
                        r.center().0,
                        r.y,
                        r.right(),
                        r.bottom(),
                        r.x,
                        r.bottom()
                    ),
                    ShapeKind::DownTriangle => format!(
                        r#"<polygon points="{},{} {},{} {},{}" fill="{fill}" {outline}/>"#,
                        r.x,
                        r.y,
                        r.right(),
                        r.y,
                        r.center().0,
                        r.bottom()
                    ),
                    ShapeKind::LeftParallelogram
                    | ShapeKind::RightParallelogram
                    | ShapeKind::InputOutput => {
                        let slant = r.w * 0.15;
                        format!(
                            r#"<polygon points="{},{} {},{} {},{} {},{}" fill="{fill}" {outline}/>"#,
                            r.x + slant,
                            r.y,
                            r.right(),
                            r.y,
                            r.right() - slant,
                            r.bottom(),
                            r.x,
                            r.bottom()
                        )
                    }
                    ShapeKind::Database
                    | ShapeKind::DatabaseAdvanced
                    | ShapeKind::StoredData
                    | ShapeKind::HardDisk => {
                        let ry = (r.h * 0.12).min(16.0);
                        format!(
                            r#"<path d="M {x} {t} a{rx},{ry} 0 0,0 {w},0 a{rx},{ry} 0 0,0 -{w},0 v{h} a{rx},{ry} 0 0,0 {w},0 v-{h}" fill="{fill}" {outline}/>"#,
                            x = r.x,
                            t = r.y + ry,
                            rx = r.w / 2.0,
                            w = r.w,
                            h = r.h - 2.0 * ry
                        )
                    }
                    _ => format!(
                        r#"<rect x="{}" y="{}" width="{}" height="{}" rx="12" fill="{fill}" {outline}/>"#,
                        r.x, r.y, r.w, r.h
                    ),
                };
                let _ = write!(out, "<g{rotate}>{body}");
                let icon = (32..=52).contains(&shape.shape.code());
                let text_box = if icon {
                    Rect {
                        x: r.x - 40.0,
                        y: r.bottom() + 4.0,
                        w: r.w + 80.0,
                        h: 40.0,
                    }
                } else {
                    r
                };
                if icon {
                    let _ = write!(
                        out,
                        r##"<text x="{}" y="{}" font-size="11" text-anchor="middle" fill="#626f86">{}</text>"##,
                        r.center().0,
                        r.center().1 + 4.0,
                        shape.shape.name()
                    );
                }
                lines(
                    &mut out,
                    &text_box,
                    shape.text.as_deref(),
                    shape.adf.as_ref(),
                    shape.align.unwrap_or(Align::Center),
                    !icon,
                    shape.font_scale,
                    "#172b4d",
                );
                out.push_str("</g>");
            }
            Element::Sticky(sticky) => {
                let r = Rect {
                    x: sticky.x,
                    y: sticky.y,
                    w: sticky.w.unwrap_or(STICKY_SIZE),
                    h: sticky.h.unwrap_or(STICKY_SIZE),
                };
                let rotate = rotation(sticky.rotation, &r);
                let _ = write!(
                    out,
                    r##"<g{rotate}><rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="#091e4224"/>"##,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    rgb(sticky.color.unwrap_or(Color::YellowSubtle))
                );
                lines(
                    &mut out,
                    &r,
                    sticky.text.as_deref(),
                    sticky.adf.as_ref(),
                    sticky.align.unwrap_or(Align::Center),
                    true,
                    sticky.font_scale,
                    "#172b4d",
                );
                out.push_str("</g>");
            }
            Element::Text(item) => {
                let r = Rect {
                    x: item.x,
                    y: item.y,
                    w: item.w,
                    h: TEXT_LINE_HEIGHT,
                };
                let color = rgb(item.color.unwrap_or(Color::DefaultText));
                lines(
                    &mut out,
                    &r,
                    item.text.as_deref(),
                    item.adf.as_ref(),
                    item.align.unwrap_or(Align::Left),
                    false,
                    item.font_scale,
                    &color,
                );
            }
            Element::Link(link) => {
                let r = Rect {
                    x: link.x,
                    y: link.y,
                    w: link.w.unwrap_or(LINK_WIDTH),
                    h: link.h.unwrap_or(LINK_HEIGHT),
                };
                let _ = write!(
                    out,
                    r##"<rect x="{}" y="{}" width="{}" height="{}" rx="8" fill="#ffffff" stroke="#c1c7d0"/><text x="{}" y="{}" font-size="13" fill="#0c66e4">{}</text>"##,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    r.x + 12.0,
                    r.y + 24.0,
                    escape(&truncate(&link.url, 36))
                );
            }
            _ => {}
        }
    }
    for segment in &segments {
        let Element::Connector(connector) = &scene.elements[segment.index] else {
            continue;
        };
        let route = connector.route.unwrap_or(Route::Elbow);
        let d = polyline(&route_points(segment, route, &scene.elements));
        line_style(
            &mut out,
            &d,
            connector.color,
            connector.stroke,
            connector.dash,
            connector.start_cap,
            connector.end_cap,
        );
    }
    for element in &scene.elements {
        if let Element::Line(line) = element {
            let d = format!("M {} {} L {} {}", line.x1, line.y1, line.x2, line.y2);
            line_style(
                &mut out,
                &d,
                line.color,
                line.stroke,
                line.dash,
                line.start_cap,
                line.end_cap,
            );
        }
    }
    for element in &scene.elements {
        match element {
            Element::Label(label) => {
                let Some(&path_index) = ids.get(label.on.as_str()) else {
                    continue;
                };
                let at = label.at.unwrap_or(0.5);
                let point = if let Some(segment) = segments.iter().find(|s| s.index == path_index) {
                    let route = match &scene.elements[path_index] {
                        Element::Connector(connector) => connector.route.unwrap_or(Route::Elbow),
                        _ => Route::Straight,
                    };
                    along(&route_points(segment, route, &scene.elements), at)
                } else if let Element::Line(line) = &scene.elements[path_index] {
                    (
                        line.x1 + (line.x2 - line.x1) * at,
                        line.y1 + (line.y2 - line.y1) * at,
                    )
                } else {
                    continue;
                };
                let words = plain(label.text.as_deref(), label.adf.as_ref()).join(" ");
                let width = words.chars().count() as f64 * 7.4 + 12.0;
                let _ = write!(
                    out,
                    r##"<rect x="{}" y="{}" width="{width}" height="22" rx="4" fill="#ffffff"/><text x="{}" y="{}" font-size="13" text-anchor="middle" fill="{}">{}</text>"##,
                    point.0 - width / 2.0,
                    point.1 - 11.0,
                    point.0,
                    point.1 + 5.0,
                    rgb(label.color.unwrap_or(Color::DefaultText)),
                    escape(&words)
                );
            }
            Element::Stamp(stamp) => {
                let size = stamp.size.unwrap_or(STAMP_SIZE);
                let center = match (&stamp.on, stamp.x, stamp.y) {
                    (Some(on), _, _) => ids
                        .get(on.as_str())
                        .and_then(|i| rect(&scene.elements[*i]))
                        .map(|r| {
                            let (ox, oy) = super::encode::stamp_offset(r.w, size);
                            (r.x + ox, r.y + oy)
                        }),
                    (None, Some(x), Some(y)) => Some((x + size / 2.0, y + size / 2.0)),
                    _ => None,
                };
                if let Some((cx, cy)) = center {
                    let _ = write!(
                        out,
                        r##"<circle cx="{cx}" cy="{cy}" r="{}" fill="#ffffff" stroke="#f5cd47" stroke-width="3"/><text x="{cx}" y="{}" font-size="10" text-anchor="middle" fill="#172b4d">{}</text>"##,
                        size / 2.0,
                        cy + 4.0,
                        stamp.sprite.name()
                    );
                }
            }
            _ => {}
        }
    }
    out.push_str("</svg>");
    out
}

/// The drawn route as a polyline (curves sampled), used both to draw and to place labels.
fn route_points(segment: &Segment, route: Route, elements: &[Element]) -> Vec<(f64, f64)> {
    let (sx, sy) = segment.start;
    let (ex, ey) = segment.end;
    let horizontal = |anchor: (f64, f64)| anchor.0 == 0.0 || anchor.0 == 1.0;
    match route {
        Route::Straight => vec![(sx, sy), (ex, ey)],
        Route::Curved => {
            let pull = |point: (f64, f64), element: usize| {
                let r = rect(&elements[element]).unwrap_or(Rect {
                    x: point.0,
                    y: point.1,
                    w: 0.0,
                    h: 0.0,
                });
                let (cx, cy) = r.center();
                let (dx, dy) = (point.0 - cx, point.1 - cy);
                let length = (dx * dx + dy * dy).sqrt().max(1.0);
                (point.0 + dx / length * 80.0, point.1 + dy / length * 80.0)
            };
            let (c1, c2) = (
                pull(segment.start, segment.from),
                pull(segment.end, segment.to),
            );
            (0..=24)
                .map(|step| {
                    let t = f64::from(step) / 24.0;
                    let u = 1.0 - t;
                    (
                        u * u * u * sx
                            + 3.0 * u * u * t * c1.0
                            + 3.0 * u * t * t * c2.0
                            + t * t * t * ex,
                        u * u * u * sy
                            + 3.0 * u * u * t * c1.1
                            + 3.0 * u * t * t * c2.1
                            + t * t * t * ey,
                    )
                })
                .collect()
        }
        Route::Elbow => {
            if horizontal(segment.from_anchor) && horizontal(segment.to_anchor) {
                let mx = (sx + ex) / 2.0;
                vec![(sx, sy), (mx, sy), (mx, ey), (ex, ey)]
            } else if !horizontal(segment.from_anchor) && !horizontal(segment.to_anchor) {
                let my = (sy + ey) / 2.0;
                vec![(sx, sy), (sx, my), (ex, my), (ex, ey)]
            } else if horizontal(segment.from_anchor) {
                vec![(sx, sy), (ex, sy), (ex, ey)]
            } else {
                vec![(sx, sy), (sx, ey), (ex, ey)]
            }
        }
    }
}

fn polyline(points: &[(f64, f64)]) -> String {
    points
        .iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{} {x} {y}", if i == 0 { "M" } else { " L" }))
        .collect()
}

/// The point at a fraction of a polyline's length.
fn along(points: &[(f64, f64)], at: f64) -> (f64, f64) {
    let lengths: Vec<f64> = points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .collect();
    let mut remaining = lengths.iter().sum::<f64>() * at;
    for (w, length) in points.windows(2).zip(&lengths) {
        if remaining <= *length && *length > 0.0 {
            let t = remaining / length;
            return (
                w[0].0 + (w[1].0 - w[0].0) * t,
                w[0].1 + (w[1].1 - w[0].1) * t,
            );
        }
        remaining -= length;
    }
    points.last().copied().unwrap_or((0.0, 0.0))
}

fn line_style(
    out: &mut String,
    d: &str,
    color: Option<Color>,
    stroke: Option<Stroke>,
    dash: Option<Dash>,
    start: Option<Cap>,
    end: Option<Cap>,
) {
    let width = match stroke.unwrap_or(Stroke::Thin) {
        Stroke::Thin => 1.5,
        Stroke::Medium => 3.0,
        Stroke::Thick => 5.0,
    };
    let dash = match dash.unwrap_or(Dash::Solid) {
        Dash::Solid => "",
        Dash::Dashed => r#" stroke-dasharray="10 6""#,
        Dash::Dotted => r#" stroke-dasharray="2 5""#,
    };
    let marker = |cap: Cap, attribute: &str| {
        if cap == Cap::None {
            String::new()
        } else {
            format!(r#" {attribute}="url(#arrow)""#)
        }
    };
    let _ = write!(
        out,
        r#"<path d="{d}" fill="none" stroke="{}" stroke-width="{width}"{dash}{}{}/>"#,
        rgb(color.unwrap_or(Color::GrayBold)),
        marker(start.unwrap_or(Cap::None), "marker-start"),
        marker(end.unwrap_or(Cap::Arrow), "marker-end")
    );
}

#[allow(clippy::too_many_arguments)]
fn lines(
    out: &mut String,
    area: &Rect,
    plain_text: Option<&str>,
    adf: Option<&Value>,
    align: Align,
    vertical_center: bool,
    scale: Option<f64>,
    color: &str,
) {
    let scale = scale.unwrap_or(1.0);
    let rows = wrapped(plain_text, adf, area.w - 16.0, scale);
    if rows.is_empty() {
        return;
    }
    let height: f64 = rows.iter().map(|(_, size, _)| size * 1.4).sum();
    let mut y = if vertical_center {
        area.center().1 - height / 2.0
    } else {
        area.y + 4.0
    };
    let (x, anchor) = match align {
        Align::Left => (area.x + 8.0, "start"),
        Align::Right => (area.right() - 8.0, "end"),
        Align::Center => (area.center().0, "middle"),
    };
    for (row, size, bold) in rows {
        y += size * 1.4;
        let _ = write!(
            out,
            r#"<text x="{x}" y="{}" font-size="{size}" text-anchor="{anchor}" fill="{color}"{}>{}</text>"#,
            y - size * 0.35,
            if bold { r#" font-weight="600""# } else { "" },
            escape(&row)
        );
    }
}

/// Lines of text with font size and weight, wrapped to the width.
fn wrapped(
    plain_text: Option<&str>,
    adf: Option<&Value>,
    width: f64,
    scale: f64,
) -> Vec<(String, f64, bool)> {
    let document = match (plain_text, adf) {
        (Some(plain_text), _) => text::to_adf(plain_text),
        (None, Some(adf)) => adf.clone(),
        _ => return Vec::new(),
    };
    let mut blocks = Vec::new();
    collect(&document, &mut blocks, "");
    let mut rows = Vec::new();
    for (prefix, words, level) in blocks {
        let size = match level {
            1 => 28.0,
            2 => 24.0,
            3 => 20.0,
            4 => 16.0,
            _ => 14.0,
        } * scale;
        let per_line = ((width / (size * 0.53)).floor() as usize).max(4);
        let mut current = prefix.to_owned();
        for word in words.split_whitespace() {
            if current.chars().count() + word.chars().count() + 1 > per_line
                && !current.trim().is_empty()
            {
                rows.push((current.clone(), size, level > 0));
                current = "  ".into();
            }
            if !current.is_empty() && !current.ends_with(' ') {
                current.push(' ');
            }
            current.push_str(word);
        }
        rows.push((current, size, level > 0));
    }
    rows
}

fn collect(node: &Value, blocks: &mut Vec<(&'static str, String, u64)>, prefix: &'static str) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "paragraph" | "heading" => {
            let words: String = node
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect();
            let level = if kind == "heading" {
                node.pointer("/attrs/level")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
            } else {
                0
            };
            blocks.push((prefix, words, level));
        }
        "bulletList" | "orderedList" => {
            for item in node
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                collect(item, blocks, if kind == "bulletList" { "• " } else { "– " });
            }
        }
        _ => {
            for child in node
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                collect(child, blocks, prefix);
            }
        }
    }
}

fn plain(plain_text: Option<&str>, adf: Option<&Value>) -> Vec<String> {
    wrapped(plain_text, adf, 10_000.0, 1.0)
        .into_iter()
        .map(|(row, _, _)| row)
        .collect()
}

fn rotation(degrees: Option<f64>, area: &Rect) -> String {
    match degrees {
        Some(angle) if angle != 0.0 => {
            let (cx, cy) = area.center();
            format!(r#" transform="rotate({angle} {cx} {cy})""#)
        }
        _ => String::new(),
    }
}

fn rgb(color: Color) -> String {
    let (r, g, b) = color.code();
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        text.chars().take(limit).collect::<String>() + "…"
    }
}
