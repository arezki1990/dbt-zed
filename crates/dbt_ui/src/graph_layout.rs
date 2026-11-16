//! A layered, left-to-right layout for a directed graph: each node in a
//! column by the longest path reaching it, the sources pulled next to what
//! they feed. An edge that skips columns passes through each of them in a
//! slot of its own, so it runs between nodes rather than over them. The
//! columns are ordered to keep crossings few, then each node is placed level
//! with what it is linked to, so chains run straight. Nodes can also be
//! grouped into boxes (a folder), each laid out inside its box and the boxes
//! laid out around each other. Pure: no gpui, so any canvas can use it.

use std::collections::{HashMap, HashSet, VecDeque};

/// Where a node is, and its column.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    /// Its column: the center's is 0, upstream ones negative.
    pub level: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placement {
    pub nodes: Vec<Placed>,
    /// For each edge, in the order given, the points it passes through
    /// between its ends, in pairs: where it enters a column it crosses and
    /// where it leaves it. Empty for an edge to the next column.
    pub routes: Vec<Vec<(f32, f32)>>,
    pub width: f32,
    pub height: f32,
}

/// Gaps and margins, in the canvas's units.
#[derive(Clone, Copy, Debug)]
pub struct Spacing {
    pub padding: f32,
    pub column_gap: f32,
    pub row_gap: f32,
}

/// The frame of a group's box: its inner margin and the band on top for
/// its name.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub padding: f32,
    pub header: f32,
}

/// Nodes laid out in their groups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grouped {
    pub placement: Placement,
    /// Each group's box, by group index: x, y, width, height. Zero for a
    /// group no node is in.
    pub boxes: Vec<(f32, f32, f32, f32)>,
}

/// The height of the slot a long edge takes in a column it crosses.
const SLOT_HEIGHT: f32 = 2.;
/// Ordering sweeps; the best ordering seen is kept.
const ORDER_SWEEPS: usize = 24;
/// Placement passes, alternating between what feeds a node and what it feeds.
const PLACE_PASSES: usize = 14;

/// Places nodes of the given `(width, height)` sizes linked by `edges`
/// (`from` feeds `to`, as indices into `sizes`). Edges that close a cycle
/// are left out of the layering only. `center`, when given, sits in
/// column 0.
pub fn layered(
    sizes: &[(f32, f32)],
    edges: &[(usize, usize)],
    center: Option<usize>,
    spacing: Spacing,
) -> Placement {
    let count = sizes.len();
    if count == 0 {
        return Placement {
            nodes: Vec::new(),
            routes: vec![Vec::new(); edges.len()],
            width: 2. * spacing.padding,
            height: 2. * spacing.padding,
        };
    }
    let level = levels(count, edges, center);
    arrange(sizes, &level, edges, spacing, &[])
}

/// Like [`layered`], with each node in at most one group (`group_of`, as
/// indices below `group_count`): a group's nodes are laid out together in
/// a box of their own, with `frame` around them, and the boxes and the
/// nodes in no group are laid out as one graph. An edge between two groups
/// follows the route between their boxes, so edges between the same two
/// groups run together.
pub fn grouped(
    sizes: &[(f32, f32)],
    edges: &[(usize, usize)],
    center: Option<usize>,
    group_of: &[Option<usize>],
    group_count: usize,
    spacing: Spacing,
    frame: Frame,
) -> Grouped {
    let count = sizes.len();
    // A unit is a group's box or a node in no group.
    let mut unit_of = vec![0_usize; count];
    let mut unit_group: Vec<Option<usize>> = Vec::new();
    let mut unit_members: Vec<Vec<usize>> = Vec::new();
    let mut group_unit: Vec<Option<usize>> = vec![None; group_count];
    for node in 0..count {
        let group = group_of
            .get(node)
            .copied()
            .flatten()
            .filter(|group| *group < group_count);
        let unit = match group {
            Some(group) => *group_unit[group].get_or_insert_with(|| {
                unit_group.push(Some(group));
                unit_members.push(Vec::new());
                unit_group.len() - 1
            }),
            None => {
                unit_group.push(None);
                unit_members.push(Vec::new());
                unit_group.len() - 1
            }
        };
        unit_of[node] = unit;
        unit_members[unit].push(node);
    }

    let valid = |&(from, to): &(usize, usize)| from < count && to < count && from != to;
    let inner_spacing = Spacing {
        padding: frame.padding,
        ..spacing
    };
    // The units, as one graph; one edge for each pair of linked units.
    let mut unit_edges: Vec<(usize, usize)> = Vec::new();
    let mut unit_edge_ix: HashMap<(usize, usize), usize> = HashMap::new();
    for edge in edges.iter().filter(|edge| valid(edge)) {
        let pair = (unit_of[edge.0], unit_of[edge.1]);
        if pair.0 != pair.1 && !unit_edge_ix.contains_key(&pair) {
            unit_edge_ix.insert(pair, unit_edges.len());
            unit_edges.push(pair);
        }
    }
    let outer_center = center.filter(|center| *center < count).map(|center| unit_of[center]);

    // Each group laid out on its own (where its nodes sit inside the box),
    // then the units around each other. Then, left to right, each group
    // orders its nodes again by where what feeds them (else what they
    // feed) now sits, and the units are laid out again with those boxes.
    let lay_group = |members: &[usize], hints: &[Option<f32>]| {
        let local: HashMap<usize, usize> = members
            .iter()
            .enumerate()
            .map(|(local, &node)| (node, local))
            .collect();
        let local_sizes: Vec<(f32, f32)> = members.iter().map(|&node| sizes[node]).collect();
        let local_edges: Vec<(usize, usize)> = edges
            .iter()
            .filter(|edge| valid(edge))
            .filter_map(|(from, to)| Some((*local.get(from)?, *local.get(to)?)))
            .collect();
        let local_center = center.and_then(|center| local.get(&center).copied());
        let level = levels(local_sizes.len(), &local_edges, local_center);
        let placement = arrange(&local_sizes, &level, &local_edges, inner_spacing, hints);
        (placement, local)
    };
    let mut inner: Vec<Option<(Placement, HashMap<usize, usize>)>> = unit_members
        .iter()
        .enumerate()
        .map(|(unit, members)| unit_group[unit].map(|_| lay_group(members, &[])))
        .collect();
    let unit_sizes_of = |inner: &[Option<(Placement, HashMap<usize, usize>)>]| -> Vec<(f32, f32)> {
        inner
            .iter()
            .enumerate()
            .map(|(unit, entry)| match entry {
                Some((placement, _)) => (placement.width, placement.height + frame.header),
                None => sizes[unit_members[unit][0]],
            })
            .collect()
    };
    let mut unit_sizes = unit_sizes_of(&inner);
    let mut outer = layered(&unit_sizes, &unit_edges, outer_center, spacing);

    let mut feeds: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut fed: Vec<Vec<usize>> = vec![Vec::new(); count];
    for &(from, to) in edges.iter().filter(|edge| valid(edge)) {
        if unit_of[from] != unit_of[to] {
            feeds[to].push(from);
            fed[from].push(to);
        }
    }
    let mut by_column: Vec<usize> = (0..unit_members.len()).collect();
    by_column.sort_by(|a, b| outer.nodes[*a].x.total_cmp(&outer.nodes[*b].x));
    for unit in by_column {
        if inner[unit].is_none() {
            continue;
        }
        let middle = |inner: &[Option<(Placement, HashMap<usize, usize>)>], node: usize| -> f32 {
            let unit = unit_of[node];
            let at = &outer.nodes[unit];
            match &inner[unit] {
                Some((placement, local)) => {
                    at.y + frame.header + placement.nodes[local[&node]].y + sizes[node].1 / 2.
                }
                None => at.y + sizes[node].1 / 2.,
            }
        };
        let hints: Vec<Option<f32>> = unit_members[unit]
            .iter()
            .map(|&node| {
                let linked = if feeds[node].is_empty() {
                    &fed[node]
                } else {
                    &feeds[node]
                };
                (!linked.is_empty()).then(|| {
                    linked.iter().map(|&other| middle(&inner, other)).sum::<f32>()
                        / linked.len() as f32
                })
            })
            .collect();
        inner[unit] = Some(lay_group(&unit_members[unit], &hints));
    }
    unit_sizes = unit_sizes_of(&inner);
    outer = layered(&unit_sizes, &unit_edges, outer_center, spacing);

    let mut nodes = vec![
        Placed {
            x: 0.,
            y: 0.,
            level: 0,
        };
        count
    ];
    for node in 0..count {
        let unit = unit_of[node];
        let at = &outer.nodes[unit];
        nodes[node] = match &inner[unit] {
            Some((placement, local)) => {
                let inside = &placement.nodes[local[&node]];
                Placed {
                    x: at.x + inside.x,
                    y: at.y + frame.header + inside.y,
                    level: at.level,
                }
            }
            None => at.clone(),
        };
    }

    // Routes: inside a box, the box's own, moved with it; between units,
    // the route between them.
    let mut local_edge_ix: Vec<HashMap<(usize, usize), usize>> = vec![HashMap::new(); inner.len()];
    for (unit, entry) in inner.iter().enumerate() {
        if let Some((_, local)) = entry {
            let mut next = 0;
            for &(from, to) in edges.iter().filter(|edge| valid(edge)) {
                if let (Some(&from), Some(&to)) = (local.get(&from), local.get(&to)) {
                    local_edge_ix[unit].entry((from, to)).or_insert(next);
                    next += 1;
                }
            }
        }
    }
    let routes = edges
        .iter()
        .map(|edge| {
            if !valid(edge) {
                return Vec::new();
            }
            let (from_unit, to_unit) = (unit_of[edge.0], unit_of[edge.1]);
            if from_unit != to_unit {
                return unit_edge_ix
                    .get(&(from_unit, to_unit))
                    .and_then(|ix| outer.routes.get(*ix))
                    .cloned()
                    .unwrap_or_default();
            }
            let Some((placement, local)) = &inner[from_unit] else {
                return Vec::new();
            };
            let at = &outer.nodes[from_unit];
            local_edge_ix[from_unit]
                .get(&(local[&edge.0], local[&edge.1]))
                .and_then(|ix| placement.routes.get(*ix))
                .map(|points| {
                    points
                        .iter()
                        .map(|(x, y)| (at.x + x, at.y + frame.header + y))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();

    let mut boxes = vec![(0., 0., 0., 0.); group_count];
    for (unit, group) in unit_group.iter().enumerate() {
        if let Some(group) = group {
            let at = &outer.nodes[unit];
            boxes[*group] = (at.x, at.y, unit_sizes[unit].0, unit_sizes[unit].1);
        }
    }
    Grouped {
        placement: Placement {
            nodes,
            routes,
            width: outer.width,
            height: outer.height,
        },
        boxes,
    }
}

/// Each node's column: the longest path reaching it, sources moved right
/// before the first column that reads them, shifted so `center` is at 0
/// (or the leftmost column is, without one).
fn levels(count: usize, edges: &[(usize, usize)], center: Option<usize>) -> Vec<i32> {
    let center = center.filter(|center| *center < count);
    let edges: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .filter(|(from, to)| from != to && *from < count && *to < count)
        .collect();
    let forward = acyclic(count, &edges, center);

    let mut incoming = vec![Vec::new(); count];
    let mut outgoing = vec![Vec::new(); count];
    for &(from, to) in &forward {
        outgoing[from].push(to);
        incoming[to].push(from);
    }
    let order = topological(count, &incoming, &outgoing);
    let mut level = vec![0_i32; count];
    for &node in &order {
        for &next in &outgoing[node] {
            level[next] = level[next].max(level[node] + 1);
        }
    }
    // A source sits right before the first column that reads it, rather
    // than far left with a long edge.
    for &node in order.iter().rev() {
        if !incoming[node].is_empty() {
            continue;
        }
        if let Some(nearest) = outgoing[node].iter().map(|next| level[*next]).min() {
            level[node] = nearest - 1;
        }
    }
    let shift = center.map_or_else(
        || level.iter().copied().min().unwrap_or(0),
        |center| level[center],
    );
    for value in &mut level {
        *value -= shift;
    }
    level
}

/// Orders and places nodes already given their columns (`level`).
///
/// `hints`, by node, is where what the node is linked to outside this
/// graph sits (in any one scale): the columns start out in that order, so
/// a group's nodes face what they are linked to.
fn arrange(
    sizes: &[(f32, f32)],
    level: &[i32],
    edges: &[(usize, usize)],
    spacing: Spacing,
    hints: &[Option<f32>],
) -> Placement {
    let count = sizes.len();
    let min_level = level.iter().copied().min().unwrap_or(0);
    let max_level = level.iter().copied().max().unwrap_or(0);
    let column_count = (max_level - min_level + 1) as usize;

    // Nodes and, for each column a long edge crosses, a slot for it.
    let mut column_of: Vec<usize> = level
        .iter()
        .map(|level| (level - min_level) as usize)
        .collect();
    let mut height: Vec<f32> = sizes.iter().map(|size| size.1).collect();
    let mut is_slot = vec![false; count];
    let mut links: Vec<(usize, usize)> = Vec::new();
    let mut slots_of_edge: Vec<Vec<usize>> = vec![Vec::new(); edges.len()];
    let mut seen_links = HashSet::new();
    for (edge_ix, &(from, to)) in edges.iter().enumerate() {
        if from == to || from >= count || to >= count {
            continue;
        }
        let (first, last) = (column_of[from], column_of[to]);
        // An edge back or within a column plays no part in the order.
        if last <= first {
            continue;
        }
        let mut previous = from;
        for column in first + 1..last {
            let slot = column_of.len();
            column_of.push(column);
            height.push(SLOT_HEIGHT);
            is_slot.push(true);
            links.push((previous, slot));
            slots_of_edge[edge_ix].push(slot);
            previous = slot;
        }
        if seen_links.insert((previous, to)) {
            links.push((previous, to));
        }
    }
    let total = column_of.len();
    let mut before = vec![Vec::new(); total];
    let mut after = vec![Vec::new(); total];
    for &(from, to) in &links {
        after[from].push(to);
        before[to].push(from);
    }

    // First order: depth-first from the leftmost column, so what is linked
    // starts out close.
    let mut discovered = vec![usize::MAX; total];
    let mut next_discovery = 0;
    let mut starts: Vec<usize> = (0..total).collect();
    starts.sort_by_key(|&node| (column_of[node], node));
    for start in starts {
        if discovered[start] != usize::MAX {
            continue;
        }
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if discovered[node] != usize::MAX {
                continue;
            }
            discovered[node] = next_discovery;
            next_discovery += 1;
            for &next in after[node].iter().rev() {
                if discovered[next] == usize::MAX {
                    stack.push(next);
                }
            }
        }
    }
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); column_count];
    for node in 0..total {
        columns[column_of[node]].push(node);
    }
    for column in &mut columns {
        column.sort_by_key(|&node| discovered[node]);
    }
    if hints.iter().any(Option::is_some) {
        // A node with no hint of its own takes the mean of its linked
        // nodes' (left to right, then right to left), else stays put.
        let mut key: Vec<Option<f32>> = (0..total)
            .map(|node| hints.get(node).copied().flatten())
            .collect();
        for rightward in [true, false] {
            let order: Vec<usize> = if rightward {
                (0..column_count).collect()
            } else {
                (0..column_count).rev().collect()
            };
            for column_ix in order {
                for &node in &columns[column_ix] {
                    if key[node].is_some() {
                        continue;
                    }
                    let around = if rightward { &before[node] } else { &after[node] };
                    let known: Vec<f32> = around.iter().filter_map(|other| key[*other]).collect();
                    if !known.is_empty() {
                        key[node] = Some(known.iter().sum::<f32>() / known.len() as f32);
                    }
                }
            }
        }
        for column in &mut columns {
            let mut keyed: Vec<(f32, usize, usize)> = Vec::with_capacity(column.len());
            let mut last = f32::NEG_INFINITY;
            for (row, &node) in column.iter().enumerate() {
                // Unhinted nodes keep their place after the one before.
                let value = key[node].unwrap_or(last);
                last = value;
                keyed.push((value, row, node));
            }
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            *column = keyed.into_iter().map(|(_, _, node)| node).collect();
        }
    }
    let mut position = vec![0_usize; total];
    let index = |columns: &Vec<Vec<usize>>, position: &mut Vec<usize>| {
        for column in columns {
            for (row, &node) in column.iter().enumerate() {
                position[node] = row;
            }
        }
    };
    index(&columns, &mut position);

    // Order: barycenter sweeps right then left, each followed by swaps of
    // neighbours that cross less; the ordering with the fewest crossings
    // seen is kept.
    let mut best = columns.clone();
    let mut best_crossings = all_crossings(&columns, &position, &after);
    for sweep in 0..ORDER_SWEEPS {
        if best_crossings == 0 {
            break;
        }
        let rightward = sweep % 2 == 0;
        let order: Vec<usize> = if rightward {
            (1..column_count).collect()
        } else {
            (0..column_count.saturating_sub(1)).rev().collect()
        };
        for column_ix in order {
            let around = if rightward { &before } else { &after };
            let mut keyed: Vec<(f32, usize)> = columns[column_ix]
                .iter()
                .map(|&node| {
                    let linked = &around[node];
                    let key = if linked.is_empty() {
                        position[node] as f32
                    } else {
                        linked.iter().map(|other| position[*other] as f32).sum::<f32>()
                            / linked.len() as f32
                    };
                    (key, node)
                })
                .collect();
            keyed.sort_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then(position[a.1].cmp(&position[b.1]))
            });
            columns[column_ix] = keyed.into_iter().map(|(_, node)| node).collect();
            for (row, &node) in columns[column_ix].iter().enumerate() {
                position[node] = row;
            }
        }
        transpose(&mut columns, &mut position, &before, &after);
        let crossings = all_crossings(&columns, &position, &after);
        if crossings < best_crossings {
            best_crossings = crossings;
            best = columns.clone();
        }
    }
    columns = best;
    index(&columns, &mut position);

    // Columns as wide as their widest node.
    let mut column_x = vec![0_f32; column_count];
    let mut column_width = vec![0_f32; column_count];
    let mut x = spacing.padding;
    for (column_ix, column) in columns.iter().enumerate() {
        let width = column
            .iter()
            .filter(|&&node| !is_slot[node])
            .map(|&node| sizes[node].0)
            .fold(0_f32, f32::max);
        column_x[column_ix] = x;
        column_width[column_ix] = width;
        x += width + spacing.column_gap;
    }

    // Rows: each node level with the middle of what it is linked to, as
    // near as the order and the gaps allow, from the left, from the right,
    // then both. Slots hold to their line hardest, so long edges run
    // straight.
    let gap_between = |upper: usize, lower: usize| {
        if is_slot[upper] && is_slot[lower] {
            spacing.row_gap / 2.
        } else {
            spacing.row_gap
        }
    };
    let mut top = vec![0_f32; total];
    for column in &columns {
        let mut y = 0.;
        for (row, &node) in column.iter().enumerate() {
            if row > 0 {
                y += gap_between(column[row - 1], node);
            }
            top[node] = y;
            y += height[node];
        }
    }
    for pass in 0..PLACE_PASSES {
        let (use_before, use_after, rightward) = match pass {
            pass if pass + 2 >= PLACE_PASSES => (true, true, pass % 2 == 0),
            pass if pass % 2 == 0 => (true, false, true),
            _ => (false, true, false),
        };
        let order: Vec<usize> = if rightward {
            (0..column_count).collect()
        } else {
            (0..column_count).rev().collect()
        };
        for column_ix in order {
            let column = &columns[column_ix];
            if column.is_empty() {
                continue;
            }
            let mut targets = Vec::with_capacity(column.len());
            let mut weights = Vec::with_capacity(column.len());
            let mut offset = 0_f32;
            for (row, &node) in column.iter().enumerate() {
                if row > 0 {
                    offset += height[column[row - 1]] + gap_between(column[row - 1], node);
                }
                let mut sum = 0.;
                let mut linked = 0;
                for list in [use_before.then_some(&before[node]), use_after.then_some(&after[node])]
                    .into_iter()
                    .flatten()
                {
                    for &other in list {
                        sum += top[other] + height[other] / 2.;
                        linked += 1;
                    }
                }
                let (middle, weight) = if linked == 0 {
                    (top[node] + height[node] / 2., 0.05)
                } else if is_slot[node] {
                    (sum / linked as f32, 8.)
                } else {
                    (sum / linked as f32, 1.)
                };
                targets.push(middle - height[node] / 2. - offset);
                weights.push(weight);
            }
            let fitted = increasing_fit(&targets, &weights);
            let mut offset = 0_f32;
            for (row, &node) in column.iter().enumerate() {
                if row > 0 {
                    offset += height[column[row - 1]] + gap_between(column[row - 1], node);
                }
                top[node] = fitted[row] + offset;
            }
        }
    }

    let lowest = top.iter().copied().fold(f32::INFINITY, f32::min);
    let lowest = if lowest.is_finite() { lowest } else { 0. };
    for value in &mut top {
        *value += spacing.padding - lowest;
    }
    let bottom = (0..total)
        .map(|node| top[node] + height[node])
        .fold(0_f32, f32::max);

    let nodes = (0..count)
        .map(|node| Placed {
            x: column_x[column_of[node]],
            y: top[node],
            level: level[node],
        })
        .collect();
    let routes = slots_of_edge
        .iter()
        .map(|slots| {
            slots
                .iter()
                .flat_map(|&slot| {
                    let column = column_of[slot];
                    let middle = top[slot] + height[slot] / 2.;
                    [
                        (column_x[column], middle),
                        (column_x[column] + column_width[column], middle),
                    ]
                })
                .collect()
        })
        .collect();
    Placement {
        nodes,
        routes,
        width: x - spacing.column_gap + spacing.padding,
        height: bottom + spacing.padding,
    }
}

/// The non-decreasing sequence nearest `targets`, each weighed by its
/// weight (pool adjacent violators).
fn increasing_fit(targets: &[f32], weights: &[f32]) -> Vec<f32> {
    // (weighted sum, weight, how many)
    let mut blocks: Vec<(f32, f32, usize)> = Vec::with_capacity(targets.len());
    for (&target, &weight) in targets.iter().zip(weights) {
        blocks.push((target * weight, weight, 1));
        while blocks.len() >= 2 {
            let last = blocks[blocks.len() - 1];
            let previous = blocks[blocks.len() - 2];
            if previous.0 / previous.1 <= last.0 / last.1 {
                break;
            }
            blocks.pop();
            let merged = blocks.last_mut().expect("two blocks");
            merged.0 += last.0;
            merged.1 += last.1;
            merged.2 += last.2;
        }
    }
    let mut fitted = Vec::with_capacity(targets.len());
    for (sum, weight, size) in blocks {
        fitted.extend(std::iter::repeat_n(sum / weight, size));
    }
    fitted
}

/// Crossings between every pair of neighbouring columns.
fn all_crossings(columns: &[Vec<usize>], position: &[usize], after: &[Vec<usize>]) -> usize {
    columns
        .windows(2)
        .map(|pair| crossings(&pair[0], pair[1].len(), position, after))
        .sum()
}

/// Crossings between the links leaving `upper` and the next column, which
/// has `lower_len` nodes.
fn crossings(upper: &[usize], lower_len: usize, position: &[usize], after: &[Vec<usize>]) -> usize {
    let mut pairs: Vec<(usize, usize)> = upper
        .iter()
        .flat_map(|&node| after[node].iter().map(move |&next| (position[node], position[next])))
        .collect();
    pairs.sort_unstable();
    // Count, for each link, the earlier ones that end below it.
    let mut tree = vec![0_usize; lower_len + 1];
    let mut count = 0;
    for (seen, &(_, end)) in pairs.iter().enumerate() {
        let mut at_most = 0;
        let mut ix = end + 1;
        while ix > 0 {
            at_most += tree[ix];
            ix -= ix & ix.wrapping_neg();
        }
        count += seen - at_most;
        let mut ix = end + 1;
        while ix <= lower_len {
            tree[ix] += 1;
            ix += ix & ix.wrapping_neg();
        }
    }
    count
}

/// Swaps neighbours in a column while that crosses less.
fn transpose(
    columns: &mut [Vec<usize>],
    position: &mut [usize],
    before: &[Vec<usize>],
    after: &[Vec<usize>],
) {
    // Crossings among the links of `upper` and `lower` when upper is above.
    let pair_crossings = |upper: usize, lower: usize, position: &[usize]| -> usize {
        let mut count = 0;
        for links in [before, after] {
            for &a in &links[upper] {
                for &b in &links[lower] {
                    if position[a] > position[b] {
                        count += 1;
                    }
                }
            }
        }
        count
    };
    for _ in 0..4 {
        let mut improved = false;
        for column in columns.iter_mut() {
            for row in 0..column.len().saturating_sub(1) {
                let (upper, lower) = (column[row], column[row + 1]);
                if pair_crossings(lower, upper, position) < pair_crossings(upper, lower, position) {
                    column.swap(row, row + 1);
                    position[upper] = row + 1;
                    position[lower] = row;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
}

/// The edges that close no cycle: a depth-first walk from the center
/// first, then from every node, drops the edges back to a node on its path.
fn acyclic(count: usize, edges: &[(usize, usize)], center: Option<usize>) -> Vec<(usize, usize)> {
    let mut outgoing = vec![Vec::new(); count];
    for &(from, to) in edges {
        outgoing[from].push(to);
    }
    // 0: unseen, 1: on the path, 2: done.
    let mut state = vec![0_u8; count];
    let mut back = HashSet::new();
    let starts = center.into_iter().chain(0..count);
    for start in starts {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0_usize)];
        state[start] = 1;
        while let Some((node, next)) = stack.last().copied() {
            if let Some(&to) = outgoing[node].get(next) {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                match state[to] {
                    0 => {
                        state[to] = 1;
                        stack.push((to, 0));
                    }
                    1 => {
                        back.insert((node, to));
                    }
                    _ => {}
                }
            } else {
                state[node] = 2;
                stack.pop();
            }
        }
    }
    edges
        .iter()
        .copied()
        .filter(|edge| !back.contains(edge))
        .collect()
}

/// Kahn's order; what a cycle would leave out comes last, as given.
fn topological(count: usize, incoming: &[Vec<usize>], outgoing: &[Vec<usize>]) -> Vec<usize> {
    let mut remaining: Vec<usize> = incoming.iter().map(Vec::len).collect();
    let mut queue: VecDeque<usize> = (0..count).filter(|&node| remaining[node] == 0).collect();
    let mut order = Vec::with_capacity(count);
    let mut seen = vec![false; count];
    while let Some(node) = queue.pop_front() {
        if seen[node] {
            continue;
        }
        seen[node] = true;
        order.push(node);
        for &next in &outgoing[node] {
            remaining[next] = remaining[next].saturating_sub(1);
            if remaining[next] == 0 {
                queue.push_back(next);
            }
        }
    }
    order.extend((0..count).filter(|&node| !seen[node]));
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACING: Spacing = Spacing {
        padding: 24.,
        column_gap: 70.,
        row_gap: 14.,
    };

    fn boxes_overlap(boxes: &[(f32, f32, f32, f32)]) -> bool {
        boxes.iter().enumerate().any(|(ix, a)| {
            boxes
                .iter()
                .skip(ix + 1)
                .any(|b| a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3)
        })
    }

    fn overlaps(placement: &Placement, sizes: &[(f32, f32)]) -> bool {
        let boxes: Vec<(f32, f32, f32, f32)> = placement
            .nodes
            .iter()
            .zip(sizes)
            .map(|(node, (width, height))| (node.x, node.y, node.x + width, node.y + height))
            .collect();
        boxes_overlap(&boxes)
    }

    #[test]
    fn edges_point_right_and_nodes_do_not_overlap() {
        // Two sources into a job, the job into a table, the table into a
        // second job that also reads a third source; one wide node.
        let sizes = [
            (140., 32.),
            (140., 32.),
            (200., 80.),
            (300., 32.),
            (140., 32.),
            (140., 32.),
        ];
        let edges = [(0, 2), (1, 2), (2, 3), (3, 4), (5, 4)];
        let placement = layered(&sizes, &edges, Some(3), SPACING);
        for &(from, to) in &edges {
            let (from, to) = (&placement.nodes[from], &placement.nodes[to]);
            assert!(to.level > from.level, "{from:?} -> {to:?}");
            assert!(to.x > from.x);
        }
        assert!(!overlaps(&placement, &sizes));
        assert_eq!(placement.nodes[3].level, 0);
        // The late source sits right before what it feeds.
        assert_eq!(placement.nodes[5].level, placement.nodes[4].level - 1);
        assert!(placement.width > 24. * 2.);
        assert_eq!(placement.routes.len(), edges.len());
    }

    #[test]
    fn a_cycle_is_laid_out_all_the_same() {
        let sizes = [(100., 32.); 3];
        let placement = layered(
            &sizes,
            &[(0, 1), (1, 2), (2, 0), (1, 1), (7, 0)],
            Some(0),
            SPACING,
        );
        assert_eq!(placement.nodes.len(), 3);
        assert_eq!(placement.routes.len(), 5);
        assert_eq!(placement.nodes[0].level, 0);
        assert!(
            placement.nodes[1].level > 0 && placement.nodes[2].level > placement.nodes[1].level
        );
        assert!(!overlaps(&placement, &sizes));
        assert_eq!(layered(&[], &[], None, SPACING).nodes.len(), 0);
        let alone = layered(&[(100., 32.)], &[], None, SPACING);
        assert_eq!(
            alone.nodes[0],
            Placed {
                x: 24.,
                y: 24.,
                level: 0
            }
        );
    }

    #[test]
    fn crossings_are_undone_by_the_sweeps() {
        // a→d, b→c drawn in order a, b and c, d would cross.
        let sizes = [(100., 32.); 4];
        let placement = layered(&sizes, &[(0, 3), (1, 2)], None, SPACING);
        let (a, b, c, d) = (
            &placement.nodes[0],
            &placement.nodes[1],
            &placement.nodes[2],
            &placement.nodes[3],
        );
        assert_eq!(a.y < b.y, d.y < c.y);
    }

    #[test]
    fn a_chain_runs_straight() {
        // a → b → c → d, with e feeding c from the side: the chain keeps
        // one line instead of each column being centered on its own.
        let sizes = [(100., 32.); 6];
        let edges = [(0, 1), (1, 2), (2, 3), (4, 2), (5, 3)];
        let placement = layered(&sizes, &edges, None, SPACING);
        let row = |node: usize| placement.nodes[node].y;
        assert!((row(0) - row(1)).abs() < 0.5, "{placement:?}");
        assert!(!overlaps(&placement, &sizes));
    }

    #[test]
    fn a_long_edge_runs_between_the_nodes_it_passes() {
        // a → b → c → d and a → d: the long edge crosses two columns.
        let sizes = [(100., 32.); 4];
        let edges = [(0, 1), (1, 2), (2, 3), (0, 3)];
        let placement = layered(&sizes, &edges, None, SPACING);
        let route = &placement.routes[3];
        assert_eq!(route.len(), 4);
        for (ix, middle) in [1, 2].into_iter().enumerate() {
            let node = &placement.nodes[middle];
            let (left, y) = route[ix * 2];
            assert_eq!(left, node.x);
            assert!(y < node.y || y > node.y + 32., "{route:?} over {node:?}");
        }
        assert!(placement.routes[0].is_empty());
    }

    #[test]
    fn groups_hold_their_nodes_and_do_not_overlap() {
        // Two staging models from a source group, a mart from both, a
        // node in no group read by the mart.
        let sizes = [(120., 32.); 6];
        let edges = [(0, 2), (1, 3), (2, 4), (3, 4), (5, 4)];
        let group_of = [Some(0), Some(0), Some(1), Some(1), Some(2), None];
        let frame = Frame {
            padding: 12.,
            header: 24.,
        };
        let grouped = grouped(&sizes, &edges, Some(4), &group_of, 3, SPACING, frame);
        for (node, group) in group_of.iter().enumerate() {
            let Some(group) = group else { continue };
            let (x, y, width, height) = grouped.boxes[*group];
            let at = &grouped.placement.nodes[node];
            assert!(at.x >= x && at.x + 120. <= x + width, "{node} in {group}");
            assert!(at.y >= y + 24. && at.y + 32. <= y + height, "{node} in {group}");
        }
        let mut rects: Vec<(f32, f32, f32, f32)> = grouped
            .boxes
            .iter()
            .map(|(x, y, width, height)| (*x, *y, x + width, y + height))
            .collect();
        let alone = &grouped.placement.nodes[5];
        rects.push((alone.x, alone.y, alone.x + 120., alone.y + 32.));
        assert!(!boxes_overlap(&rects), "{rects:?}");
        // The boxes go left to right, as their nodes feed each other.
        assert!(grouped.boxes[0].0 < grouped.boxes[1].0);
        assert!(grouped.boxes[1].0 < grouped.boxes[2].0);
        assert_eq!(grouped.placement.routes.len(), edges.len());
    }

    #[test]
    fn the_fit_keeps_order() {
        let fitted = increasing_fit(&[3., 1., 2., 5.], &[1., 1., 1., 1.]);
        assert_eq!(fitted, vec![2., 2., 2., 5.]);
    }
}
