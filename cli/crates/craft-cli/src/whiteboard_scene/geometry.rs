//! Shared scene geometry for layout and checks: element boxes, section membership and
//! connector anchor points. Everything iterates in scene order so results are deterministic.

use std::collections::HashMap;

use super::encode::unit;
use super::scene::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    /// Overlap area, zero when the boxes only touch.
    pub fn overlap(&self, other: &Rect) -> f64 {
        let w = self.right().min(other.right()) - self.x.max(other.x);
        let h = self.bottom().min(other.bottom()) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }
    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
    pub fn point(&self, (ux, uy): (f64, f64)) -> (f64, f64) {
        (self.x + self.w * ux, self.y + self.h * uy)
    }
}

/// Elements laid out and checked as boxes: shapes, stickies and links.
pub fn is_node(element: &Element) -> bool {
    matches!(
        element,
        Element::Shape(_) | Element::Sticky(_) | Element::Link(_)
    )
}

pub fn rect(element: &Element) -> Option<Rect> {
    element.bounds().map(|(x, y, w, h)| Rect { x, y, w, h })
}

pub fn ids(scene: &Scene) -> HashMap<&str, usize> {
    scene
        .elements
        .iter()
        .enumerate()
        .filter_map(|(index, element)| element.id().map(|id| (id, index)))
        .collect()
}

/// Section index for each element whose box lies fully inside a section; the first section in
/// scene order wins when sections overlap.
pub fn section_members(scene: &Scene) -> HashMap<usize, usize> {
    let sections: Vec<(usize, Rect)> = scene
        .elements
        .iter()
        .enumerate()
        .filter_map(|(index, element)| match element {
            Element::Section(_) => rect(element).map(|bounds| (index, bounds)),
            _ => None,
        })
        .collect();
    let named = ids(scene);
    let mut members = HashMap::new();
    for (index, element) in scene.elements.iter().enumerate() {
        if matches!(element, Element::Section(_)) {
            continue;
        }
        if let Some(section) = element.section().and_then(|id| named.get(id)) {
            members.insert(index, *section);
            continue;
        }
        let Some(bounds) = rect(element) else {
            continue;
        };
        if let Some((section, _)) = sections.iter().find(|(_, area)| area.contains(&bounds)) {
            members.insert(index, *section);
        }
    }
    members
}

/// Unit-box anchor for a side, with `auto` resolved against the other element.
pub fn anchor(side: Option<&Side>, own: &Rect, other: &Rect, outgoing: bool) -> (f64, f64) {
    match side {
        Some(Side::Point(point)) => (point.x, point.y),
        Some(Side::Named(name)) if *name != SideName::Auto => unit(*name),
        _ => unit(facing_side(own, other, outgoing)),
    }
}

/// The side of `own` that faces `other`, along the axis with the larger center offset.
pub fn facing_side(own: &Rect, other: &Rect, _outgoing: bool) -> SideName {
    let (ox, oy) = own.center();
    let (tx, ty) = other.center();
    let (dx, dy) = (tx - ox, ty - oy);
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            SideName::Right
        } else {
            SideName::Left
        }
    } else if dy >= 0.0 {
        SideName::Bottom
    } else {
        SideName::Top
    }
}

/// A connector reduced to a straight segment between its two anchor points.
pub struct Segment {
    pub index: usize,
    pub from: usize,
    pub to: usize,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub from_anchor: (f64, f64),
    pub to_anchor: (f64, f64),
}

pub fn segments(scene: &Scene) -> Vec<Segment> {
    let ids = ids(scene);
    let mut result = Vec::new();
    for (index, element) in scene.elements.iter().enumerate() {
        let Element::Connector(item) = element else {
            continue;
        };
        let (Some(&from), Some(&to)) = (ids.get(item.from.as_str()), ids.get(item.to.as_str()))
        else {
            continue;
        };
        let (Some(a), Some(b)) = (rect(&scene.elements[from]), rect(&scene.elements[to])) else {
            continue;
        };
        let from_anchor = anchor(item.from_side.as_ref(), &a, &b, true);
        let to_anchor = anchor(item.to_side.as_ref(), &b, &a, false);
        result.push(Segment {
            index,
            from,
            to,
            start: a.point(from_anchor),
            end: b.point(to_anchor),
            from_anchor,
            to_anchor,
        });
    }
    result
}

/// Proper intersection of two segments; shared endpoints do not count.
pub fn crosses(a: ((f64, f64), (f64, f64)), b: ((f64, f64), (f64, f64))) -> bool {
    fn orient(p: (f64, f64), q: (f64, f64), r: (f64, f64)) -> f64 {
        (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
    }
    let (d1, d2) = (orient(b.0, b.1, a.0), orient(b.0, b.1, a.1));
    let (d3, d4) = (orient(a.0, a.1, b.0), orient(a.0, a.1, b.1));
    d1 * d2 < -1e-9 && d3 * d4 < -1e-9
}

/// Whether a segment passes through the interior of a box (shrunk slightly so grazing edges and
/// endpoints on the border do not count).
pub fn passes_through(segment: ((f64, f64), (f64, f64)), area: &Rect) -> bool {
    let inner = Rect {
        x: area.x + 2.0,
        y: area.y + 2.0,
        w: area.w - 4.0,
        h: area.h - 4.0,
    };
    if inner.w <= 0.0 || inner.h <= 0.0 {
        return false;
    }
    let inside = |p: (f64, f64)| {
        p.0 > inner.x && p.0 < inner.right() && p.1 > inner.y && p.1 < inner.bottom()
    };
    if inside(segment.0) || inside(segment.1) {
        return true;
    }
    let corners = [
        (inner.x, inner.y),
        (inner.right(), inner.y),
        (inner.right(), inner.bottom()),
        (inner.x, inner.bottom()),
    ];
    (0..4).any(|edge| crosses(segment, (corners[edge], corners[(edge + 1) % 4])))
}
