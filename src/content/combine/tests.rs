use super::*;
use crate::content::{SideTexture, base, block_with, slab, test_mod};

#[test]
fn every_slab_is_a_half_on_the_side_it_names() {
    let content = base();
    let ids = [
        "stone",
        "planks",
        "thatch",
        "stone_brick",
        "smooth_stone",
        "cobblestone",
    ];
    for (half, side) in [
        ("bottom", Direction::Down),
        ("top", Direction::Up),
        ("north", Direction::North),
        ("south", Direction::South),
        ("east", Direction::East),
        ("west", Direction::West),
    ] {
        // Bottom and top are a slab's; the others a vertical slab's.
        let kind = if side.normal().y == 0 {
            "vertical_slab"
        } else {
            "slab"
        };
        for id in ids.map(|id| format!("{id}_{kind}")) {
            let state = slab(&id, half);
            assert_eq!(content.half(state), Some(side), "{id} {half}");
            assert_eq!(content.state(state).collision, [half_box(side)]);
        }
    }
    // Full blocks, plants, fluids and conduits aren't halves.
    for id in ["stone", "tuft", "water", "pipe", "lamp"] {
        assert_eq!(content.half(block_with(id, &[])), None, "{id}");
    }
}

#[test]
fn opposite_halves_combine_in_either_order_and_others_do_not() {
    let content = base();
    let (bottom, top) = (slab("stone_slab", "bottom"), slab("planks_slab", "top"));
    let both = content.combine(bottom, top).unwrap();
    assert_eq!(content.combine(top, bottom), Some(both));
    assert_eq!(content.halves(both), Some([bottom, top]));
    assert_eq!(content.block_of(both).id, COMBINED);
    assert_eq!(content.state(both).name, "Stone Slab and Oak Slab");
    // Each axis's pair, the low side first.
    let (north, south) = (
        slab("thatch_vertical_slab", "north"),
        slab("stone_vertical_slab", "south"),
    );
    assert_eq!(
        content.halves(content.combine(south, north).unwrap()),
        Some([north, south])
    );
    let (west, east) = (
        slab("planks_vertical_slab", "west"),
        slab("planks_vertical_slab", "east"),
    );
    assert!(content.combine(west, east).is_some());
    // Two of the same half, halves across axes, and full blocks don't.
    assert_eq!(content.combine(bottom, bottom), None);
    assert_eq!(content.combine(bottom, north), None);
    assert_eq!(content.combine(bottom, block_with("stone", &[])), None);
    assert_eq!(content.combine(bottom, BlockState::AIR), None);
    // A combined state is no half to combine again.
    assert_eq!(content.half(both), None);
    assert_eq!(content.combine(both, top), None);
}

/// Two halves together fill the voxel: it hides its neighbours' faces,
/// stops light on every side and shades corners like a full block, has
/// no room for fluid, and keeps each half's box and outer faces, but
/// not the two faces between the halves.
#[test]
fn a_combined_state_is_a_full_block_made_of_both_halves() {
    let content = base();
    let (bottom, top) = (slab("stone_slab", "bottom"), slab("planks_slab", "top"));
    let both = content.state(content.combine(bottom, top).unwrap());
    let stone = content.state(block_with("stone", &[]));
    assert!(both.is_opaque());
    for side in Direction::ALL {
        assert!(both.occludes(side), "{side:?}");
        assert!(both.blocks_light(side), "{side:?}");
    }
    assert_eq!(both.fluid_room, 0);
    assert_eq!(
        both.collision,
        [half_box(Direction::Down), half_box(Direction::Up)]
    );
    // Five outer faces of each half: four half sides and its end.
    assert_eq!(both.faces.len(), 10);
    assert!(both.faces.iter().all(|face| face.cull.is_some()));
    let (a, b) = (content.state(bottom), content.state(top));
    for face in &both.faces {
        assert!(a.faces.contains(face) || b.faces.contains(face));
    }
    assert_eq!(both.weight, a.weight + b.weight);
    // Each side shows both halves' textures, so it never matches a
    // translucent neighbour's.
    assert_eq!(both.side_texture(Direction::North), SideTexture::Mixed);
    // Its neighbours' faces are hidden, as against stone.
    let stone_state = block_with("stone", &[]);
    let combined = content.combine(bottom, top).unwrap();
    for side in Direction::ALL {
        assert_eq!(
            content.hides(stone_state, combined, side),
            content.hides(stone_state, stone_state, side)
        );
    }
    assert!(stone.is_opaque());
}

/// Halves of the same texture side by side, or together in one voxel, line
/// up as a full block's faces would: each face shows the part of its tile
/// a cube's face would at the same place.
#[test]
fn halves_show_the_part_of_the_tile_a_cube_would() {
    let content = base();
    let cube = content.state(block_with("planks", &[]));
    let uv_on_cube = |pos: Vec3, normal: Vec3| -> Vec2 {
        // The cube's face with this normal maps positions to UVs linearly;
        // find it from its corners.
        let face = cube.faces.iter().find(|f| f.normal == normal).unwrap();
        let (v0, v1, v3) = (face.vertices[0], face.vertices[1], face.vertices[3]);
        let (du, dv) = (v1.pos - v0.pos, v3.pos - v0.pos);
        let d = pos - v0.pos;
        let (s, t) = (
            d.dot(du) / du.length_squared(),
            d.dot(dv) / dv.length_squared(),
        );
        v0.uv + (v1.uv - v0.uv) * s + (v3.uv - v0.uv) * t
    };
    for half in ["bottom", "top", "north", "south", "east", "west"] {
        let id = if matches!(half, "bottom" | "top") {
            "planks_slab"
        } else {
            "planks_vertical_slab"
        };
        let state = content.state(slab(id, half));
        for face in &state.faces {
            for vertex in &face.vertices {
                let want = uv_on_cube(vertex.pos, face.normal);
                assert!(
                    (vertex.uv - want).length() < 1e-5,
                    "{half}: {:?} at {:?} is {:?}, a cube's is {want:?}",
                    face.normal,
                    vertex.pos,
                    vertex.uv
                );
            }
        }
    }
}

#[test]
fn breaking_one_part_leaves_the_other() {
    let content = base();
    let (bottom, top) = (slab("stone_slab", "bottom"), slab("planks_slab", "top"));
    let both = content.combine(bottom, top).unwrap();
    assert_eq!(content.without(both, bottom), top);
    assert_eq!(content.without(both, top), bottom);
    assert_eq!(content.half_at(both, 0), bottom);
    assert_eq!(content.half_at(both, 1), top);
    // Anything else is the whole block.
    let stone = block_with("stone", &[]);
    assert_eq!(content.without(stone, stone), BlockState::AIR);
    assert_eq!(content.half_at(stone, 0), stone);
}

/// Placing a slab into a voxel with a half fills the other half, with the
/// placed slab's own material whatever half its item would pick.
#[test]
fn filling_a_half_keeps_the_placed_blocks_material() {
    let content = base();
    let bottom = slab("stone_slab", "bottom");
    let granite = block_with(
        "stone_slab",
        &[("rock", "defaria:granite"), ("half", "bottom")],
    );
    let (both, half) = content.fill_half(bottom, granite).unwrap();
    let granite_top = block_with(
        "stone_slab",
        &[("rock", "defaria:granite"), ("half", "top")],
    );
    assert_eq!(half, granite_top);
    assert_eq!(content.halves(both), Some([bottom, granite_top]));
    // A vertical slab in its voxel takes a vertical slab facing it, but
    // not a slab, which has no half on that side.
    let east = slab("planks_vertical_slab", "east");
    let granite_vertical = block_with(
        "stone_vertical_slab",
        &[("rock", "defaria:granite"), ("half", "north")],
    );
    let (_, west) = content.fill_half(east, granite_vertical).unwrap();
    assert_eq!(
        west,
        block_with(
            "stone_vertical_slab",
            &[("rock", "defaria:granite"), ("half", "west")]
        )
    );
    assert_eq!(content.fill_half(east, granite), None);
    assert_eq!(content.fill_half(bottom, granite_vertical), None);
    // Nothing fills a full block, and a full block fills nothing.
    assert_eq!(content.fill_half(block_with("stone", &[]), granite), None);
    assert_eq!(content.fill_half(bottom, block_with("stone", &[])), None);
}

/// A mod's own half-height blocks combine, with each other and with
/// themselves, with nothing declared; one that isn't a plain block (it has
/// a script) doesn't.
#[test]
fn any_plain_half_combines() {
    // The bottom half, picked by a smaller box.
    let small_target = include_str!("../../../assets/mods/defaria/models/slab_bottom.json5")
        .replacen(
            "  faces: [",
            "  target: [{ from: [0.25, 0, 0.25], to: [0.75, 0.5, 0.75] }],\n  faces: [",
            1,
        );
    let content = test_mod(&[
        (
            "models/bottom.json5",
            include_str!("../../../assets/mods/defaria/models/slab_bottom.json5"),
        ),
        (
            "models/top.json5",
            include_str!("../../../assets/mods/defaria/models/slab_top.json5"),
        ),
        (
            "blocks/half.json5",
            r#"{ model: "{half}", textures: { all: "a" },
                 properties: { half: ["top", "bottom"] } }"#,
        ),
        (
            "blocks/other.json5",
            r#"{ model: "bottom", textures: { all: "b" } }"#,
        ),
        (
            "blocks/replaceable.json5",
            r#"{ model: "bottom", textures: { all: "a" }, replaceable: true }"#,
        ),
        ("models/small_target.json5", &small_target),
        (
            "blocks/small_target.json5",
            r#"{ model: "small_target", textures: { all: "a" } }"#,
        ),
        (
            "blocks/scripted.json5",
            r#"{ model: "top", textures: { all: "a" }, script: "s" }"#,
        ),
        ("scripts/s.luau", "return {}"),
    ]);
    assert!(content.errors.is_empty(), "{:#?}", content.errors);
    let half = |values: &[(&str, &str)]| content.block_state("test:half", values).unwrap();
    let (top, bottom) = (half(&[("half", "top")]), half(&[("half", "bottom")]));
    let other = content.default_state("test:other").unwrap();
    assert!(content.combine(top, bottom).is_some());
    assert!(content.combine(top, other).is_some());
    let scripted = content.default_state("test:scripted").unwrap();
    assert_eq!(content.half(scripted), None);
    // Nor one placing replaces, or one picked by another box.
    for id in ["test:replaceable", "test:small_target"] {
        assert_eq!(
            content.half(content.default_state(id).unwrap()),
            None,
            "{id}"
        );
    }
    assert_eq!(content.combine(scripted, bottom), None);
}

/// A half filled in keeps the properties placing picked for it (here a
/// `facing` from the view), not just its item's.
#[test]
fn filling_a_half_keeps_what_placing_picked() {
    let content = test_mod(&[
        (
            "models/bottom.json5",
            include_str!("../../../assets/mods/defaria/models/slab_bottom.json5"),
        ),
        (
            "models/top.json5",
            include_str!("../../../assets/mods/defaria/models/slab_top.json5"),
        ),
        (
            "blocks/half.json5",
            r#"{ model: "{half}", textures: { all: "a" }, properties: {
                 half: { values: ["bottom", "top"], place: { from: "face", values: { up: "bottom", down: "top" } } },
                 facing: { values: ["north", "south"], place: "look" } } }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{:#?}", content.errors);
    let state = |half, facing| {
        content
            .block_state("test:half", &[("half", half), ("facing", facing)])
            .unwrap()
    };
    let existing = state("bottom", "north");
    let placing = content.placed(existing, Direction::Up, Vec3::Z);
    assert_eq!(placing, state("bottom", "south"));
    let (_, half) = content.fill_half(existing, placing).unwrap();
    assert_eq!(half, state("top", "south"));
}
