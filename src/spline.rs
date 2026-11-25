//! Cubic Hermite splines: smooth curves through points, each with the
//! slope the curve has there. The terrain maps its noise through them
//! (see `terrain::shape`), as Minecraft's terrain does, so the shape of the
//! land (where plains turn into foothills, how sharp a peak is) is a few
//! numbers rather than formulas. Pure math, no Bevy.

/// A point the curve passes through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knot {
    pub x: f32,
    pub y: f32,
    /// The curve's slope here: `dy/dx`.
    pub slope: f32,
}

/// Shorthand for a [`Knot`].
pub const fn knot(x: f32, y: f32, slope: f32) -> Knot {
    Knot { x, y, slope }
}

/// A smooth curve through its knots, carrying on in a straight line at
/// their slopes beyond the first and last.
#[derive(Debug, Clone, PartialEq)]
pub struct Spline {
    knots: Vec<Knot>,
}

impl Spline {
    /// A curve through `knots`, which must be in order of `x`, at least one.
    pub fn new(knots: impl Into<Vec<Knot>>) -> Self {
        let knots = knots.into();
        assert!(!knots.is_empty(), "a spline needs a knot");
        assert!(
            knots.windows(2).all(|pair| pair[0].x < pair[1].x),
            "knots must be in order of x"
        );
        Self { knots }
    }

    /// The curve's value at `x`.
    pub fn at(&self, x: f32) -> f32 {
        let (first, last) = (self.knots[0], self.knots[self.knots.len() - 1]);
        if x <= first.x {
            return first.y + first.slope * (x - first.x);
        }
        if x >= last.x {
            return last.y + last.slope * (x - last.x);
        }
        // The first knot past `x`; there's one before it, since x > first.x.
        let next = self.knots.partition_point(|knot| knot.x <= x);
        let (a, b) = (self.knots[next - 1], self.knots[next]);
        let width = b.x - a.x;
        let t = (x - a.x) / width;
        let (t2, t3) = (t * t, t * t * t);
        // Hermite basis functions.
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * a.y + h10 * width * a.slope + h01 * b.y + h11 * width * b.slope
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve() -> Spline {
        Spline::new([
            knot(-1.0, 0.0, 0.0),
            knot(0.0, 10.0, 5.0),
            knot(2.0, 20.0, 0.0),
        ])
    }

    #[test]
    fn it_passes_through_its_knots() {
        let curve = curve();
        assert_eq!(curve.at(-1.0), 0.0);
        assert_eq!(curve.at(0.0), 10.0);
        assert_eq!(curve.at(2.0), 20.0);
    }

    #[test]
    fn it_has_the_knots_slopes_there() {
        let curve = curve();
        let slope = |x: f32| (curve.at(x + 1e-3) - curve.at(x - 1e-3)) / 2e-3;
        assert!((slope(0.0) - 5.0).abs() < 0.05, "{}", slope(0.0));
        assert!(slope(-0.999).abs() < 0.05);
        assert!(slope(1.999).abs() < 0.05);
    }

    #[test]
    fn it_is_continuous() {
        let curve = curve();
        let mut last = curve.at(-3.0);
        for step in 1..=5000 {
            let x = -3.0 + step as f32 * 1e-3;
            let y = curve.at(x);
            assert!((y - last).abs() < 0.05, "jump at {x}");
            last = y;
        }
    }

    #[test]
    fn it_carries_on_straight_past_the_ends() {
        let curve = Spline::new([knot(0.0, 1.0, 2.0), knot(1.0, 2.0, -1.0)]);
        assert_eq!(curve.at(-1.0), -1.0);
        assert_eq!(curve.at(3.0), 0.0);
        let flat = Spline::new([knot(0.0, 4.0, 0.0)]);
        assert_eq!(flat.at(-10.0), 4.0);
        assert_eq!(flat.at(10.0), 4.0);
    }

    #[test]
    #[should_panic(expected = "in order")]
    fn knots_out_of_order_are_refused() {
        Spline::new([knot(1.0, 0.0, 0.0), knot(0.0, 0.0, 0.0)]);
    }
}
