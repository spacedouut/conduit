// SPDX-License-Identifier: GPL-3.0-only

//! Default positions for nodes the user hasn't placed themselves.

/// Space kept between nodes.
pub const GAP: f32 = 24.0;
/// Space between the output, other and input columns.
pub const COLUMN_GAP: f32 = 120.0;
/// Where the first column and row start, in canvas space.
pub const ORIGIN: (f32, f32) = (20.0, 20.0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// Whether the two rectangles are closer than [`GAP`].
    fn crowds(&self, other: &Rect) -> bool {
        self.x < other.x + other.width + GAP
            && other.x < self.x + self.width + GAP
            && self.y < other.y + other.height + GAP
            && other.y < self.y + self.height + GAP
    }
}

/// The first `y` at or below `start_y` where a `width`×`height` node at `x`
/// keeps clear of every rectangle in `taken`.
pub fn free_y(taken: &[Rect], x: f32, width: f32, height: f32, start_y: f32) -> f32 {
    let mut candidate = Rect {
        x,
        y: start_y,
        width,
        height,
    };
    while let Some(below) = taken
        .iter()
        .filter(|rect| rect.crowds(&candidate))
        .map(|rect| rect.y + rect.height + GAP)
        .reduce(f32::max)
    {
        candidate.y = below;
    }
    candidate.y
}

/// Stack nodes into columns, left to right, keeping clear of `taken`.
///
/// `nodes` holds each node's column and size, in the order they should be
/// stacked. Returns the origin of each node.
pub fn stack(taken: &[Rect], nodes: &[(usize, f32, f32)], columns: usize) -> Vec<(f32, f32)> {
    let mut column_x = Vec::with_capacity(columns);
    let mut x = ORIGIN.0;
    for column in 0..columns {
        column_x.push(x);
        let widest = nodes
            .iter()
            .filter(|(node_column, _, _)| *node_column == column)
            .map(|(_, width, _)| *width)
            .reduce(f32::max);
        if let Some(widest) = widest {
            x += widest + COLUMN_GAP;
        }
    }

    let mut taken = taken.to_vec();
    let mut next_y = vec![ORIGIN.1; columns];
    nodes
        .iter()
        .map(|&(column, width, height)| {
            let column = column.min(columns.saturating_sub(1));
            let x = column_x[column];
            let y = free_y(&taken, x, width, height, next_y[column]);
            taken.push(Rect {
                x,
                y,
                width,
                height,
            });
            next_y[column] = y + height + GAP;
            (x, y)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_without_overlap() {
        let positions = stack(
            &[],
            &[(0, 100.0, 50.0), (0, 80.0, 200.0), (0, 90.0, 30.0)],
            3,
        );
        assert_eq!(positions[0], ORIGIN);
        assert_eq!(positions[1], (ORIGIN.0, ORIGIN.1 + 50.0 + GAP));
        assert_eq!(
            positions[2],
            (ORIGIN.0, ORIGIN.1 + 50.0 + GAP + 200.0 + GAP)
        );
    }

    #[test]
    fn columns_are_spaced_by_their_widest_node() {
        let positions = stack(
            &[],
            &[(0, 300.0, 50.0), (2, 100.0, 50.0), (1, 150.0, 50.0)],
            3,
        );
        assert_eq!(positions[0].0, ORIGIN.0);
        assert_eq!(positions[2].0, ORIGIN.0 + 300.0 + COLUMN_GAP);
        assert_eq!(
            positions[1].0,
            ORIGIN.0 + 300.0 + COLUMN_GAP + 150.0 + COLUMN_GAP
        );
        assert!(positions.iter().all(|(_, y)| *y == ORIGIN.1));
    }

    #[test]
    fn empty_columns_take_no_space() {
        let positions = stack(&[], &[(0, 200.0, 50.0), (2, 100.0, 50.0)], 3);
        assert_eq!(positions[1].0, ORIGIN.0 + 200.0 + COLUMN_GAP);
    }

    #[test]
    fn skips_past_nodes_the_user_placed() {
        let placed = Rect {
            x: 0.0,
            y: 40.0,
            width: 200.0,
            height: 100.0,
        };
        let y = free_y(&[placed], ORIGIN.0, 100.0, 50.0, ORIGIN.1);
        assert_eq!(y, 40.0 + 100.0 + GAP);
        let far_away = Rect {
            x: 1000.0,
            ..placed
        };
        assert_eq!(
            free_y(&[far_away], ORIGIN.0, 100.0, 50.0, ORIGIN.1),
            ORIGIN.1
        );
    }
}
