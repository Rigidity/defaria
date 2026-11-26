use super::*;
use crate::content::{Direction, FULL_FLUID, Render, base, block, grass, sand, test_mod};
use bevy::mesh::{MeshVertexAttribute, VertexAttributeValues};

/// A completely full chunk: stone, topped with dirt, topped with grass.
fn filled() -> Chunk {
    let mut chunk = Chunk::default();
    for (x, y, z) in Chunk::positions() {
        let state = match y {
            y if y == CHUNK_SIZE - 1 => grass(),
            y if y >= CHUNK_SIZE - 4 => block("soil"),
            _ => block("stone"),
        };
        chunk.set(x, y, z, state);
    }
    chunk
}

fn air(_: IVec3) -> BlockState {
    BlockState::AIR
}

/// Meshes `chunk` with `outside` giving the voxels around it, all in full
/// sky light.
fn chunk_mesh(chunk: &Chunk, content: &Content, outside: impl Fn(IVec3) -> BlockState) -> Mesh {
    let inside = |pos: IVec3| pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(16)).all();
    Neighborhood::from_fn(|pos| {
        let block = if inside(pos) {
            chunk.at(pos)
        } else {
            outside(pos)
        };
        (block, OPEN_SKY)
    })
    .mesh(content)
}

fn mesh(chunk: &Chunk, outside: impl Fn(IVec3) -> BlockState) -> Mesh {
    chunk_mesh(chunk, base(), outside)
}

fn faces(mesh: &Mesh) -> usize {
    assert_eq!(mesh.indices().unwrap().len(), mesh.count_vertices() / 4 * 6);
    mesh.count_vertices() / 4
}

#[test]
fn lone_voxel_gets_six_faces() {
    let mut chunk = Chunk::default();
    chunk.set(5, 5, 5, block("soil"));
    assert_eq!(faces(&mesh(&chunk, air)), 6);
}

#[test]
fn faces_between_solid_voxels_are_culled() {
    let mut chunk = Chunk::default();
    chunk.set(5, 5, 5, block("soil"));
    chunk.set(6, 5, 5, block("stone"));
    assert_eq!(faces(&mesh(&chunk, air)), 10);
}

/// A slab on a block hides the block's top and its own bottom; a block
/// beside it keeps its face against the slab, whose upper half shows,
/// and the slab's side against the block is hidden.
#[test]
fn slabs_hide_only_what_they_cover() {
    let mut chunk = Chunk::default();
    chunk.set(5, 5, 5, block("stone"));
    chunk.set(5, 6, 5, block("stone_slab"));
    assert_eq!(faces(&mesh(&chunk, air)), 5 + 5);
    chunk.set(6, 6, 5, block("stone"));
    // The new block: all 6 faces; the slab loses its east side.
    assert_eq!(faces(&mesh(&chunk, air)), 5 + 4 + 6);

    let mesh = mesh(&chunk, air);
    let positions = vec3s(&mesh, Mesh::ATTRIBUTE_POSITION);
    let top = positions
        .chunks(4)
        .find(|quad| quad.iter().all(|p| p.y == 6.5));
    assert!(top.is_some(), "the slab's top is at half height");
}

#[test]
fn filled_chunk_shows_only_its_surface() {
    let side = CHUNK_SIZE * CHUNK_SIZE;
    assert_eq!(faces(&mesh(&filled(), air)), 6 * side);
    // Solid all around: nothing is visible.
    assert_eq!(faces(&mesh(&filled(), |_| block("stone"))), 0);
    // Solid below only (like a chunk stacked on another): the bottom goes.
    let below = |pos: IVec3| {
        if pos.y < 0 {
            block("stone")
        } else {
            BlockState::AIR
        }
    };
    assert_eq!(faces(&mesh(&filled(), below)), 5 * side);
}

/// A neighbourhood copies each neighbouring chunk's edge from that
/// chunk's own coordinates, and reads missing chunks as open sky.
#[test]
fn gather_copies_the_edges_of_the_chunks_around() {
    let mut middle = Chunk::default();
    middle.set(0, 0, 0, block("soil"));
    let mut west = Chunk::default();
    west.set(15, 0, 0, block("stone"));
    let mut corner = Chunk::default();
    corner.set(0, 15, 15, sand());
    let mut light = ChunkLight::default();
    light.set_at(IVec3::new(15, 0, 0), Light::new(3, [4, 5, 6]));
    let dark = ChunkLight::default();
    let n = Neighborhood::gather(|offset| match offset.to_array() {
        [0, 0, 0] => Some((&middle, &dark)),
        [-1, 0, 0] => Some((&west, &light)),
        [1, -1, -1] => Some((&corner, &dark)),
        _ => None,
    });
    assert_eq!(n.block(IVec3::ZERO), block("soil"));
    assert_eq!(n.light(IVec3::ZERO), Light::DARK);
    assert_eq!(n.block(IVec3::new(-1, 0, 0)), block("stone"));
    assert_eq!(n.light(IVec3::new(-1, 0, 0)), Light::new(3, [4, 5, 6]));
    assert_eq!(n.block(IVec3::new(16, -1, -1)), sand());
    assert_eq!(n.block(IVec3::new(16, 16, 16)), BlockState::AIR);
    assert_eq!(n.light(IVec3::new(16, 16, 16)), OPEN_SKY);
}

#[test]
fn empty_chunk_has_empty_mesh() {
    let mesh = mesh(&Chunk::default(), air);
    assert_eq!(mesh.count_vertices(), 0);
    assert_eq!(mesh.indices().unwrap().len(), 0);
}

fn vec3s(mesh: &Mesh, attribute: MeshVertexAttribute) -> Vec<Vec3> {
    let Some(VertexAttributeValues::Float32x3(values)) = mesh.attribute(attribute) else {
        panic!("missing {attribute:?}");
    };
    values.iter().map(|&v| Vec3::from(v)).collect()
}

fn vec2s(mesh: &Mesh, attribute: MeshVertexAttribute) -> Vec<Vec2> {
    let Some(VertexAttributeValues::Float32x2(values)) = mesh.attribute(attribute) else {
        panic!("missing {attribute:?}");
    };
    values.iter().map(|&v| Vec2::from(v)).collect()
}

#[test]
fn single_voxel_spans_its_unit_cube() {
    let mut chunk = Chunk::default();
    chunk.set(2, 3, 4, block("stone"));
    let positions = vec3s(&mesh(&chunk, air), Mesh::ATTRIBUTE_POSITION);
    let (min, max) = positions
        .iter()
        .fold((Vec3::MAX, Vec3::MIN), |(min, max), &p| {
            (min.min(p), max.max(p))
        });
    assert_eq!(min, Vec3::new(2.0, 3.0, 4.0));
    assert_eq!(max, Vec3::new(3.0, 4.0, 5.0));
}

/// The cube model's faces: one per side, each culled against its own
/// side, with corners starting at the bottom left of the texture and
/// running counter-clockwise from outside, so the texture is neither
/// mirrored nor rotated and side faces have their top edge at +Y.
#[test]
fn cube_faces_start_bottom_left_and_go_round() {
    let stone = base().state(block("stone"));
    let mut sides: Vec<Direction> = stone.faces.iter().filter_map(|f| f.cull).collect();
    sides.sort_by_key(|d| *d as u8);
    assert_eq!(sides, Direction::ALL);
    for face in &stone.faces {
        let side = face.cull.unwrap();
        assert_eq!(face.normal, side.normal().as_vec3());
        assert!(stone.occludes(side));
        let [bl, br, tr, tl] = [0, 1, 2, 3].map(|i| face.vertices[i].pos);
        let (right, up) = (br - bl, tl - bl);
        assert_eq!(right.cross(up), face.normal);
        assert_eq!(tr, bl + right + up);
        if face.normal.y == 0.0 {
            assert_eq!(up, Vec3::Y, "side face {side:?} isn't upright");
        }
        let uvs = face.vertices.iter().map(|v| v.uv).collect::<Vec<_>>();
        // `v` points down the image, so its top row is the face's top.
        assert_eq!(uvs, [Vec2::Y, Vec2::ONE, Vec2::X, Vec2::ZERO]);
    }
}

/// Each face's winding must point the same way as its normal, otherwise
/// backface culling hides the outside of the cube.
#[test]
fn faces_wind_counter_clockwise_around_their_normal() {
    for name in ["stone", "log"] {
        for state in [block(name), BlockState(block(name).0 + 1)] {
            for face in &base().state(state).faces {
                let [a, b, c, d] = [0, 1, 2, 3].map(|i| face.vertices[i].pos);
                for (p, q, r) in [(a, b, c), (a, c, d)] {
                    let winding = (q - p).cross(r - p).normalize();
                    assert_eq!(winding, face.normal);
                }
            }
        }
    }
}

/// The tile on each side, by the side's normal.
fn tiles(state: BlockState) -> Vec<(IVec3, String)> {
    let content = base();
    let mut tiles: Vec<_> = content
        .state(state)
        .faces
        .iter()
        .map(|face| {
            let id = &content.textures[face.layer as usize].template;
            (
                face.normal.as_ivec3(),
                id.trim_start_matches("defaria:").to_owned(),
            )
        })
        .collect();
    tiles.sort_by_key(|(normal, _)| normal.to_array());
    tiles
}

fn tile_on(state: BlockState, normal: IVec3) -> String {
    tiles(state)
        .into_iter()
        .find(|(n, _)| *n == normal)
        .unwrap()
        .1
}

#[test]
fn grass_has_a_top_sides_and_a_dirt_bottom() {
    let grass = grass();
    assert_eq!(tile_on(grass, IVec3::Y), "turf");
    assert_eq!(tile_on(grass, IVec3::NEG_Y), "soil");
    for side in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
        assert_eq!(tile_on(grass, side), "soil_turf");
    }
    assert_eq!(tile_on(block("stone"), IVec3::Y), "stone");
}

/// A log has rings on the ends of its axis and bark around it.
#[test]
fn logs_show_their_rings_along_their_axis() {
    let log = base().block("defaria:log").unwrap();
    for (axis, normal) in [("y", IVec3::Y), ("x", IVec3::X), ("z", IVec3::Z)] {
        let state = log.state(&[("axis", axis)]).unwrap();
        for (side, tile) in tiles(state) {
            let expected = if side.abs() == normal {
                "log_top"
            } else {
                "log_side"
            };
            assert_eq!(tile, expected, "axis {axis}, side {side}");
        }
    }
}

#[test]
fn each_face_samples_its_own_tile_layer() {
    let mut chunk = Chunk::default();
    chunk.set(1, 1, 1, grass());
    let mesh = mesh(&chunk, air);
    let normals = vec3s(&mesh, Mesh::ATTRIBUTE_NORMAL);
    let content = base();
    for (normal, face) in normals.iter().zip(faces_of(&mesh)) {
        let tile = tile_on(grass(), normal.as_ivec3());
        let template = &content.textures[face[0] as usize].template;
        assert_eq!(*template, format!("defaria:{tile}"));
    }
}

/// Mod `test` plus a `wall` that covers every side but draws nothing, a
/// `lid` ([`crate::content::QUAD`]: a top face, covering only its top),
/// a `pair` (a top and a bottom face with no `cull`) and a one-triangle
/// `tri`.
fn shapes() -> Content {
    let content = test_mod(&[
        (
            "models/wall.json5",
            r#"{ slots: { all: null }, occludes: ["up", "down", "north", "south", "east", "west"], faces: [] }"#,
        ),
        (
            "models/tri.json5",
            r#"{ slots: { all: null }, faces: [{ texture: "all", cull: "up", vertices: [
                { pos: [0, 1, 1], uv: [0, 1] }, { pos: [1, 1, 1], uv: [1, 1] }, { pos: [1, 1, 0], uv: [1, 0] },
            ] }] }"#,
        ),
        (
            "blocks/wall.json5",
            r#"{ model: "wall", textures: { all: "a" } }"#,
        ),
        (
            "blocks/lid.json5",
            r#"{ model: "quad", textures: { all: "a" } }"#,
        ),
        (
            "blocks/pair.json5",
            r#"{ model: "pair", textures: { top: "a", bottom: "a" } }"#,
        ),
        (
            "blocks/tri.json5",
            r#"{ model: "tri", textures: { all: "a" } }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    content
}

fn shape(content: &Content, name: &str) -> BlockState {
    content.default_state(&format!("test:{name}")).unwrap()
}

/// A face is hidden by what its neighbour covers on the side facing it,
/// not by what the neighbour covers on the far side.
#[test]
fn faces_are_hidden_by_the_side_of_the_neighbour_facing_them() {
    let content = shapes();
    let (lid, wall) = (shape(&content, "lid"), shape(&content, "wall"));
    // A lid under a lid: the upper one covers only its top, not the
    // bottom facing the lower lid's face, so both faces show.
    let mut chunk = Chunk::default();
    chunk.set(1, 1, 1, lid);
    chunk.set(1, 2, 1, lid);
    assert_eq!(chunk_mesh(&chunk, &content, air).count_vertices(), 8);
    // A wall covers its bottom, so the lid's face under it goes.
    chunk.set(1, 2, 1, wall);
    assert_eq!(chunk_mesh(&chunk, &content, air).count_vertices(), 0);
    // A lid on the far side of the chunk border, under a wall outside it.
    let mut edge = Chunk::default();
    edge.set(1, CHUNK_SIZE - 1, 1, lid);
    assert_eq!(chunk_mesh(&edge, &content, |_| wall).count_vertices(), 0);
    assert_eq!(chunk_mesh(&edge, &content, air).count_vertices(), 4);
}

/// Faces with no `cull` side draw even when buried.
#[test]
fn faces_without_a_cull_side_always_draw() {
    let content = shapes();
    let mut chunk = Chunk::default();
    chunk.set(1, 1, 1, shape(&content, "pair"));
    let wall = |_| shape(&content, "wall");
    let mesh = chunk_mesh(&chunk, &content, wall);
    assert_eq!(mesh.count_vertices(), 8);
    for (x, y, z) in [
        (0, 1, 1),
        (2, 1, 1),
        (1, 0, 1),
        (1, 2, 1),
        (1, 1, 0),
        (1, 1, 2),
    ] {
        chunk.set(x, y, z, shape(&content, "wall"));
    }
    assert_eq!(chunk_mesh(&chunk, &content, air).count_vertices(), 8);
}

/// A three-vertex face is one triangle: three vertices, three indices.
#[test]
fn triangles_mesh_as_one_triangle() {
    let content = shapes();
    let mut chunk = Chunk::default();
    chunk.set(2, 2, 2, shape(&content, "tri"));
    chunk.set(4, 2, 2, shape(&content, "lid"));
    let mesh = chunk_mesh(&chunk, &content, air);
    assert_eq!(mesh.count_vertices(), 3 + 4);
    let Some(Indices::U32(indices)) = mesh.indices() else {
        panic!("no indices");
    };
    assert_eq!(indices, &[0, 1, 2, 3, 4, 5, 3, 5, 6]);
}

/// One face's shading, read back from the mesh: each corner's ambient
/// occlusion and light (sky, red, green, blue, 255 at level 15), in
/// corner order.
#[derive(Debug, PartialEq)]
struct Shading {
    occlusion: [u8; 4],
    light: [[u8; 4]; 4],
}

fn faces_of(mesh: &Mesh) -> Vec<[u32; 2]> {
    let Some(VertexAttributeValues::Uint32x2(faces)) = mesh.attribute(ATTRIBUTE_FACE) else {
        panic!("no face attribute");
    };
    faces.clone()
}

/// The shading of the face of block `pos` facing `normal`.
fn shading(mesh: &Mesh, pos: IVec3, normal: IVec3) -> Shading {
    let positions = vec3s(mesh, Mesh::ATTRIBUTE_POSITION);
    let normals = vec3s(mesh, Mesh::ATTRIBUTE_NORMAL);
    let Some(VertexAttributeValues::Uint32x4(lights)) = mesh.attribute(ATTRIBUTE_LIGHT) else {
        panic!("no light attribute");
    };
    let faces = faces_of(mesh);
    let center = pos.as_vec3() + Vec3::splat(0.5) + normal.as_vec3() * 0.5;
    let first = (0..positions.len())
        .step_by(4)
        .find(|&i| {
            let middle = (positions[i] + positions[i + 2]) / 2.0;
            normals[i] == normal.as_vec3() && middle.distance(center) < 1e-4
        })
        .unwrap_or_else(|| panic!("no face of {pos} facing {normal}"));
    // Every vertex of a face carries the same corners.
    for i in first..first + 4 {
        assert_eq!((lights[i], faces[i]), (lights[first], faces[first]));
    }
    Shading {
        occlusion: faces[first][1].to_le_bytes(),
        light: lights[first].map(u32::to_le_bytes),
    }
}

fn neighborhood(blocks: &[(IVec3, BlockState)], light: impl Fn(IVec3) -> Light) -> Neighborhood {
    Neighborhood::from_fn(|pos| {
        let block = blocks
            .iter()
            .find(|(p, _)| *p == pos)
            .map_or(BlockState::AIR, |(_, b)| *b);
        (block, light(pos))
    })
}

const FULL: [u8; 4] = [255, 0, 0, 0];

#[test]
fn open_faces_have_no_occlusion() {
    let pos = IVec3::new(5, 5, 5);
    let mesh = neighborhood(&[(pos, block("stone"))], |_| OPEN_SKY).mesh(base());
    for normal in Direction::ALL.map(Direction::normal) {
        let shading = shading(&mesh, pos, normal);
        assert_eq!(shading.occlusion, [OPEN; 4], "{normal}");
        assert_eq!(shading.light, [FULL; 4]);
    }
}

/// A block beside the face's front voxel darkens the two corners on that
/// side; one only diagonally across darkens just its corner; two on both
/// sides of a corner close it completely.
#[test]
fn neighbours_occlude_the_corners_they_touch() {
    let stone = block("stone");
    let pos = IVec3::new(5, 5, 5);
    let up = IVec3::Y;
    // The top face's corners run (0,1,1), (1,1,1), (1,1,0), (0,1,0).
    let top = |others: &[IVec3]| {
        let mut blocks = vec![(pos, stone)];
        blocks.extend(others.iter().map(|&o| (pos + up + o, stone)));
        shading(&neighborhood(&blocks, |_| OPEN_SKY).mesh(base()), pos, up).occlusion
    };
    assert_eq!(top(&[IVec3::X]), [3, 2, 2, 3], "east edge");
    assert_eq!(
        top(&[IVec3::new(1, 0, 1)]),
        [3, 2, 3, 3],
        "south-east diagonal"
    );
    assert_eq!(top(&[IVec3::X, IVec3::Z]), [2, 0, 2, 3], "east and south");
    assert_eq!(
        top(&[IVec3::X, IVec3::Z, IVec3::new(1, 0, 1)]),
        [2, 0, 2, 3],
        "the diagonal doesn't matter once both edges are blocked"
    );
}

/// A corner's light is the average of the voxels around it that aren't
/// opaque.
#[test]
fn corner_light_averages_the_open_voxels_around_it() {
    let stone = block("stone");
    let pos = IVec3::new(5, 5, 5);
    // Sky 15 on the east half of the layer above, 7 on the west half;
    // a block north-west above blocks a voxel, whose light is ignored.
    let light = |p: IVec3| {
        let sky = if p.x > pos.x { 15 } else { 7 };
        Light::new(sky, [0, 0, 0])
    };
    let blocks = [(pos, stone), (pos + IVec3::new(-1, 1, -1), stone)];
    let shading = shading(&neighborhood(&blocks, light).mesh(base()), pos, IVec3::Y);
    let level = |avg: f32| (avg * LEVEL_STEP).round() as u8;
    // Corners (0,1,1) and (0,1,0) see only west voxels; (1,1,*) see two
    // of each. (0,1,0) has its diagonal blocked, leaving three.
    assert_eq!(
        shading.light.map(|c| c[0]),
        [level(7.0), level(11.0), level(11.0), level(7.0)]
    );
    assert_eq!(shading.occlusion, [3, 3, 3, 2]);
}

/// With both edges of a corner blocked, light can't get round to it, so
/// the diagonal voxel's light is left out even when it's open.
#[test]
fn closed_corners_ignore_the_diagonal() {
    let stone = block("stone");
    let pos = IVec3::new(5, 5, 5);
    // Bright only at the open diagonal above the (1, 1, 1) corner.
    let diagonal = pos + IVec3::new(1, 1, 1);
    let light = |p: IVec3| Light::new(if p == diagonal { 15 } else { 3 }, [0; 3]);
    let blocks = [
        (pos, stone),
        (pos + IVec3::new(1, 1, 0), stone),
        (pos + IVec3::new(0, 1, 1), stone),
    ];
    let shading = shading(&neighborhood(&blocks, light).mesh(base()), pos, IVec3::Y);
    let level = |avg: f32| (avg * LEVEL_STEP).round() as u8;
    assert_eq!(shading.occlusion[1], 0, "corner (1, 1, 1) is closed");
    assert_eq!(
        shading.light[1][0],
        level(3.0),
        "only the front voxel counts"
    );
}

/// Block light averages per colour channel, like the sky's.
#[test]
fn coloured_light_averages_per_channel() {
    let pos = IVec3::new(5, 5, 5);
    let light = |_| Light::new(0, [15, 3, 0]);
    let shading = shading(
        &neighborhood(&[(pos, block("stone"))], light).mesh(base()),
        pos,
        IVec3::X,
    );
    assert_eq!(shading.light, [[0, 255, 51, 0]; 4]);
}

/// The faces of a block that gives off light are drawn fully lit.
#[test]
fn light_sources_glow() {
    let pos = IVec3::new(5, 5, 5);
    let blocks = [
        (pos, block("red_lamp")),
        (pos + IVec3::new(1, 1, 0), block("stone")),
    ];
    let shading = shading(
        &neighborhood(&blocks, |_| Light::DARK).mesh(base()),
        pos,
        IVec3::Y,
    );
    assert_eq!(shading.occlusion, [OPEN; 4]);
    assert_eq!(shading.light, [[0, 255, 255, 255]; 4]);
}

/// A triangle's three corners are its own; the made-up fourth keeps the
/// blend flat across the triangle.
#[test]
fn triangles_extend_their_shading_to_a_fourth_corner() {
    let a = CornerShading {
        light: [10, 20, 30, 40],
        occlusion: 3,
    };
    let b = CornerShading {
        light: [20, 20, 250, 0],
        occlusion: 1,
    };
    let c = CornerShading {
        light: [30, 20, 10, 0],
        occlusion: 3,
    };
    let d = b.extend(c, a);
    assert_eq!(d.light, [40, 20, 230, 0]);
    assert_eq!(d.occlusion, 1);
    let mesh = shapes();
    let mut chunk = Chunk::default();
    chunk.set(2, 2, 2, shape(&mesh, "tri"));
    let corners: Vec<Vec2> = vec2s(&chunk_mesh(&chunk, &mesh, air), ATTRIBUTE_CORNER);
    assert_eq!(corners, [Vec2::ZERO, Vec2::X, Vec2::Y]);

    // In a mesh, with light differing at each corner: the third packed
    // corner is the made-up one.
    let pos = IVec3::new(2, 2, 2);
    let light = |p: IVec3| Light::new((p.x * 3 + p.z) as u8, [0; 3]);
    let tri = shape(&mesh, "tri");
    let built = Neighborhood::from_fn(|p| (if p == pos { tri } else { BlockState::AIR }, light(p)))
        .mesh(&mesh);
    let Some(VertexAttributeValues::Uint32x4(lights)) = built.attribute(ATTRIBUTE_LIGHT) else {
        panic!("no light attribute");
    };
    let sky = lights[0].map(|corner| corner.to_le_bytes()[0] as i32);
    assert_ne!(sky[0], sky[1]);
    assert_eq!(sky[2], (sky[1] + sky[3] - sky[0]).clamp(0, 255));
}

/// Faces per mesh, opaque, cut-out and translucent, of `blocks` (by
/// name, at chunk-local positions) in open air.
fn split(blocks: &[(IVec3, &str)]) -> [usize; 3] {
    let blocks: Vec<_> = blocks.iter().map(|&(p, b)| (p, block(b))).collect();
    let meshes = neighborhood(&blocks, |_| OPEN_SKY).meshes(base());
    [Render::Opaque, Render::Cutout, Render::Translucent].map(|r| faces(meshes.get(r)))
}

const A: IVec3 = IVec3::new(5, 5, 5);
const B: IVec3 = IVec3::new(6, 5, 5);

/// Each block's faces go in the mesh for how it's drawn. Water is
/// double-sided: each face twice, front and back; glass isn't.
#[test]
fn faces_go_in_the_mesh_for_their_render() {
    assert_eq!(split(&[(A, "stone")]), [6, 0, 0]);
    assert_eq!(split(&[(A, "leaves")]), [0, 6, 0]);
    assert_eq!(split(&[(A, "glass")]), [0, 0, 6]);
    assert_eq!(split(&[(A, "water")]), [0, 0, 12]);
    // Lava glows, but it's opaque; double-sided like water.
    assert_eq!(split(&[(A, "lava")]), [12, 0, 0]);
}

/// You can be inside a fluid, so even opaque lava doesn't hide a solid
/// opaque block's face: the floor and walls around a lava pool are
/// still drawn, and seen from inside it the world doesn't show through
/// them. Against anything else, only one face is drawn where they
/// meet, so the two don't fight over the same plane: lava's (double
/// sided, 12 faces alone).
#[test]
fn fluids_keep_solid_faces_and_draw_one_face_elsewhere() {
    // Stone's face against the lava stays; the lava's against the
    // stone goes.
    assert_eq!(split(&[(A, "stone"), (B, "lava")]), [6 + 10, 0, 0]);
    assert_eq!(split(&[(A, "lava"), (B, "lava")]), [20, 0, 0]);
    assert_eq!(split(&[(A, "stone"), (B, "water")]), [6, 0, 10]);
    // Water, glass and leaves lose their face against lava.
    assert_eq!(split(&[(A, "water"), (B, "lava")]), [12, 0, 10]);
    assert_eq!(split(&[(A, "glass"), (B, "lava")]), [12, 0, 5]);
    assert_eq!(split(&[(A, "leaves"), (B, "lava")]), [12, 5, 0]);
}

/// The translucent mesh of these blocks, each with the amount of fluid
/// kept for it.
fn fluid_mesh(blocks: &[(IVec3, &str, Option<u16>)]) -> Mesh {
    Neighborhood::with_fluid(|pos| {
        blocks
            .iter()
            .find(|(p, _, _)| *p == pos)
            .map_or((BlockState::AIR, OPEN_SKY, None), |&(_, b, kept)| {
                (block(b), OPEN_SKY, kept)
            })
    })
    .meshes(base())
    .translucent
}

fn positions(mesh: &Mesh) -> Vec<Vec3> {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        panic!("no positions");
    };
    positions.iter().map(|&p| Vec3::from(p)).collect()
}

/// Fluid that isn't full is drawn at the next of 16 heights up: its top
/// lowered, and its sides cut short, their texture cut rather than
/// squashed.
#[test]
fn fluid_is_drawn_at_its_height() {
    let half = fluid_mesh(&[(A, "water", Some(FULL_FLUID / 2))]);
    assert_eq!(faces(&half), 12, "six faces, front and back");
    let top = positions(&half).iter().map(|p| p.y).fold(0.0, f32::max);
    assert_eq!(top, A.y as f32 + 0.5);
    // A sliver still shows, a sixteenth high.
    let sliver = fluid_mesh(&[(A, "water", Some(1))]);
    let top = positions(&sliver).iter().map(|p| p.y).fold(0.0, f32::max);
    assert_eq!(top, A.y as f32 + 1.0 / 16.0);
    // The sides' texture runs from the middle of the tile down.
    let Some(VertexAttributeValues::Float32x2(uvs)) = half.attribute(Mesh::ATTRIBUTE_UV_0) else {
        panic!("no uvs");
    };
    let mut vs: Vec<f32> = uvs.iter().map(|uv| uv[1]).collect();
    vs.sort_by(f32::total_cmp);
    vs.dedup();
    assert_eq!(vs, [0.0, 0.5, 1.0]);
}

/// Between two voxels of the same fluid, only the higher one's side
/// shows, from the lower one's height up.
#[test]
fn fluid_shows_its_side_above_lower_fluid() {
    let mesh = fluid_mesh(&[(A, "water", None), (B, "water", Some(FULL_FLUID / 4))]);
    // A: top, bottom, three sides and the band over B; B: five faces.
    assert_eq!(faces(&mesh), 2 * 11);
    let band: Vec<Vec3> = positions(&mesh)
        .into_iter()
        .filter(|p| p.x == B.x as f32)
        .filter(|p| p.y > A.y as f32 + 0.25)
        .collect();
    assert!(!band.is_empty());
    let level = fluid_mesh(&[(A, "water", Some(490)), (B, "water", Some(500))]);
    assert_eq!(faces(&level), 2 * 10, "drawn as high: no face between them");
}

/// Meshing reads the fluid amounts from the chunks themselves, across
/// a chunk border too: half a block of water against a quarter in the
/// next chunk shows its side above the quarter.
#[test]
fn fluid_heights_come_from_the_chunks_across_borders() {
    let (here, there) = (IVec3::new(15, 5, 5), IVec3::new(0, 5, 5));
    let mut middle = Chunk::default();
    middle.set_at(here, block("water"));
    middle.set_fluid_at(here, FULL_FLUID / 2);
    let mut east = Chunk::default();
    east.set_at(there, block("water"));
    east.set_fluid_at(there, FULL_FLUID / 4);
    let light = ChunkLight::default();
    let meshes = Neighborhood::gather(|offset| match offset {
        IVec3::ZERO => Some((&middle, &light)),
        IVec3::X => Some((&east, &light)),
        _ => None,
    })
    .meshes(base());
    let positions = positions(&meshes.translucent);
    let top = positions.iter().map(|p| p.y).fold(0.0, f32::max);
    assert_eq!(top, 5.5);
    // The side against the east chunk starts a quarter up: only that
    // band has corners there.
    let band = positions
        .iter()
        .filter(|p| p.x == 16.0 && p.y == 5.25)
        .count();
    assert_eq!(band, 4, "front and back, two corners each");
}

/// A sliver of water on a pond's surface is drawn on top of it, and the
/// pond's top stays hidden under it, so the surface has no hole.
#[test]
fn a_sliver_on_a_surface_leaves_no_hole() {
    let mesh = fluid_mesh(&[(A, "water", None), (A + IVec3::Y, "water", Some(1))]);
    // Below: bottom and four sides; above: top and four sides.
    assert_eq!(faces(&mesh), 2 * 10);
    let top = positions(&mesh).iter().map(|p| p.y).fold(0.0, f32::max);
    assert_eq!(top, A.y as f32 + 1.0 + 1.0 / 16.0);
}

/// Fluid with the same fluid above it is drawn full height, so a falling
/// column has no seams, and has no top.
#[test]
fn fluid_under_fluid_is_full_height() {
    let column = fluid_mesh(&[(A, "water", Some(100)), (A + IVec3::Y, "water", Some(100))]);
    assert_eq!(faces(&column), 2 * 10);
    let lower_sides_top = positions(&column)
        .into_iter()
        .filter(|p| p.y <= A.y as f32 + 1.0)
        .map(|p| p.y)
        .fold(0.0, f32::max);
    assert_eq!(lower_sides_top, A.y as f32 + 1.0);
}

/// A full block over fluid hides its top only if the fluid is full: a
/// lowered top lies inside the voxel, below the block.
#[test]
fn a_block_over_fluid_hides_only_a_full_top() {
    let stone = (A + IVec3::Y, "stone", None);
    assert_eq!(faces(&fluid_mesh(&[(A, "water", None), stone])), 2 * 5);
    assert_eq!(faces(&fluid_mesh(&[(A, "water", Some(500)), stone])), 2 * 6);
}

/// Nothing is hidden next to a cut-out block, not even another one:
/// its holes show what's behind. An opaque block still hides a
/// cut-out's face against it.
#[test]
fn faces_next_to_cut_outs_are_not_culled() {
    assert_eq!(split(&[(A, "leaves"), (B, "leaves")]), [0, 12, 0]);
    assert_eq!(split(&[(A, "stone"), (B, "leaves")]), [6, 5, 0]);
}

/// Faces between two of the same translucent block are hidden, so
/// glass walls and water look like one body; against anything else
/// both faces show.
#[test]
fn faces_between_the_same_translucent_block_are_culled() {
    // Water is double-sided, so two faces each.
    assert_eq!(split(&[(A, "glass"), (B, "glass")]), [0, 0, 10]);
    assert_eq!(split(&[(A, "water"), (B, "water")]), [0, 0, 20]);
    assert_eq!(split(&[(A, "glass"), (B, "water")]), [0, 0, 6 + 12]);
    assert_eq!(split(&[(A, "stone"), (B, "glass")]), [6, 0, 5]);
    assert_eq!(split(&[(A, "leaves"), (B, "glass")]), [0, 6, 6]);
}

/// See-through blocks cast no ambient occlusion and don't hide light
/// from a corner: a floor beside and under them is shaded exactly as
/// it would be with nothing there. An opaque block in the same places
/// does shade it.
#[test]
fn see_through_blocks_cast_no_shadow_on_corners() {
    let floor: Vec<_> = (4..7)
        .flat_map(|x| (4..7).map(move |z| (IVec3::new(x, 4, z), block("stone"))))
        .collect();
    let light = |p: IVec3| Light::new((p.x + 2 * p.z + p.y) as u8 % 16, [0; 3]);
    let top = |extra: &[(IVec3, BlockState)]| {
        let blocks: Vec<_> = floor.iter().chain(extra).copied().collect();
        let mesh = neighborhood(&blocks, light).meshes(base()).opaque;
        shading(&mesh, A - IVec3::Y, IVec3::Y)
    };
    let bare = top(&[]);
    for name in ["leaves", "glass", "water", "tuft"] {
        let around = [A, A + IVec3::X, A + IVec3::Z, A + IVec3::ONE.with_y(0)];
        let around = around.map(|p| (p, block(name)));
        assert_eq!(top(&around), bare, "{name} over and beside");
        assert!(!base().state(block(name)).is_opaque(), "{name}");
    }
    let stone = [(A + IVec3::X, block("stone"))];
    assert_ne!(top(&stone).occlusion, bare.occlusion);
}

/// A plant is four faces inside its block, lit by its own voxel, and
/// it covers no side, so it hides nothing around it and nothing hides
/// it.
#[test]
fn plants_are_crossed_quads_that_hide_nothing() {
    assert_eq!(split(&[(A, "tuft")]), [0, 4, 0]);
    assert_eq!(split(&[(A, "tuft"), (B, "stone")]), [6, 4, 0]);
    let below = A - IVec3::Y;
    assert_eq!(split(&[(A, "tuft"), (below, "soil")]), [6, 4, 0]);
    let blocks = [(A, block("tuft"))];
    let light = |p: IVec3| Light::new(if p == A { 9 } else { 15 }, [0; 3]);
    let meshes = neighborhood(&blocks, light).meshes(base());
    let Some(VertexAttributeValues::Uint32x4(lights)) = meshes.cutout.attribute(ATTRIBUTE_LIGHT)
    else {
        panic!("no light attribute");
    };
    let nine = (9.0 * LEVEL_STEP).round() as u8;
    for light in lights {
        assert!(light.iter().all(|c| c.to_le_bytes()[0] == nine));
    }
}

/// A double-sided block's back faces are its front faces turned round:
/// the same corners, wound
/// the other way, facing the other way, each corner with the front's
/// light and occlusion. So the underside of water's surface is lit as
/// its top is.
#[test]
fn back_faces_mirror_the_front() {
    let blocks = [(A, block("water")), (A - IVec3::X, block("stone"))];
    let light = |p: IVec3| Light::new((p.x + 2 * p.z + 3 * p.y) as u8 % 16, [0; 3]);
    let meshes = neighborhood(&blocks, light).meshes(base());
    let mesh = &meshes.translucent;
    let positions = vec3s(mesh, Mesh::ATTRIBUTE_POSITION);
    let normals = vec3s(mesh, Mesh::ATTRIBUTE_NORMAL);
    let corners = vec2s(mesh, ATTRIBUTE_CORNER);
    let Some(VertexAttributeValues::Uint32x4(lights)) = mesh.attribute(ATTRIBUTE_LIGHT) else {
        panic!("no light attribute");
    };
    let faces = faces_of(mesh);
    // The light and occlusion each vertex gets, blended to its corner.
    let at = |i: usize| {
        let corner = CORNERS
            .iter()
            .position(|c| Vec2::from(*c) == corners[i])
            .unwrap();
        let occlusion = faces[i][1].to_le_bytes()[corner];
        (lights[i][corner], occlusion)
    };
    assert_eq!(positions.len(), 10 * 4, "five faces, front and back");
    for face in positions.chunks(8).enumerate().map(|(n, _)| n * 8) {
        let (front, back) = (face, face + 4);
        assert_eq!(normals[back], -normals[front]);
        let wound =
            |i: usize| (positions[i + 1] - positions[i]).cross(positions[i + 2] - positions[i]);
        assert!(wound(back).dot(normals[back]) > 0.0, "counter-clockwise");
        for i in 0..4 {
            let twin = (0..4)
                .map(|j| back + j)
                .find(|&j| positions[j] == positions[front + i])
                .expect("the back has the front's corners");
            assert_eq!(at(twin), at(front + i));
        }
    }
}

/// Faces that all lie in one plane, like open water's surface (front
/// and back), never need sorting; faces in two planes do.
#[test]
fn only_faces_in_several_planes_need_sorting() {
    let needs = |blocks: &[(IVec3, &str)]| {
        let blocks: Vec<_> = blocks.iter().map(|&(p, b)| (p, block(b))).collect();
        // Stone all around below and beside, so only tops show.
        let ground = |p: IVec3| {
            let found = blocks.iter().find(|(q, _)| *q == p).map(|(_, b)| *b);
            let state = found.unwrap_or(if p.y < 5 || p.x < 2 || p.x > 6 || p.z < 2 || p.z > 6 {
                block("stone")
            } else {
                BlockState::AIR
            });
            (state, OPEN_SKY)
        };
        Neighborhood::from_fn(ground)
            .meshes(base())
            .translucent_faces
            .needs_sorting()
    };
    let sheet: Vec<_> = (2..=6)
        .flat_map(|x| (2..=6).map(move |z| (IVec3::new(x, 5, z), "water")))
        .collect();
    assert!(!needs(&sheet));
    let mut deeper = sheet.clone();
    deeper[0].0.y = 6;
    assert!(needs(&deeper));
    assert!(needs(&[(A, "glass")]));
}

/// Faces facing the same way in different planes, like two water
/// surfaces at different heights, need sorting too.
#[test]
fn parallel_planes_need_sorting() {
    let face = |y: f32, first: u32| SortFace {
        center: Vec3::new(0.5, y, 0.5),
        sort_at: Vec3::new(0.5, y, 0.5),
        normal: Vec3::Y,
        first,
        indices: &[0, 1, 2, 0, 2, 3],
    };
    assert!(!TranslucentFaces::new(vec![face(5.0, 0), face(5.0, 4)]).needs_sorting());
    assert!(TranslucentFaces::new(vec![face(5.0, 0), face(7.0, 4)]).needs_sorting());
    // A face and its back share their plane.
    assert_eq!(either_way(Vec3::NEG_Y), Vec3::Y);
    assert_eq!(
        either_way(Vec3::new(-1.0, 0.0, 1.0)),
        Vec3::new(1.0, 0.0, -1.0)
    );
}

/// Back faces are flagged for the shader to draw a hair farther away;
/// front faces aren't, and the flag leaves the layer as it was. Glass,
/// not double-sided, has no backs.
#[test]
fn back_faces_are_flagged() {
    let glass = neighborhood(&[(A, block("glass"))], |_| OPEN_SKY).meshes(base());
    assert!(
        faces_of(&glass.translucent)
            .iter()
            .all(|f| f[0] & BACK_FACE == 0)
    );
    for name in ["water", "lava"] {
        let meshes = neighborhood(&[(A, block(name))], |_| OPEN_SKY).meshes(base());
        let mesh = if name == "lava" {
            meshes.opaque
        } else {
            meshes.translucent
        };
        let faces = faces_of(&mesh);
        let layer = base().state(block(name)).faces[0].layer;
        for (n, quad) in faces.chunks(4).enumerate() {
            let back = n % 2 == 1;
            assert_eq!(quad[0][0] & BACK_FACE != 0, back, "{name} face {n}");
            assert_eq!(quad[0][0] & !BACK_FACE, layer);
        }
    }
}

/// From inside water, its back face against glass sorts after the
/// glass's front face in the same plane: the water blends over the
/// glass, as it's nearer.
#[test]
fn backs_sort_nearer_than_faces_in_their_plane() {
    let blocks = [(A, block("glass")), (B, block("water"))];
    let meshes = neighborhood(&blocks, |_| OPEN_SKY).meshes(base());
    let normals = vec3s(&meshes.translucent, Mesh::ATTRIBUTE_NORMAL);
    let positions = vec3s(&meshes.translucent, Mesh::ATTRIBUTE_POSITION);
    let faces = faces_of(&meshes.translucent);
    // In the water, looking at the glass (west, -X).
    let eye = B.as_vec3() + Vec3::new(0.8, 0.5, 0.5);
    let order = meshes.translucent_faces.indices_from(eye);
    let at = |pick: &dyn Fn(usize) -> bool| {
        order
            .chunks(6)
            .position(|quad| pick(quad[0] as usize))
            .unwrap()
    };
    let plane = |i: usize| (positions[i].x - B.x as f32).abs() < 1e-6;
    let glass_front = at(&|i| plane(i) && normals[i] == Vec3::X && faces[i][0] & BACK_FACE == 0);
    let water_back = at(&|i| plane(i) && normals[i] == Vec3::X && faces[i][0] & BACK_FACE != 0);
    assert!(glass_front < water_back, "{glass_front} vs {water_back}");
}

/// Translucent faces sort farthest first from the eye, as whole faces.
#[test]
fn translucent_faces_sort_back_to_front() {
    let blocks = [
        (A, block("glass")),
        (A + IVec3::new(0, 0, 3), block("glass")),
    ];
    let meshes = neighborhood(&blocks, |_| OPEN_SKY).meshes(base());
    let positions = vec3s(&meshes.translucent, Mesh::ATTRIBUTE_POSITION);
    let center = |quad: &[u32]| {
        quad.iter().map(|&i| positions[i as usize]).sum::<Vec3>() / quad.len() as f32
    };
    for eye in [
        Vec3::new(5.5, 6.0, 20.0),
        Vec3::new(5.5, 6.0, -10.0),
        Vec3::splat(6.0),
    ] {
        let indices = meshes.translucent_faces.indices_from(eye);
        assert_eq!(indices.len(), 12 * 6);
        let distances: Vec<f32> = indices
            .chunks(6)
            .map(|quad| {
                // Two triangles of one face: 0 1 2, 0 2 3.
                assert_eq!((quad[0], quad[2]), (quad[3], quad[4]));
                center(&[quad[0], quad[1], quad[2], quad[5]]).distance(eye)
            })
            .collect();
        // Back faces sort from a little inside their block.
        assert!(
            distances.is_sorted_by(|a, b| *a >= b - 0.02),
            "{eye}: {distances:?}"
        );
    }
}
