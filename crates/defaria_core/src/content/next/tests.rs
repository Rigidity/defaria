use super::*;
use bevy::math::{Vec2, Vec3};

use crate::content::{BlockBox, test_mod};

/// Air all round but `east` and `west`.
fn around(east: BlockState, west: BlockState) -> Around {
    Around::read(|side| match side {
        Direction::East => east,
        Direction::West => west,
        _ => BlockState::AIR,
    })
}

/// Parts with a `next` are kept apart from the state's own: their faces
/// and boxes, covering nothing.
#[test]
fn parts_drawn_by_neighbours_are_kept_apart() {
    let content = vines();
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let vine = content.default_state("test:vine").unwrap();
    let state = content.state(vine);
    assert_eq!(state.faces.len(), 1);
    assert_eq!(state.next_faces.len(), 4);
    assert_eq!(
        state.next_parts,
        [
            NextPart {
                rule: 0,
                faces: 0..2,
                collision: 0..1,
                target: 0..1,
            },
            NextPart {
                rule: 1,
                faces: 2..4,
                collision: 1..2,
                target: 1..2,
            },
        ]
    );
    // The quad's (no box, as it's flat) and its top: nothing from the
    // pairs, whose boxes (the whole block, around their faces) are kept
    // apart. With those to aim at, the state has no whole block of its
    // own to aim at.
    assert_eq!(state.parts.len(), 1);
    assert!(state.collision.is_empty());
    assert!(state.target.is_empty());
    assert_eq!(state.next_target, [BlockBox::FULL; 2]);
    assert!(state.occludes(Direction::Up) && !state.occludes(Direction::Down));
    assert_eq!(state.next_collision, [BlockBox::FULL; 2]);
    assert_eq!(content.block_of(vine).next.len(), 2);
}

/// Things bump into the collision boxes of the parts the neighbours
/// draw, and only those.
#[test]
fn neighbours_pick_the_boxes_bumped_into() {
    let content = vines();
    let vine = content.default_state("test:vine").unwrap();
    let tall = content
        .block_state("test:post", &[("tall", "yes")])
        .unwrap();
    let boxes =
        |east: BlockState, west: BlockState| content.collision(vine, &around(east, west)).len();
    assert_eq!(boxes(tall, BlockState::AIR), 2);
    assert_eq!(boxes(BlockState::AIR, BlockState::AIR), 1);
    assert_eq!(boxes(BlockState::AIR, tall), 0);
    // A block without such parts lends its own.
    let post = content.default_state("test:post").unwrap();
    assert!(matches!(
        content.collision(post, &Around::AIR),
        std::borrow::Cow::Borrowed(_)
    ));
}

/// Each part is drawn where the blocks next to it match its rule.
#[test]
fn neighbours_pick_the_parts_drawn() {
    let content = vines();
    let vine = content.default_state("test:vine").unwrap();
    let tall = content
        .block_state("test:post", &[("tall", "yes")])
        .unwrap();
    let short = content.default_state("test:post").unwrap();
    let drawn = |east: BlockState, west: BlockState| -> Vec<usize> {
        content
            .next_faces(vine, &around(east, west))
            .map(|(i, _)| i)
            .collect()
    };
    assert_eq!(drawn(tall, BlockState::AIR), [0, 1, 2, 3]);
    assert_eq!(drawn(short, BlockState::AIR), [2, 3]);
    assert_eq!(drawn(tall, short), [0, 1]);
    assert!(drawn(BlockState::AIR, tall).is_empty());
}

/// A `next` naming no block, or a property the block hasn't got, is an
/// error, and the block's parts drawn by their neighbours aren't drawn.
#[test]
fn bad_rules_are_errors() {
    for (next, error) in [
        (
            r#"{ east: { block: "nothing" } }"#,
            "`test:nothing` isn't a block",
        ),
        (
            r#"{ east: { block: "post", when: { short: "yes" } } }"#,
            "no property `short`",
        ),
    ] {
        let vine = format!(
            r#"{{ model: "quad", textures: {{ all: "a", top: "a", bottom: "a" }},
                 parts: [{{ model: "pair", next: {next} }}] }}"#
        );
        let content = test_mod(&[
            (
                "blocks/post.json5",
                r#"{ properties: { tall: ["no", "yes"] }, model: "quad", textures: { all: "a" } }"#,
            ),
            ("blocks/vine.json5", Box::leak(vine.into_boxed_str())),
        ]);
        let errors = content.errors.join("\n");
        assert!(
            errors.contains("part 1: `next`") && errors.contains(error),
            "{errors}"
        );
        let vine = content.default_state("test:vine").unwrap();
        assert_eq!(content.next_faces(vine, &Around::AIR).count(), 0);
    }
}

/// A floor wire joined up bumps into what it draws: its riser to the top
/// under a floor wire, its climb up the north side under a north-wall
/// wire.
#[test]
fn a_wire_collides_where_it_bends() {
    let content = crate::content::base();
    let wire = |values: &[(&str, &str)]| content.block_state("defaria:wire", values).unwrap();
    let below = wire(&[("on", "down"), ("up", "true")]);
    let boxes = |above: BlockState| {
        content
            .collision(below, |side| match side {
                Direction::Up => above,
                _ => BlockState::AIR,
            })
            .into_owned()
    };
    let floor = boxes(wire(&[("on", "down"), ("down", "true")]));
    // The core, and the riser up the middle to the top.
    assert_eq!(floor.len(), 2);
    assert!(floor.iter().any(|b| b.max.y == 1.0 && b.min.z > 0.0));
    let wall = boxes(wire(&[("on", "north"), ("down", "true")]));
    // The core, and the climb: along the floor, then up the north side.
    assert_eq!(wall.len(), 3);
    assert!(wall.iter().any(|b| b.max.y == 1.0 && b.min.z == 0.0));
    assert!(!wall.iter().any(|b| b.max.y == 1.0 && b.min.z > 0.0));
}

/// Where a voxel's boxes reach `side`: each box's footprint on that side
/// of the block, as (min, max) on the side's other two axes.
fn ends_on(boxes: &[BlockBox], side: Direction) -> Vec<(Vec2, Vec2)> {
    let n = side.normal();
    let axis = n.abs().max_position();
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    boxes
        .iter()
        .filter(|b| {
            let at = if n[axis] > 0 {
                b.max[axis]
            } else {
                b.min[axis]
            };
            (at - if n[axis] > 0 { 1.0 } else { 0.0 }).abs() < 1e-5
        })
        .map(|b| (Vec2::new(b.min[u], b.min[v]), Vec2::new(b.max[u], b.max[v])))
        .collect()
}

/// Whether `boxes` are all one piece: each shares some of a face with
/// another, or overlaps it, all joined up (touching only along an edge or
/// at a corner isn't joined).
fn one_piece(boxes: &[BlockBox]) -> bool {
    let touch = |a: &BlockBox, b: &BlockBox| {
        let overlap = a.max.min(b.max) - a.min.max(b.min);
        overlap.cmpge(Vec3::splat(-1e-5)).all()
            && overlap.cmpgt(Vec3::splat(1e-5)).bitmask().count_ones() >= 2
    };
    let mut reached = vec![false; boxes.len()];
    let mut stack = vec![0];
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut reached[i], true) {
            continue;
        }
        stack.extend((0..boxes.len()).filter(|&j| !reached[j] && touch(&boxes[i], &boxes[j])));
    }
    boxes.is_empty() || reached.iter().all(|&r| r)
}

/// Whether every footprint in `inner` lies within one in `outer`.
fn within(inner: &[(Vec2, Vec2)], outer: &[(Vec2, Vec2)]) -> bool {
    inner.iter().all(|(lo, hi)| {
        outer
            .iter()
            .any(|(olo, ohi)| lo.cmpge(*olo - 1e-5).all() && hi.cmple(*ohi + 1e-5).all())
    })
}

/// Every base conduit (wires, pipes, tubes), in every way its shape can
/// go (its materials at their defaults), is one piece and meets a joined
/// neighbour of its own kind wherever that one is: each reaches the side
/// between them, and one's end there lies within the other's (the same
/// end, for two arms meeting). Pairs where one would rest on the other
/// are left out: a wire rests only on a solid face (its `support`), which
/// a wire isn't. The bends drawn by the
/// neighbours are worked out by `scripts/wire-models.py` for each wire
/// alone, so this is what keeps the two halves of every joint in step.
#[test]
fn joined_conduits_meet() {
    let content = crate::content::base();
    let mut checked = Vec::new();
    for block in content.blocks.iter().filter(|b| b.is_conduit()) {
        let Some(connections) = &block.connections else {
            continue;
        };
        // The states that differ in shape: every property but materials
        // at any value.
        let shapes: Vec<BlockState> = (0..block.state_count)
            .map(|offset| BlockState(block.first_state + offset))
            .filter(|&state| {
                let index = crate::content::index_of(
                    &block.properties,
                    (state.0 - block.first_state) as usize,
                );
                block
                    .properties
                    .iter()
                    .zip(&index)
                    .all(|(p, &i)| !p.materials || i == 0)
            })
            .collect();
        let joined = |state: BlockState, side: Direction| {
            let index =
                crate::content::index_of(&block.properties, (state.0 - block.first_state) as usize);
            index[connections.sides[side as usize]] == 1
        };
        for side in Direction::ALL {
            let here: Vec<_> = shapes
                .iter()
                .copied()
                .filter(|&s| joined(s, side))
                .collect();
            let there: Vec<_> = shapes
                .iter()
                .copied()
                .filter(|&s| joined(s, side.opposite()))
                .collect();
            // Not resting on each other, which their `support` rules out.
            let rests = |state: BlockState, toward: Direction| {
                content.support_side(state) == Some(toward)
                    && !content.supports(state, BlockState::AIR)
            };
            for &a in here.iter().filter(|&&a| !rests(a, side)) {
                for &b in there.iter().filter(|&&b| !rests(b, side.opposite())) {
                    let boxes_a =
                        content.collision(a, |d| if d == side { b } else { BlockState::AIR });
                    let boxes_b = content.collision(b, |d| {
                        if d == side.opposite() {
                            a
                        } else {
                            BlockState::AIR
                        }
                    });
                    assert!(
                        one_piece(&boxes_a),
                        "{}, with {} {side:?} of it, is in pieces: {boxes_a:?}",
                        content.state(a).name,
                        content.state(b).name,
                    );
                    let (end_a, end_b) =
                        (ends_on(&boxes_a, side), ends_on(&boxes_b, side.opposite()));
                    assert!(
                        !end_a.is_empty()
                            && !end_b.is_empty()
                            && (within(&end_a, &end_b) || within(&end_b, &end_a)),
                        "{} and, {side:?} of it, {} don't meet: {end_a:?} and {end_b:?}",
                        content.state(a).name,
                        content.state(b).name,
                    );
                    checked.push(block.id.as_str());
                }
            }
        }
    }
    for id in ["defaria:wire", "defaria:pipe", "defaria:tube"] {
        let joints = checked.iter().filter(|&&c| c == id).count();
        assert!(joints > 100, "only {joints} joints of {id} checked");
    }
}
