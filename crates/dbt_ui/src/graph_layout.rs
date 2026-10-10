//! A layered, left-to-right layout for a directed graph: each node in a
//! column by the longest path reaching it, the sources pulled next to what
//! they feed, the columns ordered to keep crossings few, each node placed
//! with its own size. Pure: no gpui, so any canvas can use it.

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
            width: 2. * spacing.padding,
            height: 2. * spacing.padding,
        };
    }
    let edges: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .filter(|(from, to)| from != to && *from < count && *to < count)
        .collect();
    let forward = acyclic(count, &edges, center);

    // Longest path from the sources, in topological order.
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

    // Columns, ordered: first as given, then by the mean position of the
    // neighbours, sweeping right then left.
    let min_level = level.iter().copied().min().unwrap_or(0);
    let max_level = level.iter().copied().max().unwrap_or(0);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); (max_level - min_level + 1) as usize];
    for node in 0..count {
        columns[(level[node] - min_level) as usize].push(node);
    }
    let mut position = vec![0_f32; count];
    let place = |columns: &Vec<Vec<usize>>, position: &mut Vec<f32>| {
        for column in columns {
            for (row, &node) in column.iter().enumerate() {
                position[node] = row as f32;
            }
        }
    };
    place(&columns, &mut position);
    for sweep in 0..4 {
        let rightward = sweep % 2 == 0;
        let indices: Vec<usize> = if rightward {
            (1..columns.len()).collect()
        } else {
            (0..columns.len().saturating_sub(1)).rev().collect()
        };
        for ix in indices {
            let neighbours = if rightward { &incoming } else { &outgoing };
            let mut keyed: Vec<(f32, usize)> = columns[ix]
                .iter()
                .map(|&node| {
                    let around = &neighbours[node];
                    let key = if around.is_empty() {
                        position[node]
                    } else {
                        around.iter().map(|other| position[*other]).sum::<f32>()
                            / around.len() as f32
                    };
                    (key, node)
                })
                .collect();
            keyed.sort_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then(position[a.1].total_cmp(&position[b.1]))
            });
            columns[ix] = keyed.into_iter().map(|(_, node)| node).collect();
            for (row, &node) in columns[ix].iter().enumerate() {
                position[node] = row as f32;
            }
        }
    }

    // Positions: columns as wide as their widest node, each centered on the
    // tallest column.
    let column_height = |column: &Vec<usize>| -> f32 {
        let total: f32 = column.iter().map(|&node| sizes[node].1).sum();
        total + spacing.row_gap * column.len().saturating_sub(1) as f32
    };
    let tallest = columns.iter().map(column_height).fold(0_f32, f32::max);
    let mut nodes = vec![
        Placed {
            x: 0.,
            y: 0.,
            level: 0
        };
        count
    ];
    let mut x = spacing.padding;
    for column in &columns {
        let width = column
            .iter()
            .map(|&node| sizes[node].0)
            .fold(0_f32, f32::max);
        let mut y = spacing.padding + (tallest - column_height(column)) / 2.;
        for &node in column {
            nodes[node] = Placed {
                x,
                y,
                level: level[node],
            };
            y += sizes[node].1 + spacing.row_gap;
        }
        x += width + spacing.column_gap;
    }
    Placement {
        nodes,
        width: x - spacing.column_gap + spacing.padding,
        height: tallest + 2. * spacing.padding,
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
    let mut back = std::collections::HashSet::new();
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
    let mut queue: std::collections::VecDeque<usize> =
        (0..count).filter(|&node| remaining[node] == 0).collect();
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

    fn overlaps(placement: &Placement, sizes: &[(f32, f32)]) -> bool {
        let boxes: Vec<(f32, f32, f32, f32)> = placement
            .nodes
            .iter()
            .zip(sizes)
            .map(|(node, (width, height))| (node.x, node.y, node.x + width, node.y + height))
            .collect();
        boxes.iter().enumerate().any(|(ix, a)| {
            boxes
                .iter()
                .skip(ix + 1)
                .any(|b| a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3)
        })
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
}
