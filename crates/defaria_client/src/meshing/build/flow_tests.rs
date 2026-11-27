//! Fluid faces' flow in the mesh (see `meshing::scroll`): which faces flow,
//! which way, and that neighbouring faces agree where they meet.

use bevy::{mesh::VertexAttributeValues, prelude::*};

use super::{ATTRIBUTE_FACE, ATTRIBUTE_FLOW, BACK_FACE, Neighborhood, OPEN_SKY};
use crate::meshing::scroll::{FLOWING, TEXTURE_BITS, WRAPPED};
use defaria_core::{
    content::{BlockState, FULL_FLUID, base, block},
    world::{CHUNK_SIZE, FLUID_STEPS, VoxelFluid, filled, fluid_step},
};

/// One fluid face of a mesh (its front), as drawn.
#[derive(Debug, Clone)]
struct Face {
    normal: Vec3,
    /// Its layer's flags: [`WRAPPED`], [`FLOWING`].
    flags: u32,
    positions: Vec<Vec3>,
    uvs: Vec<Vec2>,
    flows: Vec<Vec2>,
}

impl Face {
    fn flowing(&self) -> bool {
        self.flags & FLOWING != 0
    }

    /// The flow at its middle: what the shader blends to there.
    fn mean(&self) -> Vec2 {
        self.flows.iter().sum::<Vec2>() / self.flows.len() as f32
    }
}

/// A chunk's neighbourhood from `voxel` by world position, for the chunk
/// at `chunk` (in chunks). A voxel is a block or a fluid by name, with the
/// step its fluid is drawn at and the amount of it; `block+fluid` is the
/// block waterlogged with the fluid.
fn neighborhood(
    chunk: IVec3,
    voxel: &impl Fn(IVec3) -> Option<(&'static str, u16, u16)>,
) -> Neighborhood {
    let origin = chunk * CHUNK_SIZE as i32;
    Neighborhood::with_steps(|pos| {
        let Some((name, step, amount)) = voxel(origin + pos) else {
            return (BlockState::AIR, OPEN_SKY, (None, 0));
        };
        let (own, fluid) = match name.split_once('+') {
            Some((own, fluid)) => (block(own), Some(block(fluid))),
            None if base().state(block(name)).is_fluid() => (BlockState::AIR, Some(block(name))),
            None => (block(name), None),
        };
        let fluid = fluid.map(|kind| VoxelFluid { kind, amount });
        (
            own,
            OPEN_SKY,
            (fluid, if fluid.is_some() { step } else { 0 }),
        )
    })
}

/// The fluid faces of the meshes of the chunk at `chunk`, with positions in
/// the world.
fn fluid_faces(
    chunk: IVec3,
    voxel: &impl Fn(IVec3) -> Option<(&'static str, u16, u16)>,
) -> Vec<Face> {
    let meshes = neighborhood(chunk, voxel).meshes(base());
    let fluid_textures: Vec<u32> = ["water", "lava"]
        .iter()
        .flat_map(|name| base().state(block(name)).faces.iter().map(|f| f.layer))
        .collect();
    let origin = (chunk * CHUNK_SIZE as i32).as_vec3();
    let mut out = Vec::new();
    for mesh in [&meshes.opaque, &meshes.translucent] {
        let (
            Some(VertexAttributeValues::Float32x3(positions)),
            Some(VertexAttributeValues::Float32x3(normals)),
            Some(VertexAttributeValues::Float32x2(uvs)),
            Some(VertexAttributeValues::Uint32x2(layers)),
            Some(VertexAttributeValues::Snorm16x2(flows)),
        ) = (
            mesh.attribute(Mesh::ATTRIBUTE_POSITION),
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL),
            mesh.attribute(Mesh::ATTRIBUTE_UV_0),
            mesh.attribute(ATTRIBUTE_FACE),
            mesh.attribute(ATTRIBUTE_FLOW),
        )
        else {
            continue;
        };
        // A face a quad; fronts only.
        for first in (0..positions.len()).step_by(4) {
            let layer = layers[first][0];
            if layer & BACK_FACE != 0 || !fluid_textures.contains(&(layer & TEXTURE_BITS)) {
                continue;
            }
            let quad = first..first + 4;
            out.push(Face {
                normal: Vec3::from(normals[first]),
                flags: layer & !TEXTURE_BITS,
                positions: quad
                    .clone()
                    .map(|i| origin + Vec3::from(positions[i]))
                    .collect(),
                uvs: quad.clone().map(|i| Vec2::from(uvs[i])).collect(),
                flows: quad
                    .map(|i| Vec2::from(flows[i].map(|v| v as f32 / i16::MAX as f32)))
                    .collect(),
            });
        }
    }
    out
}

const FLOOR: i32 = 3;
const A: IVec3 = IVec3::new(4, FLOOR + 1, 4);
const B: IVec3 = IVec3::new(5, FLOOR + 1, 4);

/// Fluid on a floor of stone, each `(position, fluid, drawn step,
/// amount)`; a voxel lookup for [`fluid_faces`].
fn on_stone(
    fluid: Vec<(IVec3, &'static str, u16, u16)>,
) -> impl Fn(IVec3) -> Option<(&'static str, u16, u16)> {
    move |pos| {
        if pos.y == FLOOR && (-4..12).contains(&pos.x) && (-4..12).contains(&pos.z) {
            return Some(("stone", FLUID_STEPS, FULL_FLUID));
        }
        fluid
            .iter()
            .find(|(p, ..)| *p == pos)
            .map(|&(_, name, step, amount)| (name, step, amount))
    }
}

/// Water at these positions and amounts, each drawn at its own height.
fn water(voxels: &[(IVec3, u16)]) -> Vec<(IVec3, &'static str, u16, u16)> {
    voxels
        .iter()
        .map(|&(pos, amount)| (pos, "water", fluid_step(amount), amount))
        .collect()
}

fn faces(fluid: Vec<(IVec3, &'static str, u16, u16)>) -> Vec<Face> {
    fluid_faces(IVec3::ZERO, &on_stone(fluid))
}

fn still(faces: &[Face]) -> bool {
    !faces.is_empty()
        && faces
            .iter()
            .all(|face| !face.flowing() && face.flows.iter().all(|f| *f == Vec2::ZERO))
}

fn top_flows(faces: &[Face]) -> bool {
    faces
        .iter()
        .any(|face| face.normal == Vec3::Y && face.flowing())
}

/// A level pool doesn't flow anywhere, and its faces' texture
/// coordinates come from the world.
#[test]
fn a_level_pool_is_still() {
    let faces = faces(water(&[(A, FULL_FLUID), (B, FULL_FLUID)]));
    assert!(still(&faces), "{faces:?}");
    assert!(faces.iter().all(|face| face.flags & WRAPPED != 0));
}

/// Water next to lower water flows toward it: the tops' flow points that
/// way (u runs along +X on a top).
#[test]
fn water_flows_toward_lower_water() {
    let faces = faces(water(&[(A, FULL_FLUID), (B, FULL_FLUID / 2)]));
    let tops: Vec<&Face> = faces.iter().filter(|f| f.normal == Vec3::Y).collect();
    assert_eq!(tops.len(), 2);
    for top in tops {
        assert!(top.flowing());
        let mean = top.mean();
        assert!(mean.x > 0.3 && mean.y.abs() < 1e-3, "{mean}");
    }
}

/// A top runs down the slope it's drawn at: with lower water beside it on
/// two sides and diagonally, it flows diagonally.
#[test]
fn a_top_runs_down_its_slope() {
    let half = FULL_FLUID / 2;
    let faces = faces(water(&[
        (A, FULL_FLUID),
        (A + IVec3::X, half),
        (A + IVec3::Z, half),
        (A + IVec3::new(1, 0, 1), half),
    ]));
    let top = faces
        .iter()
        .find(|f| f.normal == Vec3::Y && f.positions.iter().all(|p| p.x <= 5.0 && p.z <= 5.0))
        .unwrap();
    let mean = top.mean();
    assert!(mean.x > 0.1 && (mean.x - mean.y).abs() < 1e-3, "{mean}");
}

/// Water over air falls: its sides flow straight down (+v).
#[test]
fn falling_water_flows_down_its_sides() {
    let faces = fluid_faces(IVec3::ZERO, &|pos| {
        (pos == A + IVec3::Y * 4).then_some(("water", FLUID_STEPS, FULL_FLUID))
    });
    let sides: Vec<&Face> = faces.iter().filter(|f| f.normal.y == 0.0).collect();
    assert_eq!(sides.len(), 4);
    for side in sides {
        assert!(side.flowing());
        for flow in &side.flows {
            assert!(flow.distance(Vec2::Y) < 1e-3, "{flow}");
        }
    }
}

/// Whether fluid flows goes by how much it holds, as the flow solver
/// does, not by the heights drawn, which are sticky: a settled pool can be
/// drawn a step or two uneven for good, and a sheet still spreading can
/// be drawn level.
#[test]
fn flow_goes_by_amounts_not_drawn_heights() {
    let pair = |fluid, a: (u16, u16), b: (u16, u16)| {
        faces(vec![(A, fluid, a.0, a.1), (B, fluid, b.0, b.1)])
    };
    // Settled water, the same amount drawn a step apart.
    assert!(still(&pair("water", (8, 460), (7, 460))));
    // Settled lava: less than its ripple (50) apart, drawn two steps apart.
    assert!(still(&pair("lava", (9, 540), (7, 500))));
    // A thin sheet spreading: drawn level, a few dozen units apart.
    assert!(top_flows(&pair("water", (2, 100), (2, 70))));
    // Lava a ripple apart flows.
    assert!(top_flows(&pair("lava", (9, 560), (9, 505))));
    // Where flow stops: water's ripple is only 2 units, but flow can leave
    // neighbours up to an eighth's worth (8) apart; lava stops under its
    // ripple, 50.
    assert!(still(&pair("water", (8, 467), (8, 460))));
    assert!(top_flows(&pair("water", (8, 468), (8, 460))));
    assert!(still(&pair("lava", (9, 549), (9, 500))));
}

/// Fluid on full fluid of its own is a deep pool, not a fall; on fluid of
/// its own with room it falls; on a different fluid it rests.
#[test]
fn only_room_below_makes_a_fall() {
    let up = A + IVec3::Y;
    let sides = |lower: (&'static str, u16), upper: &'static str| -> Vec<Face> {
        faces(vec![
            (A, lower.0, fluid_step(lower.1), lower.1),
            (up, upper, FLUID_STEPS, FULL_FLUID),
        ])
        .into_iter()
        .filter(|f| f.normal.y == 0.0)
        .collect()
    };
    assert!(still(&sides(("water", FULL_FLUID), "water")));
    let falling = sides(("water", FULL_FLUID / 2), "water");
    // The upper voxel's sides flow down; so do the lower's, toward the
    // fall above them, fading to still at their bottom.
    let flowing: Vec<&Vec2> = falling
        .iter()
        .flat_map(|f| &f.flows)
        .filter(|f| f.length() > 1e-3)
        .collect();
    assert!(flowing.len() >= 8, "{falling:?}");
    assert!(
        flowing.iter().all(|f| f.normalize().y > 0.999),
        "{flowing:?}"
    );
    assert!(still(&sides(("water", FULL_FLUID), "lava")));
}

/// Every vertex where faces facing the same way meet has one flow and one
/// texture position (within a whole block), whichever face it's part of:
/// the flow runs on from tile to tile without a seam.
fn assert_seamless(faces: &[Face]) {
    let mut seen: Vec<(Vec3, Vec3, Vec2, Vec2)> = Vec::new();
    let mut shared = 0;
    for face in faces {
        for ((&pos, &uv), &flow) in face.positions.iter().zip(&face.uvs).zip(&face.flows) {
            let uv = uv - uv.floor();
            match seen
                .iter()
                .find(|(n, p, ..)| *n == face.normal && p.distance(pos) < 1e-4)
            {
                Some(&(_, _, other_uv, other_flow)) => {
                    shared += 1;
                    assert!(
                        flow.distance(other_flow) < 1e-3,
                        "{pos} facing {}: {flow} vs {other_flow}",
                        face.normal
                    );
                    let gap = (uv - other_uv).abs();
                    assert!(
                        gap.cmplt(Vec2::splat(1e-4)).all()
                            || gap.cmpgt(Vec2::splat(1.0 - 1e-4)).any(),
                        "{pos}: texture at {uv} vs {other_uv}"
                    );
                }
                None => seen.push((face.normal, pos, uv, flow)),
            }
        }
    }
    assert!(shared > 0, "no faces met");
}

/// A sheet of water spreading out from a pile: every top flows outward,
/// each its own way, and they agree wherever they meet.
#[test]
fn spreading_water_flows_seamlessly() {
    let mut voxels = Vec::new();
    for x in 2..9 {
        for z in 2..9 {
            let distance = (x - 5i32).abs().max((z - 5i32).abs()) as u16;
            let amount = FULL_FLUID - distance * 250;
            voxels.push((IVec3::new(x, FLOOR + 1, z), amount.max(60)));
        }
    }
    // A column falling onto the middle.
    voxels.push((IVec3::new(5, FLOOR + 2, 5), FULL_FLUID / 2));
    voxels.push((IVec3::new(5, FLOOR + 3, 5), FULL_FLUID / 2));
    let faces = faces(water(&voxels));
    let tops: Vec<&Face> = faces
        .iter()
        .filter(|f| f.normal == Vec3::Y && f.flowing())
        .collect();
    assert!(tops.len() > 20, "{}", tops.len());
    // The flow runs outward from the middle, every way.
    let outward = tops
        .iter()
        .filter(|top| {
            let middle = top.positions.iter().sum::<Vec3>() / 4.0;
            let away = (middle.xz() - Vec2::splat(5.5)).normalize_or_zero();
            top.mean().dot(away) > 0.0
        })
        .count();
    assert!(
        outward * 10 >= tops.len() * 9,
        "{outward} of {}",
        tops.len()
    );
    assert_seamless(&faces);
}

/// Two chunks agree on the faces along the edge between them, though each
/// sees only so far past it: the corners there average voxels from both.
#[test]
fn chunks_agree_along_their_edge() {
    let edge = CHUNK_SIZE as i32;
    let voxel = move |pos: IVec3| {
        let ground =
            pos.y == FLOOR && (edge - 6..edge + 6).contains(&pos.x) && (0..8).contains(&pos.z);
        if ground {
            return Some(("stone", FLUID_STEPS, FULL_FLUID));
        }
        // A slope down toward +X across the edge, and along Z.
        let water =
            pos.y == FLOOR + 1 && (edge - 5..edge + 5).contains(&pos.x) && (1..7).contains(&pos.z);
        water.then(|| {
            let amount = (FULL_FLUID as i32 - (pos.x - edge + 5) * 80 - pos.z * 15) as u16;
            ("water", fluid_step(amount), amount)
        })
    };
    let mut faces = fluid_faces(IVec3::ZERO, &voxel);
    faces.extend(fluid_faces(IVec3::X, &voxel));
    assert!(faces.iter().filter(|f| f.flowing()).count() > 10);
    assert_seamless(&faces);
}

/// Water in a waterlogged slab is the same fluid to the flow as the water
/// around it: flowing across a slab, the slab's water flows too, and the
/// faces meet without a seam; a still pond around a slab stays still.
#[test]
fn flow_runs_on_through_waterlogged_blocks() {
    // Water stepping down along +X: full, a slab's half of the voxel
    // three quarters full (its level, 875, between its neighbours'), then
    // three quarters of a block.
    let room = FULL_FLUID / 2;
    let in_slab = room * 3 / 4;
    let row = |amounts: [u16; 3]| {
        let mut voxels = Vec::new();
        for z in 3..6 {
            let at = |x| IVec3::new(x, FLOOR + 1, z);
            voxels.push((at(3), "water", fluid_step(amounts[0]), amounts[0]));
            let slab = fluid_step(filled(amounts[1], room));
            voxels.push((at(4), "stone_slab+water", slab, amounts[1]));
            voxels.push((at(5), "water", fluid_step(amounts[2]), amounts[2]));
        }
        faces(voxels)
    };
    let flowing = row([FULL_FLUID, in_slab, FULL_FLUID * 3 / 4]);
    let slab_top = flowing
        .iter()
        .find(|f| {
            f.normal == Vec3::Y
                && f.positions
                    .iter()
                    .all(|p| (4.0..=5.0).contains(&p.x) && (4.0..=5.0).contains(&p.z))
        })
        .expect("the slab's water has a top");
    assert!(slab_top.flowing(), "{slab_top:?}");
    assert!(slab_top.mean().x > 0.1, "{}", slab_top.mean());
    assert_seamless(&flowing);
    // Level: the slab's half full is as high as full water beside it.
    assert!(still(&row([FULL_FLUID, room, FULL_FLUID])));
}
