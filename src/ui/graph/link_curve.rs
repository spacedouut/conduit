// SPDX-License-Identifier: GPL-3.0-only

//! Shape of the curve a link is drawn as, and whether any of it can be on screen.

/// The cubic Bézier curve drawn for a link, in screen space of the graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkCurve {
    pub start: (f64, f64),
    pub control1: (f64, f64),
    pub control2: (f64, f64),
    pub end: (f64, f64),
}

impl LinkCurve {
    /// The curve from the anchor of an output port to the anchor of an input port.
    pub fn new(output: (f64, f64), input: (f64, f64)) -> Self {
        let (output_x, output_y) = output;
        let (input_x, input_y) = input;

        // If the output port is farther right than the input port and they have
        // a similar y coordinate, apply a y offset to the control points
        // so that the curve sticks out a bit.
        let y_control_offset = if output_x > input_x {
            f64::max(0.0, 25.0 - (output_y - input_y).abs())
        } else {
            0.0
        };

        // Place curve control offset by half the x distance between the two points.
        // This makes the curve scale well for varying distances between the two ports,
        // especially when the output port is farther right than the input port.
        let half_x_dist = f64::abs(output_x - input_x) / 2.0;

        Self {
            start: output,
            control1: (output_x + half_x_dist, output_y - y_control_offset),
            control2: (input_x - half_x_dist, input_y - y_control_offset),
            end: input,
        }
    }

    /// Whether a stroke of `line_width` along the curve may touch the `width`×`height`
    /// area with its origin at `(0, 0)`.
    ///
    /// A curve never leaves the bounding box of its four points, so this is `false`
    /// only for links that are entirely outside the area. It can be `true` for a link
    /// that just misses the area, which is then drawn needlessly but correctly.
    pub fn may_be_visible(&self, line_width: f64, width: f64, height: f64) -> bool {
        let points = [self.start, self.control1, self.control2, self.end];
        let min = |f: fn(&(f64, f64)) -> f64| points.iter().map(f).fold(f64::INFINITY, f64::min);
        let max =
            |f: fn(&(f64, f64)) -> f64| points.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
        let margin = line_width / 2.0;

        min(|p| p.0) - margin <= width
            && max(|p| p.0) + margin >= 0.0
            && min(|p| p.1) - margin <= height
            && max(|p| p.1) + margin >= 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: f64 = 800.0;
    const HEIGHT: f64 = 600.0;

    fn visible(output: (f64, f64), input: (f64, f64)) -> bool {
        LinkCurve::new(output, input).may_be_visible(2.0, WIDTH, HEIGHT)
    }

    #[test]
    fn link_inside_the_view_is_visible() {
        assert!(visible((100.0, 100.0), (400.0, 300.0)));
    }

    #[test]
    fn link_with_one_end_in_the_view_is_visible() {
        assert!(visible((-500.0, 100.0), (400.0, 300.0)));
        assert!(visible((400.0, 300.0), (2000.0, 900.0)));
    }

    #[test]
    fn link_crossing_the_view_with_both_ends_outside_is_visible() {
        // Left to right, top to bottom, and corner to corner.
        assert!(visible((-300.0, 300.0), (1200.0, 320.0)));
        assert!(visible((390.0, -200.0), (410.0, 900.0)));
        assert!(visible((-100.0, -100.0), (900.0, 700.0)));
    }

    #[test]
    fn link_entirely_on_one_side_of_the_view_is_hidden() {
        assert!(!visible((-400.0, 100.0), (-100.0, 300.0)));
        assert!(!visible((900.0, 100.0), (1300.0, 300.0)));
        assert!(!visible((100.0, -300.0), (400.0, -50.0)));
        assert!(!visible((100.0, 700.0), (400.0, 900.0)));
    }

    #[test]
    fn backwards_link_bulging_into_the_view_is_visible() {
        // The output is right of the input, so the curve swings out past both ends by
        // half their x distance: both ends are left of the view, the curve is not.
        let curve = LinkCurve::new((-10.0, 300.0), (-210.0, 300.0));
        assert!(curve.control1.0 > 0.0);
        assert!(curve.may_be_visible(2.0, WIDTH, HEIGHT));
    }

    #[test]
    fn backwards_link_raised_into_the_view_is_visible() {
        // Ends at a similar height below the view; the curve is lifted by up to 25px.
        assert!(visible((500.0, 610.0), (300.0, 610.0)));
        assert!(!visible((300.0, 610.0), (500.0, 610.0)));
    }

    #[test]
    fn stroke_width_reaching_into_the_view_is_visible() {
        let curve = LinkCurve::new((100.0, -4.0), (400.0, -4.0));
        assert!(!curve.may_be_visible(2.0, WIDTH, HEIGHT));
        assert!(curve.may_be_visible(10.0, WIDTH, HEIGHT));
    }

    #[test]
    fn link_touching_the_edge_of_the_view_is_visible() {
        assert!(visible((-200.0, 100.0), (0.0, 100.0)));
        assert!(visible((WIDTH, 100.0), (WIDTH + 200.0, 100.0)));
    }

    #[test]
    fn control_points_match_the_drawn_curve() {
        let curve = LinkCurve::new((100.0, 200.0), (300.0, 260.0));
        assert_eq!(curve.start, (100.0, 200.0));
        assert_eq!(curve.control1, (200.0, 200.0));
        assert_eq!(curve.control2, (200.0, 260.0));
        assert_eq!(curve.end, (300.0, 260.0));

        // Output right of input at a similar height: both control points are raised.
        let curve = LinkCurve::new((300.0, 200.0), (100.0, 210.0));
        assert_eq!(curve.control1, (400.0, 185.0));
        assert_eq!(curve.control2, (0.0, 195.0));
    }
}
