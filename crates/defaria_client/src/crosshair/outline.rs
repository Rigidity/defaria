//! The outline of a block's shape: the edges of the union of its boxes,
//! so a shape made of several boxes (a wire's core, arms and bends) is
//! outlined as one, without the seams between its boxes.

use bevy::prelude::*;

use defaria_core::content::BlockBox;

/// How far to either side of an edge the occupancy is sampled, in blocks:
/// well under a pixel (1/16), well over rounding.
const SAMPLE: f32 = 1e-3;

/// One edge of an outline, in the block's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub from: Vec3,
    pub to: Vec3,
    /// Which way is out of the shape at the edge, across it: a unit
    /// vector, or zero where two boxes meet only along the edge.
    pub out: Vec3,
}

/// Whether `p` is inside any of `boxes` (not on a face).
fn inside(boxes: &[BlockBox], p: Vec3) -> bool {
    boxes
        .iter()
        .any(|b| p.cmpgt(b.min).all() && p.cmplt(b.max).all())
}

/// Every coordinate along `axis` that a box starts or ends at, sorted.
fn stops(boxes: &[BlockBox], axis: usize) -> Vec<f32> {
    let mut stops: Vec<f32> = boxes
        .iter()
        .flat_map(|b| [b.min[axis], b.max[axis]])
        .collect();
    stops.sort_by(f32::total_cmp);
    stops.dedup_by(|a, b| (*a - *b).abs() < SAMPLE / 10.0);
    stops
}

/// The edges of the union of `boxes`: every line where its surface folds,
/// convex or concave, and none where two boxes only meet face to face.
/// Each line along an axis lies where a box starts or ends on the other
/// two, and is split where any box does along its own; a piece of it is
/// an edge where the four quarters around it aren't filled as a flat face
/// (or nothing, or solid) would fill them. Neighbouring pieces of a line
/// are joined.
pub fn outline(boxes: &[BlockBox]) -> Vec<Edge> {
    let all = [0, 1, 2].map(|axis| stops(boxes, axis));
    let mut edges = Vec::new();
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for &at_u in &all[u] {
            for &at_v in &all[v] {
                let mut run: Option<Edge> = None;
                for span in all[axis].windows(2) {
                    let piece = edge_piece(boxes, axis, at_u, at_v, span[0], span[1]);
                    run = match (run, piece) {
                        (Some(mut edge), Some(next))
                            if edge.out == next.out && edge.to == next.from =>
                        {
                            edge.to = next.to;
                            Some(edge)
                        }
                        (done, next) => {
                            edges.extend(done);
                            next
                        }
                    };
                }
                edges.extend(run);
            }
        }
    }
    edges
}

/// The piece of the line along `axis` at `at_u`, `at_v` (on the next two
/// axes) from `from` to `to`, if it's an edge.
fn edge_piece(
    boxes: &[BlockBox],
    axis: usize,
    at_u: f32,
    at_v: f32,
    from: f32,
    to: f32,
) -> Option<Edge> {
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let point = |along: f32, du: f32, dv: f32| {
        let mut p = Vec3::ZERO;
        p[axis] = along;
        p[u] = at_u + du;
        p[v] = at_v + dv;
        p
    };
    let mid = (from + to) / 2.0;
    // The four quarters around the line, as (u, v) signs.
    let quarters = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let filled = quarters.map(|(su, sv)| inside(boxes, point(mid, su * SAMPLE, sv * SAMPLE)));
    let count = filled.iter().filter(|&&f| f).count();
    // A flat face fills two quarters side by side; two filled across from
    // each other meet only along the line, which is an edge.
    let flat = count == 2 && filled[0] != filled[2];
    if count == 0 || count == 4 || flat {
        return None;
    }
    let mut out = Vec3::ZERO;
    for (&(su, sv), &f) in quarters.iter().zip(&filled) {
        if !f {
            out[u] += su;
            out[v] += sv;
        }
    }
    Some(Edge {
        from: point(from, 0.0, 0.0),
        to: point(to, 0.0, 0.0),
        out: out.normalize_or_zero(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(min: [f32; 3], max: [f32; 3]) -> BlockBox {
        BlockBox {
            min: Vec3::from(min),
            max: Vec3::from(max),
        }
    }

    /// A lone box is outlined by its twelve edges, each pointing out.
    #[test]
    fn a_box_has_twelve_edges() {
        let edges = outline(&[BlockBox::FULL]);
        assert_eq!(edges.len(), 12);
        for edge in &edges {
            let mid = (edge.from + edge.to) / 2.0;
            assert!(
                (mid + edge.out * 0.1 - Vec3::splat(0.5)).length()
                    > (mid - Vec3::splat(0.5)).length()
            );
        }
    }

    /// Two boxes end to end are one: no seam where they meet, and the
    /// edges along them run the whole length.
    #[test]
    fn boxes_end_to_end_have_no_seam() {
        let edges = outline(&[
            b([0.0, 0.0, 0.0], [0.5, 0.25, 0.25]),
            b([0.5, 0.0, 0.0], [1.0, 0.25, 0.25]),
        ]);
        assert_eq!(edges.len(), 12);
        assert!(edges.iter().all(|e| !(e.from.x == 0.5 && e.to.x == 0.5)));
        assert_eq!(
            edges
                .iter()
                .filter(|e| e.from.x == 0.0 && e.to.x == 1.0)
                .count(),
            4
        );
    }

    /// An L of two boxes has its inner, concave edge, though it's no box's
    /// edge, and no seam between the two.
    #[test]
    fn an_l_has_its_inner_edge() {
        let edges = outline(&[
            b([0.0, 0.0, 0.0], [1.0, 0.25, 1.0]),
            b([0.0, 0.0, 0.0], [0.25, 1.0, 1.0]),
        ]);
        // An L extruded along z: 6 corners of the L, each a line along z,
        // and the L's outline (6 edges) at each end.
        assert_eq!(edges.len(), 18);
        let inner = edges
            .iter()
            .find(|e| e.from.x == 0.25 && e.from.y == 0.25 && e.from.z != e.to.z)
            .expect("the inner edge");
        assert_eq!((inner.from.z, inner.to.z), (0.0, 1.0));
        // It points out into the empty quarter, up and east.
        assert!(inner.out.x > 0.0 && inner.out.y > 0.0);
    }
}
