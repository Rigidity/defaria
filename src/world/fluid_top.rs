//! The sloped surface of a fluid voxel ([`FluidTop`]): what's drawn, swum
//! in and fogged under.

use bevy::prelude::*;

use super::chunk::fluid_surface;
use crate::content::BlockState;

/// What one of the voxels around a corner of a fluid's top is to that
/// corner's height (see [`FluidTop`]).
#[derive(Debug, Clone, Copy, PartialEq)]
enum AroundCorner {
    /// Not the same fluid: it doesn't count.
    Other,
    /// The same fluid, its surface this high (see [`fluid_surface`]).
    Surface(f32),
    /// The same fluid, with the same fluid above it: a column, which
    /// raises the corner to a full block.
    Covered,
}

/// The surface of a fluid voxel: how high each corner of its top is, in
/// blocks above the voxel's bottom. Each corner is at the average height
/// of the voxels of the same fluid around it (the four that share its
/// vertical edge, this one among them), or a full block if any of them
/// has the same fluid above it. So a flat pool stays flat, a surface
/// slopes down toward where it flows, and two voxels of the same fluid
/// side by side meet at the same heights, with no step between them.
/// Each voxel's own height is sticky (see `sticky_step`), so the
/// corners are too.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidTop {
    /// By corner: `[x][z]`, 0 at the voxel's low side and 1 at its high.
    corners: [[f32; 2]; 2],
}

impl FluidTop {
    /// The top of the fluid of block `fluid` at `pos`, with `voxel` giving
    /// the block at a position and the height its fluid is drawn at (see
    /// [`Chunk::fluid_step_at`](super::Chunk::fluid_step_at)), or `None` where nothing is loaded, which
    /// counts as neither the fluid nor anything over it. Meshing and
    /// swimming both build it here, so they see the same surface.
    pub fn of(
        fluid: BlockState,
        pos: IVec3,
        voxel: impl Fn(IVec3) -> Option<(BlockState, u16)>,
    ) -> Self {
        Self::around(|offset| {
            let at = pos + IVec3::new(offset.x, 0, offset.y);
            match voxel(at) {
                Some((here, step)) if here == fluid => {
                    if voxel(at + IVec3::Y).is_some_and(|(above, _)| above == fluid) {
                        AroundCorner::Covered
                    } else {
                        AroundCorner::Surface(fluid_surface(step))
                    }
                }
                _ => AroundCorner::Other,
            }
        })
    }

    /// The top of a fluid voxel, from what each voxel beside it (and
    /// diagonally) is to its corners, by offset in X and Z from it
    /// (each -1, 0 or 1; `(0, 0)` is the voxel itself).
    fn around(voxel: impl Fn(IVec2) -> AroundCorner) -> Self {
        let around: [[AroundCorner; 3]; 3] = std::array::from_fn(|x| {
            std::array::from_fn(|z| voxel(IVec2::new(x as i32 - 1, z as i32 - 1)))
        });
        let corner = |x: usize, z: usize| {
            let (mut sum, mut count) = (0.0, 0);
            for column in [x, x + 1] {
                for row in [z, z + 1] {
                    match around[column][row] {
                        AroundCorner::Covered => return 1.0,
                        AroundCorner::Surface(height) => {
                            sum += height;
                            count += 1;
                        }
                        AroundCorner::Other => {}
                    }
                }
            }
            if count == 0 { 0.0 } else { sum / count as f32 }
        };
        Self {
            corners: std::array::from_fn(|x| std::array::from_fn(|z| corner(x, z))),
        }
    }

    /// A top at `height` all over.
    #[cfg(test)]
    pub fn flat(height: f32) -> Self {
        Self {
            corners: [[height; 2]; 2],
        }
    }

    /// The height of the corner at `x` and `z`, each 0 or 1.
    pub fn corner(&self, x: usize, z: usize) -> f32 {
        self.corners[x][z]
    }

    /// The height of the surface at `xz` in the voxel (each from 0 to 1),
    /// blended between the corners. The drawn top is two triangles, which
    /// can differ from this by no more than its slope across the voxel.
    pub fn at(&self, xz: Vec2) -> f32 {
        let [[a, b], [c, d]] = self.corners;
        let xz = xz.clamp(Vec2::ZERO, Vec2::ONE);
        let low = a + (c - a) * xz.x;
        let high = b + (d - b) * xz.x;
        low + (high - low) * xz.y
    }

    /// Whether every corner is a full block high.
    pub fn is_full(&self) -> bool {
        self.corners.iter().flatten().all(|&h| h >= 1.0)
    }

    /// The height at a vertex of the voxel's model, by its X and Z.
    pub(crate) fn at_vertex(&self, pos: Vec3) -> f32 {
        self.corner((pos.x > 0.5) as usize, (pos.z > 0.5) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The top of the fluid at the middle of a 3×3 patch, each voxel given
    /// as what it is to the corners.
    fn top_of(patch: [[AroundCorner; 3]; 3]) -> FluidTop {
        FluidTop::around(|offset| patch[(offset.x + 1) as usize][(offset.y + 1) as usize])
    }

    /// Each corner is at the average height of the same fluid around it,
    /// leaving out anything else; a column beside a corner raises it to a
    /// full block.
    #[test]
    fn corners_average_the_fluid_around_them() {
        use AroundCorner::{Covered, Other, Surface};
        let half = Surface(0.5);
        // A flat pool stays flat, and so does its edge.
        assert_eq!(top_of([[half; 3]; 3]), FluidTop::flat(0.5));
        assert_eq!(
            top_of([[Other; 3], [half; 3], [half; 3]]),
            FluidTop::flat(0.5)
        );
        // A step down from a full row slopes from it.
        let top = top_of([[Surface(1.0); 3], [half; 3], [Surface(0.25); 3]]);
        assert_eq!(top.corner(0, 0), 0.75);
        assert_eq!(top.corner(0, 1), 0.75);
        assert_eq!(top.corner(1, 0), 0.375);
        assert_eq!(top.at(Vec2::new(0.5, 0.5)), 0.5625);
        // A column on one diagonal raises only that corner.
        let mut patch = [[half; 3]; 3];
        patch[2][2] = Covered;
        let top = top_of(patch);
        assert_eq!(top.corner(1, 1), 1.0);
        assert_eq!(
            [top.corner(0, 0), top.corner(0, 1), top.corner(1, 0)],
            [0.5; 3]
        );
        // A voxel that's a column itself is full all round.
        let mut patch = [[Other; 3]; 3];
        patch[1][1] = Covered;
        assert!(top_of(patch).is_full());
    }
}
