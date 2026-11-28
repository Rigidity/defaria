//! A block's `support`: what it must rest on, and on which side. A plant
//! stands on the blocks its `support` lists, under it; a block hung on a
//! wall can rest on any solid face, on the side a property says. Worked out once every block
//! is in, as it names other blocks, into a [`Support`] on the block
//! ([`Block::support`](super::Block::support)).

use super::{
    BlockState, Content, Direction,
    bake::When,
    index_of, qualify,
    schema::{SupportDef, SupportsDef},
    state_count,
};

/// What a block must rest on: which side, and what holds it there.
#[derive(Debug, Clone, PartialEq)]
pub struct Support {
    pub side: SupportSide,
    pub by: SupportBy,
}

/// The side a block rests on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SupportSide {
    /// Always the same side (a plant's is down).
    Fixed(Direction),
    /// The value of the block's property at this index into its
    /// properties, whose values are all direction names (a wire's `on`).
    Property(usize),
}

/// What holds a block up on the side it rests on.
#[derive(Debug, Clone, PartialEq)]
pub enum SupportBy {
    /// These states, sorted.
    States(Vec<BlockState>),
    /// Any block whose face toward it is solid: covers that side.
    Solid,
}

/// Works out the `support` of the block at `index` in
/// [`Content::blocks`] and keeps it on the block. An error leaves the
/// block without one.
pub(super) fn bake(
    index: usize,
    def: &SupportsDef,
    namespace: &str,
    content: &mut Content,
) -> Result<(), String> {
    let (side, blocks, solid) = match def {
        SupportsDef::Blocks(blocks) => (None, blocks.as_slice(), false),
        SupportsDef::Rule(rule) => (rule.side.as_deref(), rule.blocks.as_slice(), rule.solid),
    };
    let side = match side {
        None => SupportSide::Fixed(Direction::Down),
        Some(side) => parse_side(side, index, content)?,
    };
    let by = match (blocks.is_empty(), solid) {
        (true, false) => return Err("`support` lists no blocks".into()),
        (false, true) => return Err("`support` lists blocks and says `solid`: give one".into()),
        (true, true) => SupportBy::Solid,
        (false, false) => SupportBy::States(states(blocks, namespace, content)?),
    };
    // Each state's side, worked out once.
    let block = &content.blocks[index];
    let first = block.first_state as usize;
    let sides: Vec<_> = (0..block.state_count as usize)
        .map(|offset| match side {
            SupportSide::Fixed(side) => Some(side),
            SupportSide::Property(at) => {
                let value = &block.properties[at].values[index_of(&block.properties, offset)[at]];
                Direction::from_name(value)
            }
        })
        .collect();
    for (offset, side) in sides.into_iter().enumerate() {
        content.states[first + offset].rests_on = side;
    }
    content.blocks[index].support = Some(Support { side, by });
    Ok(())
}

/// A `support`'s `side`: a direction, or `{property}` naming one of the
/// block's properties whose values are all directions.
fn parse_side(side: &str, index: usize, content: &Content) -> Result<SupportSide, String> {
    if let Some(name) = side.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let properties = &content.blocks[index].properties;
        let at = properties
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("`support` side: no property `{name}`"))?;
        if let Some(value) = properties[at]
            .values
            .iter()
            .find(|v| Direction::from_name(v).is_none())
        {
            return Err(format!(
                "`support` side: `{name}` has `{value}`, which isn't a direction"
            ));
        }
        return Ok(SupportSide::Property(at));
    }
    Direction::from_name(side)
        .map(SupportSide::Fixed)
        .ok_or_else(|| format!("`support` side `{side}` isn't a direction or a `{{property}}`"))
}

/// The states `blocks` lists, sorted.
fn states(
    blocks: &[SupportDef],
    namespace: &str,
    content: &Content,
) -> Result<Vec<BlockState>, String> {
    let mut states = Vec::new();
    for rule in blocks {
        let id = qualify(&rule.block, namespace)?;
        let block = content
            .block(&id)
            .ok_or_else(|| format!("`support` names `{id}`, which isn't a block"))?;
        let when = When::new(&rule.when, &block.properties, namespace, content)
            .map_err(|error| format!("`support` on `{id}`: {error}"))?;
        states.extend(
            (0..state_count(&block.properties))
                .filter(|&offset| when.matches(&index_of(&block.properties, offset)))
                .map(|offset| BlockState(block.first_state + offset as u16)),
        );
    }
    states.sort_unstable();
    states.dedup();
    Ok(states)
}

impl Content {
    /// The side `block` rests on, if its block has a `support`: what's
    /// there must hold it up ([`Content::supports`]).
    pub fn support_side(&self, block: BlockState) -> Option<Direction> {
        self.state(block).rests_on
    }

    /// Whether `block` can rest on `on`, the block on the side it rests on
    /// ([`Content::support_side`]): always, unless its block has a
    /// `support`, which must list `on`, or, for a solid one, `on` must
    /// cover its face toward `block`.
    pub fn supports(&self, block: BlockState, on: BlockState) -> bool {
        let Some(support) = &self.block_of(block).support else {
            return true;
        };
        match &support.by {
            SupportBy::States(states) => states.binary_search(&on).is_ok(),
            SupportBy::Solid => self
                .support_side(block)
                .is_some_and(|side| self.state(on).occludes(side.opposite())),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::content::{base, block, block_with, test_mod};

    #[test]
    fn plants_stand_on_what_their_support_lists() {
        let content = base();
        let tuft = block("tuft");
        let grass = crate::content::grass();
        let dirt = block_with("soil", &[("soil", "defaria:dirt"), ("turf", "none")]);
        assert!(content.support_side(tuft).is_some());
        assert!(content.supports(tuft, grass));
        assert!(content.supports(tuft, dirt));
        // Bare loam is soil plants grow in, like bare dirt; bare sand isn't.
        let loam = block_with("soil", &[("soil", "defaria:loam"), ("turf", "none")]);
        for plant in ["tuft", "tulip", "daisy", "bluebell", "fern", "mushroom"] {
            assert!(content.supports(block(plant), loam), "{plant} on loam");
        }
        assert!(!content.supports(block("dead_bush"), loam));
        assert!(!content.supports(tuft, crate::content::sand()));
        assert!(!content.supports(tuft, block("stone")));
        assert!(!content.supports(tuft, crate::content::BlockState::AIR));
        assert!(!content.supports(tuft, tuft));
        // Grass on sand holds grass plants, and not a dead bush, which
        // wants bare sand.
        let grassy_sand = block_with(
            "soil",
            &[("soil", "defaria:sand"), ("turf", "defaria:grass")],
        );
        let bush = block("dead_bush");
        assert!(content.supports(tuft, grassy_sand));
        assert!(content.supports(bush, crate::content::sand()));
        assert!(!content.supports(bush, grassy_sand));
        assert!(!content.supports(bush, grass));
        // Blocks without a `support` stand on anything, air included.
        assert!(content.support_side(block("stone")).is_none());
        assert!(content.supports(block("stone"), crate::content::BlockState::AIR));
    }

    #[test]
    fn support_matches_states_by_when() {
        let content = test_mod(&[
            (
                "blocks/ground.json5",
                r#"{ properties: { wet: ["false", "true"] }, model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "blocks/rock.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "blocks/reed.json5",
                r#"{ model: "quad", textures: { all: "a" },
                     support: [{ block: "ground", when: { wet: "true" } }, { block: "rock" }] }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let reed = content.default_state("test:reed").unwrap();
        let wet = content
            .block_state("test:ground", &[("wet", "true")])
            .unwrap();
        let dry = content
            .block_state("test:ground", &[("wet", "false")])
            .unwrap();
        let rock = content.default_state("test:rock").unwrap();
        assert!(content.supports(reed, wet));
        assert!(content.supports(reed, rock));
        assert!(!content.supports(reed, dry));
    }

    /// A rule rests a block on any block covering its face toward it, on
    /// the side its property says, or on the blocks it lists on a fixed
    /// side.
    #[test]
    fn rules_rest_blocks_on_a_side() {
        use crate::content::{BlockState, Direction};
        let content = test_mod(&[
            // A quad covers only its top.
            (
                "blocks/floor.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "blocks/lamp.json5",
                r#"{ model: "quad", textures: { all: "a" },
                     properties: { on: ["down", "up", "north", "south", "east", "west"] },
                     support: { side: "{on}", solid: true } }"#,
            ),
            (
                "blocks/vine.json5",
                r#"{ model: "quad", textures: { all: "a" },
                     support: { side: "north", blocks: [{ block: "floor" }] } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let lamp = |on: &str| content.block_state("test:lamp", &[("on", on)]).unwrap();
        let floor = content.default_state("test:floor").unwrap();
        let vine = content.default_state("test:vine").unwrap();
        assert_eq!(content.support_side(lamp("down")), Some(Direction::Down));
        assert_eq!(content.support_side(lamp("east")), Some(Direction::East));
        assert!(content.support_side(lamp("up")).is_some());
        // The floor covers its top: a lamp on it stands, one under it
        // doesn't hang.
        assert!(content.supports(lamp("down"), floor));
        assert!(!content.supports(lamp("up"), floor));
        assert!(!content.supports(lamp("down"), BlockState::AIR));
        assert_eq!(content.support_side(vine), Some(Direction::North));
        assert!(content.supports(vine, floor));
        assert!(!content.supports(vine, lamp("down")));
        assert_eq!(content.support_side(floor), None);
    }

    #[test]
    fn bad_support_is_reported() {
        for (support, error) in [
            ("[]", "lists no blocks"),
            ("{ side: \"up\" }", "lists no blocks"),
            (
                "{ solid: true, blocks: [{ block: \"stone\" }] }",
                "give one",
            ),
            ("{ side: \"sideways\", solid: true }", "isn't a direction"),
            (
                "{ side: \"{facing}\", solid: true }",
                "no property `facing`",
            ),
            (
                r#"[{ block: "nothing" }]"#,
                "`test:nothing`, which isn't a block",
            ),
            (
                r#"[{ block: "rock", when: { wet: "true" } }]"#,
                "no property `wet`",
            ),
        ] {
            let content = test_mod(&[
                (
                    "blocks/rock.json5",
                    r#"{ model: "quad", textures: { all: "a" } }"#,
                ),
                (
                    "blocks/reed.json5",
                    &format!(
                        r#"{{ model: "quad", textures: {{ all: "a" }}, support: {support} }}"#
                    ),
                ),
            ]);
            assert!(
                content
                    .errors
                    .iter()
                    .any(|e| e.contains("reed") && e.contains(error)),
                "{support}: {:?}",
                content.errors
            );
        }
    }
}
