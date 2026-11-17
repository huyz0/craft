//! Deterministic auto-layout. Nodes (shapes, stickies, links) move; connectors keep their ends
//! and get spread anchor points; sections are refitted around the nodes they contained.
//!
//! - `layered`: Sugiyama-style. Depth-first cycle breaking, longest-path layers, barycenter
//!   ordering sweeps, centered coordinates. Suits flowcharts, pipelines and dependency maps.
//! - `tree`: tidy tree from the roots (nodes without incoming connectors). Suits mind maps, org
//!   charts and breakdowns.
//! - `grid`: row-major grid, per section when nodes sit in sections (each section becomes a
//!   column of the board). Suits sticky boards, retros and kanban.
//!
//! Spacing values are multiples of the 8 px grid. Equal input gives equal output.

use std::collections::{BTreeMap, HashMap};

use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::geometry::*;
use super::scene::*;
use super::vocabulary::ShapeKind;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    #[default]
    Layered,
    Tree,
    Mindmap,
    Grid,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Right,
    Down,
}

pub struct Options {
    pub algorithm: Algorithm,
    pub direction: Direction,
    pub gap: f64,
    pub columns: Option<usize>,
}

const SECTION_PADDING: f64 = 40.0;
const SECTION_TITLE: f64 = 64.0;

pub fn layout(scene: &Scene, options: &Options) -> Scene {
    let mut scene = scene.clone();
    let members = section_members(&scene);
    let nodes: Vec<usize> = (0..scene.elements.len())
        .filter(|i| is_node(&scene.elements[*i]))
        .collect();
    let edges = edges(&scene);
    let mut sizes = Vec::new();
    let positions = match options.algorithm {
        Algorithm::Layered => layered(&scene, &nodes, &edges, &members, options),
        Algorithm::Tree => tree(&scene, &nodes, &edges, options),
        Algorithm::Mindmap => mindmap(&scene, &nodes, &edges, options),
        Algorithm::Grid => grid(&scene, &nodes, &members, options, &mut sizes),
    };
    for (index, (x, y)) in &positions {
        place(&mut scene.elements[*index], snap(*x), snap(*y));
    }
    for (section, w, h) in sizes {
        if let Element::Section(item) = &mut scene.elements[section] {
            (item.w, item.h) = (snap(w), snap(h));
        }
    }
    if options.algorithm != Algorithm::Grid {
        refit_sections(&mut scene, &members);
    }
    spread_anchors(&mut scene, options);
    scene
}

fn snap(value: f64) -> f64 {
    (value / 8.0).round() * 8.0
}

fn size(element: &Element) -> (f64, f64) {
    rect(element).map_or((0.0, 0.0), |r| (r.w, r.h))
}

fn place(element: &mut Element, x: f64, y: f64) {
    match element {
        Element::Shape(item) => (item.x, item.y) = (x, y),
        Element::Sticky(item) => (item.x, item.y) = (x, y),
        Element::Link(item) => (item.x, item.y) = (x, y),
        Element::Section(item) => (item.x, item.y) = (x, y),
        _ => {}
    }
}

/// Directed node-to-node edges from connectors, in scene order, without self loops.
fn edges(scene: &Scene) -> Vec<(usize, usize)> {
    let ids = ids(scene);
    scene
        .elements
        .iter()
        .filter_map(|element| match element {
            Element::Connector(item) => {
                let from = *ids.get(item.from.as_str())?;
                let to = *ids.get(item.to.as_str())?;
                (from != to && is_node(&scene.elements[from]) && is_node(&scene.elements[to]))
                    .then_some((from, to))
            }
            _ => None,
        })
        .collect()
}

/// Main-axis and cross-axis extents for the direction.
fn extents(scene: &Scene, node: usize, direction: Direction) -> (f64, f64) {
    let (w, h) = size(&scene.elements[node]);
    match direction {
        Direction::Right => (w, h),
        Direction::Down => (h, w),
    }
}

fn to_xy(main: f64, cross: f64, direction: Direction) -> (f64, f64) {
    match direction {
        Direction::Right => (main, cross),
        Direction::Down => (cross, main),
    }
}

fn layered(
    scene: &Scene,
    nodes: &[usize],
    edges: &[(usize, usize)],
    members: &HashMap<usize, usize>,
    options: &Options,
) -> Vec<(usize, (f64, f64))> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let position: HashMap<usize, usize> = nodes.iter().enumerate().map(|(k, n)| (*n, k)).collect();
    let n = nodes.len();
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (from, to) in edges {
        out[position[from]].push(position[to]);
    }
    // Depth-first cycle breaking: an edge back to a node on the stack is reversed.
    let mut state = vec![0u8; n];
    let mut dag: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some((node, next)) = stack.pop() {
            if next < out[node].len() {
                stack.push((node, next + 1));
                let target = out[node][next];
                match state[target] {
                    0 => {
                        dag.push((node, target));
                        state[target] = 1;
                        stack.push((target, 0));
                    }
                    1 => dag.push((target, node)),
                    _ => dag.push((node, target)),
                }
            } else {
                state[node] = 2;
            }
        }
    }
    // Longest-path layering in a stable topological order.
    let mut indegree = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (a, b) in &dag {
        succ[*a].push(*b);
        pred[*b].push(*a);
        indegree[*b] += 1;
    }
    let mut layer = vec![0usize; n];
    let mut ready: std::collections::BTreeSet<usize> =
        (0..n).filter(|v| indegree[*v] == 0).collect();
    while let Some(v) = ready.pop_first() {
        for &w in &succ[v] {
            layer[w] = layer[w].max(layer[v] + 1);
            indegree[w] -= 1;
            if indegree[w] == 0 {
                ready.insert(w);
            }
        }
    }
    // Sections are tiers: all nodes of a section share one layer (the earliest any of them
    // reached), and no two sections or loose layers share a layer, so each section becomes one
    // column (or row) that its refitted box can wrap without overlapping another.
    let tiered = (0..n).any(|v| members.contains_key(&nodes[v]));
    if tiered {
        let mut tier: BTreeMap<usize, usize> = BTreeMap::new();
        for v in 0..n {
            if let Some(section) = members.get(&nodes[v]) {
                let entry = tier.entry(*section).or_insert(layer[v]);
                *entry = (*entry).min(layer[v]);
            }
        }
        let key = |v: usize| match members.get(&nodes[v]) {
            Some(section) => (tier[section], 0, *section),
            None => (layer[v], 1, usize::MAX),
        };
        let keys: std::collections::BTreeSet<(usize, usize, usize)> = (0..n).map(key).collect();
        let rank: HashMap<(usize, usize, usize), usize> =
            keys.into_iter().enumerate().map(|(r, k)| (k, r)).collect();
        layer = (0..n).map(|v| rank[&key(v)]).collect();
    }
    let depth = layer.iter().copied().max().unwrap_or(0) + 1;
    // Long edges get one placeholder per skipped layer so ordering and spacing keep a lane free
    // for the line instead of drawing it through the nodes in between.
    let mut virtual_layer = layer.clone();
    let mut chain_pred = pred.clone();
    let mut chain_succ = succ.clone();
    for (a, b) in &dag {
        let mut previous = *a;
        for l in layer[*a] + 1..layer[*b] {
            let dummy = virtual_layer.len();
            virtual_layer.push(l);
            chain_pred.push(vec![previous]);
            chain_succ.push(Vec::new());
            chain_succ[previous].retain(|w| w != b);
            chain_succ[previous].push(dummy);
            previous = dummy;
        }
        if previous != *a {
            chain_succ[previous].push(*b);
            chain_pred[*b].retain(|w| w != a);
            chain_pred[*b].push(previous);
        }
    }
    let (pred, succ) = (chain_pred, chain_succ);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (v, l) in virtual_layer.iter().enumerate() {
        layers[*l].push(v);
    }
    let lane = |v: usize| -> (f64, f64) {
        if v < n {
            extents(scene, nodes[v], options.direction)
        } else {
            (0.0, 32.0)
        }
    };
    // Barycenter sweeps, down then up, with ties kept in the previous order.
    for sweep in 0..8 {
        let downward = sweep % 2 == 0;
        let order: Vec<usize> = if downward {
            (1..depth).collect()
        } else {
            (0..depth.saturating_sub(1)).rev().collect()
        };
        for l in order {
            let reference = if downward { l - 1 } else { l + 1 };
            let rank: HashMap<usize, usize> = layers[reference]
                .iter()
                .enumerate()
                .map(|(k, v)| (*v, k))
                .collect();
            let mut keyed: Vec<(f64, usize, usize)> = layers[l]
                .iter()
                .enumerate()
                .map(|(k, v)| {
                    let neighbours = if downward { &pred[*v] } else { &succ[*v] };
                    let ranks: Vec<f64> = neighbours
                        .iter()
                        .filter_map(|w| rank.get(w))
                        .map(|r| *r as f64)
                        .collect();
                    let key = if ranks.is_empty() {
                        k as f64
                    } else {
                        ranks.iter().sum::<f64>() / ranks.len() as f64
                    };
                    (key, k, *v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            layers[l] = keyed.into_iter().map(|(_, _, v)| v).collect();
        }
    }
    // Cross-axis centers: start packed and centered, then pull each node toward the mean of its
    // neighbours in the adjacent layer (alternating directions) while keeping order and spacing,
    // so chains run straight instead of zigzagging.
    let total = virtual_layer.len();
    let mut center = vec![0.0f64; total];
    for members in &layers {
        let span: f64 = members.iter().map(|v| lane(*v).1).sum::<f64>()
            + options.gap * members.len().saturating_sub(1) as f64;
        let mut cursor = -span / 2.0;
        for v in members {
            let across = lane(*v).1;
            center[*v] = cursor + across / 2.0;
            cursor += across + options.gap;
        }
    }
    for pass in 0..8 {
        let downward = pass % 2 == 0;
        let order: Vec<usize> = if downward {
            (1..depth).collect()
        } else {
            (0..depth.saturating_sub(1)).rev().collect()
        };
        for l in order {
            let members = &layers[l];
            let desired: Vec<f64> = members
                .iter()
                .map(|v| {
                    let neighbours = if downward { &pred[*v] } else { &succ[*v] };
                    if neighbours.is_empty() {
                        center[*v]
                    } else {
                        neighbours.iter().map(|w| center[*w]).sum::<f64>() / neighbours.len() as f64
                    }
                })
                .collect();
            let half: Vec<f64> = members.iter().map(|v| lane(*v).1 / 2.0).collect();
            let count = members.len();
            let mut forward = desired.clone();
            for i in 1..count {
                forward[i] = forward[i].max(forward[i - 1] + half[i - 1] + options.gap + half[i]);
            }
            let mut backward = desired.clone();
            for i in (0..count.saturating_sub(1)).rev() {
                backward[i] =
                    backward[i].min(backward[i + 1] - half[i + 1] - options.gap - half[i]);
            }
            let mut placed: Vec<f64> = (0..count)
                .map(|i| (forward[i] + backward[i]) / 2.0)
                .collect();
            for i in 1..count {
                placed[i] = placed[i].max(placed[i - 1] + half[i - 1] + options.gap + half[i]);
            }
            for (i, v) in members.iter().enumerate() {
                center[*v] = placed[i];
            }
        }
    }
    let layer_gap = options.gap * 1.5 + if tiered { SECTION_PADDING * 2.0 } else { 0.0 };
    let mut result = Vec::new();
    let mut main = 0.0;
    for members in &layers {
        let thickness = members.iter().map(|v| lane(*v).0).fold(0.0, f64::max);
        for v in members {
            if *v >= n {
                continue;
            }
            let (along, across) = lane(*v);
            result.push((
                nodes[*v],
                to_xy(
                    main + (thickness - along) / 2.0,
                    center[*v] - across / 2.0,
                    options.direction,
                ),
            ));
        }
        main += thickness + layer_gap;
    }
    result
}

/// Two-sided mind map: the root in the middle, first-level branches split between the right
/// and left sides (balanced by leaf count, in order), each side a tidy tree growing outward with
/// leaves stacked beside their branch.
fn mindmap(
    scene: &Scene,
    nodes: &[usize],
    edges: &[(usize, usize)],
    options: &Options,
) -> Vec<(usize, (f64, f64))> {
    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut parent: HashMap<usize, usize> = HashMap::new();
    for (from, to) in edges {
        if !parent.contains_key(to) && to != from {
            parent.insert(*to, *from);
            children.entry(*from).or_default().push(*to);
        }
    }
    let Some(root) = nodes
        .iter()
        .copied()
        .find(|n| !parent.contains_key(n))
        .or_else(|| nodes.first().copied())
    else {
        return Vec::new();
    };
    fn leaves(node: usize, children: &BTreeMap<usize, Vec<usize>>, depth: usize) -> f64 {
        let kids = children.get(&node).map(Vec::as_slice).unwrap_or_default();
        if kids.is_empty() || depth > 64 {
            1.0
        } else {
            kids.iter().map(|k| leaves(*k, children, depth + 1)).sum()
        }
    }
    let branches: Vec<usize> = children.get(&root).cloned().unwrap_or_default();
    // Split branches between the sides: equal branch counts (within one), then the closest leaf
    // totals, keeping branch order on each side; earliest split wins ties. Larger maps alternate.
    let weights: Vec<f64> = branches.iter().map(|b| leaves(*b, &children, 0)).collect();
    let count = branches.len();
    let mut best: Option<(f64, u32)> = None;
    if count <= 16 {
        for mask in 0u32..(1 << count) {
            let right_count = mask.count_ones() as usize;
            if right_count * 2 < count || right_count * 2 > count + 1 {
                continue;
            }
            let right_weight: f64 = (0..count)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| weights[i])
                .sum();
            let left_weight: f64 = weights.iter().sum::<f64>() - right_weight;
            let difference = (right_weight - left_weight).abs();
            if best.is_none_or(|(value, _)| difference < value) {
                best = Some((difference, mask));
            }
        }
    }
    let mask = best.map_or_else(
        || {
            (0..count)
                .filter(|i| i % 2 == 0)
                .fold(0u32, |m, i| m | (1 << (i % 32)))
        },
        |(_, mask)| mask,
    );
    let mut right = Vec::new();
    let mut left = Vec::new();
    for (i, branch) in branches.into_iter().enumerate() {
        if i < 32 && mask & (1 << i) != 0 {
            right.push(branch)
        } else {
            left.push(branch)
        }
    }
    let side_options = Options {
        algorithm: options.algorithm,
        direction: Direction::Right,
        gap: options.gap,
        columns: None,
    };
    let (root_w, root_h) = size(&scene.elements[root]);
    let mut seen = std::collections::BTreeSet::new();
    seen.insert(root);
    let mut result = vec![(root, (-root_w / 2.0, -root_h / 2.0))];
    for (mirror, side) in [(false, right), (true, left)] {
        let mut placed = Vec::new();
        let mut cross = 0.0;
        for branch in side {
            let breadth = place_subtree(
                scene,
                branch,
                root_w / 2.0 + options.gap * 1.5,
                cross,
                &children,
                &mut seen,
                &mut placed,
                &side_options,
            );
            cross += breadth + options.gap;
        }
        let height = (cross - options.gap).max(0.0);
        for (node, (x, y)) in placed {
            let (w, _) = size(&scene.elements[node]);
            let x = if mirror { -x - w } else { x };
            result.push((node, (x, y - height / 2.0)));
        }
    }
    // Anything not reachable from the root goes below the map.
    let mut below = result
        .iter()
        .map(|(n, (_, y))| y + size(&scene.elements[*n]).1)
        .fold(0.0, f64::max)
        + options.gap * 2.0;
    for node in nodes {
        if !seen.contains(node) && *node != root {
            result.push((*node, (0.0, below)));
            below += size(&scene.elements[*node]).1 + options.gap;
        }
    }
    result
}

fn tree(
    scene: &Scene,
    nodes: &[usize],
    edges: &[(usize, usize)],
    options: &Options,
) -> Vec<(usize, (f64, f64))> {
    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut parent: HashMap<usize, usize> = HashMap::new();
    for (from, to) in edges {
        if !parent.contains_key(to) && *to != *from {
            parent.insert(*to, *from);
            children.entry(*from).or_default().push(*to);
        }
    }
    // Break any parent cycle so every node is reachable from exactly one root.
    let mut roots: Vec<usize> = nodes
        .iter()
        .copied()
        .filter(|n| !parent.contains_key(n))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    let mut cross = 0.0;
    let mut pending = nodes.iter().copied();
    loop {
        let root = match roots.first().copied() {
            Some(root) => {
                roots.remove(0);
                root
            }
            None => match pending.find(|n| !seen.contains(n)) {
                Some(root) => root,
                None => break,
            },
        };
        if seen.contains(&root) {
            continue;
        }
        let breadth = place_subtree(
            scene,
            root,
            0.0,
            cross,
            &children,
            &mut seen,
            &mut result,
            options,
        );
        cross += breadth + options.gap;
    }
    result
}

/// Places a subtree with its cross-axis extent starting at `cross`; returns that extent.
#[allow(clippy::too_many_arguments)]
fn place_subtree(
    scene: &Scene,
    node: usize,
    main: f64,
    cross: f64,
    children: &BTreeMap<usize, Vec<usize>>,
    seen: &mut std::collections::BTreeSet<usize>,
    result: &mut Vec<(usize, (f64, f64))>,
    options: &Options,
) -> f64 {
    seen.insert(node);
    let (along, across) = extents(scene, node, options.direction);
    let kids: Vec<usize> = children
        .get(&node)
        .into_iter()
        .flatten()
        .copied()
        .filter(|c| !seen.contains(c))
        .collect();
    let next_main = main + along + options.gap * 1.5;
    let mut offset = cross;
    let mut spans = Vec::new();
    for kid in kids {
        if seen.contains(&kid) {
            continue;
        }
        let breadth = place_subtree(
            scene, kid, next_main, offset, children, seen, result, options,
        );
        spans.push(breadth);
        offset += breadth + options.gap;
    }
    let children_span = if spans.is_empty() {
        0.0
    } else {
        offset - options.gap - cross
    };
    let breadth = across.max(children_span);
    result.push((
        node,
        to_xy(main, cross + (breadth - across) / 2.0, options.direction),
    ));
    breadth
}

/// Places nodes; section sizes go to `sizes` because sections are resized to fit.
fn grid(
    scene: &Scene,
    nodes: &[usize],
    members: &HashMap<usize, usize>,
    options: &Options,
    sizes: &mut Vec<(usize, f64, f64)>,
) -> Vec<(usize, (f64, f64))> {
    let mut groups: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
    for node in nodes {
        groups
            .entry(members.get(node).copied())
            .or_default()
            .push(*node);
    }
    let mut result = Vec::new();
    let mut section_x = 0.0;
    let mut tallest: f64 = 0.0;
    let in_sections = groups.keys().any(Option::is_some);
    for (section, items) in &groups {
        let columns = options
            .columns
            .unwrap_or_else(|| {
                if section.is_some() {
                    2
                } else {
                    (items.len() as f64).sqrt().ceil() as usize
                }
            })
            .max(1);
        let cell_w = items
            .iter()
            .map(|i| size(&scene.elements[*i]).0)
            .fold(0.0, f64::max);
        let cell_h = items
            .iter()
            .map(|i| size(&scene.elements[*i]).1)
            .fold(0.0, f64::max);
        let rows = items.len().div_ceil(columns);
        let used_columns = columns.min(items.len());
        let inner_w =
            used_columns as f64 * cell_w + used_columns.saturating_sub(1) as f64 * options.gap;
        let inner_h = rows as f64 * cell_h + rows.saturating_sub(1) as f64 * options.gap;
        let (origin_x, origin_y) = match section {
            Some(_) => (section_x + SECTION_PADDING, SECTION_TITLE),
            None if in_sections => (0.0, tallest + options.gap * 3.0),
            None => (0.0, 0.0),
        };
        for (k, item) in items.iter().enumerate() {
            let (column, row) = (k % columns, k / columns);
            result.push((
                *item,
                (
                    origin_x + column as f64 * (cell_w + options.gap),
                    origin_y + row as f64 * (cell_h + options.gap),
                ),
            ));
        }
        if let Some(section) = section {
            let w = inner_w + SECTION_PADDING * 2.0;
            let h = inner_h + SECTION_TITLE + SECTION_PADDING;
            result.push((*section, (section_x, 0.0)));
            sizes.push((*section, w, h));
            section_x += w + (options.gap * 2.0).max(64.0);
            tallest = tallest.max(h);
        }
    }
    // Sections side by side end level, which reads as one board rather than ragged columns.
    for size in sizes.iter_mut() {
        size.2 = tallest;
    }
    result
}

fn refit_sections(scene: &mut Scene, members: &HashMap<usize, usize>) {
    let mut bounds: BTreeMap<usize, (f64, f64, f64, f64)> = BTreeMap::new();
    for (element, section) in members {
        let Some(r) = rect(&scene.elements[*element]) else {
            continue;
        };
        let entry = bounds
            .entry(*section)
            .or_insert((f64::MAX, f64::MAX, f64::MIN, f64::MIN));
        *entry = (
            entry.0.min(r.x),
            entry.1.min(r.y),
            entry.2.max(r.right()),
            entry.3.max(r.bottom()),
        );
    }
    for (section, (left, top, right, bottom)) in bounds {
        if let Element::Section(item) = &mut scene.elements[section] {
            item.x = snap(left - SECTION_PADDING);
            item.y = snap(top - SECTION_TITLE);
            item.w = snap(right - left + SECTION_PADDING * 2.0);
            item.h = snap(bottom - top + SECTION_TITLE + SECTION_PADDING);
        }
    }
}

/// Sort key along a side, connector index, and whether this is the connector's start.
type EndKey = (f64, usize, bool);

/// Sets every connector end to the side facing the other end (along the flow for layered and
/// tree layouts) and spreads connectors that share a side evenly along it, ordered by where
/// their other end lies, so lines neither stack nor cross needlessly.
fn spread_anchors(scene: &mut Scene, options: &Options) {
    let ids = ids(scene);
    let mut ends: BTreeMap<(usize, u8), Vec<EndKey>> = BTreeMap::new();
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
        let preferred =
            |own: &Rect, other: &Rect, outgoing: bool| match (options.algorithm, options.direction)
            {
                (Algorithm::Grid, _) => facing_side(own, other, outgoing),
                (_, Direction::Right) if (other.center().0 - own.center().0).abs() > 1.0 => {
                    if other.center().0 > own.center().0 {
                        SideName::Right
                    } else {
                        SideName::Left
                    }
                }
                (_, Direction::Down) if (other.center().1 - own.center().1).abs() > 1.0 => {
                    if other.center().1 > own.center().1 {
                        SideName::Bottom
                    } else {
                        SideName::Top
                    }
                }
                _ => facing_side(own, other, outgoing),
            };
        for (element_index, own, other, outgoing) in [(from, a, b, true), (to, b, a, false)] {
            // Positions changed, so earlier sides no longer face anything: layout owns them.
            let name = preferred(&own, &other, outgoing);
            let (ox, oy) = other.center();
            let key = match name {
                SideName::Top | SideName::Bottom => ox,
                _ => oy,
            };
            ends.entry((element_index, name as u8))
                .or_default()
                .push((key, index, outgoing));
        }
    }
    let sides = [
        SideName::Auto,
        SideName::Top,
        SideName::Right,
        SideName::Bottom,
        SideName::Left,
    ];
    for ((_, side), mut list) in ends {
        list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let count = list.len() as f64;
        let side = sides[side as usize];
        for (k, (_, connector, outgoing)) in list.into_iter().enumerate() {
            let along = if count == 1.0 {
                0.5
            } else {
                (k as f64 + 1.0) / (count + 1.0)
            };
            let along = (along * 1000.0).round() / 1000.0;
            let point = match side {
                SideName::Top => UnitPoint { x: along, y: 0.0 },
                SideName::Bottom => UnitPoint { x: along, y: 1.0 },
                SideName::Left => UnitPoint { x: 0.0, y: along },
                _ => UnitPoint { x: 1.0, y: along },
            };
            let value = if count == 1.0 {
                Side::Named(side)
            } else {
                Side::Point(point)
            };
            if let Element::Connector(item) = &mut scene.elements[connector] {
                if outgoing {
                    item.from_side = Some(value)
                } else {
                    item.to_side = Some(value)
                }
            }
        }
    }
    diamond_exits(scene, options);
}

/// A decision's exits leave from its corners: towards a target above, ahead or below it uses
/// the top, forward or bottom corner, so Yes and No branches separate visibly. Exits that would
/// share a corner keep the spread points.
fn diamond_exits(scene: &mut Scene, options: &Options) {
    if matches!(options.algorithm, Algorithm::Grid | Algorithm::Mindmap) {
        return;
    }
    let ids = ids(scene);
    let mut exits: BTreeMap<usize, Vec<(usize, SideName)>> = BTreeMap::new();
    for (index, element) in scene.elements.iter().enumerate() {
        let Element::Connector(item) = element else {
            continue;
        };
        let (Some(&from), Some(&to)) = (ids.get(item.from.as_str()), ids.get(item.to.as_str()))
        else {
            continue;
        };
        let Element::Shape(shape) = &scene.elements[from] else {
            continue;
        };
        if !matches!(shape.shape, ShapeKind::Diamond | ShapeKind::Decision) {
            continue;
        }
        let (Some(a), Some(b)) = (rect(&scene.elements[from]), rect(&scene.elements[to])) else {
            continue;
        };
        let (cross, half) = match options.direction {
            Direction::Right => (b.center().1 - a.center().1, a.h / 2.0),
            Direction::Down => (b.center().0 - a.center().0, a.w / 2.0),
        };
        let side = match (options.direction, cross) {
            (Direction::Right, c) if c < -half => SideName::Top,
            (Direction::Right, c) if c > half => SideName::Bottom,
            (Direction::Right, _) => SideName::Right,
            (Direction::Down, c) if c < -half => SideName::Left,
            (Direction::Down, c) if c > half => SideName::Right,
            (Direction::Down, _) => SideName::Bottom,
        };
        exits.entry(from).or_default().push((index, side));
    }
    for (_, list) in exits {
        let distinct: std::collections::BTreeSet<u8> =
            list.iter().map(|(_, side)| *side as u8).collect();
        if list.len() < 2 || distinct.len() != list.len() {
            continue;
        }
        for (connector, side) in list {
            if let Element::Connector(item) = &mut scene.elements[connector] {
                item.from_side = Some(Side::Named(side));
            }
        }
    }
}
