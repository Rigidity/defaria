use super::*;

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
