//! Properties a block sets as it's placed, from the face you click or the
//! way you look, such as a log's `axis`. See "Placement" in
//! `docs/modding.md`.

use std::collections::BTreeMap;

use bevy::prelude::*;

use super::{BlockState, Content, Direction, PlaceFrom, Property, index_of, offset_of};

/// How a property picks its value when its block is placed.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub from: PlaceFrom,
    /// The index of the value each direction gives, in [`Direction::ALL`]
    /// order. `None` keeps the value the block was going to have.
    pub values: [Option<usize>; 6],
}

impl Placement {
    /// Where to read a direction from, and which value each one gives by
    /// name. With no names given, each direction gives the value named
    /// after its axis (`x`, `y`, `z`) or after itself (`up`, `north`, ...).
    pub fn new(
        from: PlaceFrom,
        values: &[String],
        given: Option<&BTreeMap<Direction, String>>,
    ) -> Result<Self, String> {
        let index = |value: &str| values.iter().position(|v| v == value);
        let mut out = [None; 6];
        match given {
            Some(given) => {
                if given.is_empty() {
                    return Err("`place` `values` is empty".into());
                }
                for (side, value) in given {
                    let i = index(value).ok_or_else(|| {
                        format!("`place` gives `{value}`, which isn't one of its values")
                    })?;
                    out[*side as usize] = Some(i);
                }
            }
            None => {
                for side in Direction::ALL {
                    out[side as usize] = index(side.axis_name()).or_else(|| index(side.name()));
                }
                if out.iter().all(Option::is_none) {
                    return Err("`place` needs `values` unless the property's values are \
                         axes (`x`, `y`, `z`) or directions (`up`, `north`, ...)"
                        .into());
                }
            }
        }
        Ok(Self { from, values: out })
    }

    /// The value index placing against `face` (the side of the block
    /// clicked, pointing back at you) while looking along `look` gives.
    /// Looking steeply up or down at a property that has no value for
    /// that falls back to the way you face across the ground, so a
    /// machine placed at your feet still faces along your look.
    pub fn value(&self, face: Direction, look: Vec3) -> Option<usize> {
        let looked = || {
            self.values[Direction::along(look) as usize].or_else(|| {
                let flat = Vec3::new(look.x, 0.0, look.z);
                (flat.length_squared() > 1e-6)
                    .then(|| self.values[Direction::along(flat) as usize])
                    .flatten()
            })
        };
        match self.from {
            PlaceFrom::Face => self.values[face as usize],
            PlaceFrom::Look => looked(),
            PlaceFrom::Against => self.values[face.opposite() as usize].or_else(looked),
        }
    }
}

impl Content {
    /// The state a block placed as `state` ends up in, placed against
    /// `face` of the block clicked while looking along `look`: each
    /// property with a [`Placement`] takes the value it gives.
    pub fn placed(&self, state: BlockState, face: Direction, look: Vec3) -> BlockState {
        if state.is_air() {
            return state;
        }
        let block = self.block_of(state);
        let properties: &[Property] = &block.properties;
        let offset = (state.0 - block.first_state) as usize;
        let mut index = index_of(properties, offset);
        for (p, property) in properties.iter().enumerate() {
            if let Some(i) = property.place.as_ref().and_then(|pl| pl.value(face, look)) {
                index[p] = i;
            }
        }
        BlockState(block.first_state + offset_of(properties, &index) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{base, block, test_mod};

    /// The base game's logs run along the axis of the face they're placed
    /// against, keeping their wood.
    #[test]
    fn logs_take_their_axis_from_the_face() {
        let content = base();
        let log = content.block("defaria:log").unwrap();
        let birch = log.state(&[("wood", "defaria:birch")]).unwrap();
        for (face, axis) in [
            (Direction::Up, "y"),
            (Direction::Down, "y"),
            (Direction::East, "x"),
            (Direction::West, "x"),
            (Direction::North, "z"),
            (Direction::South, "z"),
        ] {
            // The view doesn't matter, only the face.
            let placed = content.placed(birch, face, Vec3::NEG_Y);
            assert_eq!(
                placed,
                log.state(&[("wood", "defaria:birch"), ("axis", axis)])
                    .unwrap(),
                "{face:?}"
            );
        }
        // Blocks with nothing to pick are placed as they are.
        let stone = block("stone");
        assert_eq!(content.placed(stone, Direction::East, Vec3::X), stone);
    }

    #[test]
    fn look_and_explicit_values() {
        let content = test_mod(&[
            ("models/m.json5", "{ faces: [] }"),
            (
                "blocks/pillar.json5",
                r#"{ model: "m", properties: { axis: { values: ["y", "x", "z"], place: "look" } } }"#,
            ),
            (
                "blocks/half.json5",
                r#"{ model: "m", properties: { half: {
                    values: ["bottom", "top"],
                    place: { from: "face", values: { up: "bottom", down: "top" } },
                } } }"#,
            ),
            (
                "blocks/facing.json5",
                r#"{ model: "m", properties: { facing: {
                    values: ["north", "south", "east", "west"], place: "look",
                } } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let pillar = content.block("test:pillar").unwrap();
        let s = pillar.default_state();
        let axis = |a| pillar.state(&[("axis", a)]).unwrap();
        assert_eq!(
            content.placed(s, Direction::Up, Vec3::new(0.9, -0.3, 0.0)),
            axis("x")
        );
        assert_eq!(
            content.placed(s, Direction::Up, Vec3::new(0.1, 0.3, -0.9)),
            axis("z")
        );

        let half = content.block("test:half").unwrap();
        let s = half.default_state();
        let top = half.state(&[("half", "top")]).unwrap();
        assert_eq!(content.placed(s, Direction::Down, Vec3::Y), top);
        assert_eq!(content.placed(top, Direction::Up, Vec3::Y), s);
        // A side isn't listed, so the value stays as it was.
        assert_eq!(content.placed(top, Direction::East, Vec3::Y), top);

        // Directions name values too; looking up or down names none here.
        let facing = content.block("test:facing").unwrap();
        let s = facing.default_state();
        let west = facing.state(&[("facing", "west")]).unwrap();
        assert_eq!(content.placed(s, Direction::Up, Vec3::NEG_X), west);
        assert_eq!(content.placed(west, Direction::Up, Vec3::NEG_Y), west);
    }

    /// `against` takes the side of the placed block that touches the block
    /// clicked, whichever way you look: placed on a block's north side,
    /// it's `south`, even looking along the wall. With no value for that
    /// side (placed on a floor), it's the way you look.
    #[test]
    fn against_takes_the_side_touching_the_block_clicked() {
        let content = base();
        let slab = content.block("defaria:stone_vertical_slab").unwrap();
        let half = |h| slab.state(&[("half", h)]).unwrap();
        let s = slab.default_state();
        // Looking mostly east along a wall's north side.
        let along = Vec3::new(0.9, -0.1, 0.3).normalize();
        for (face, expected) in [
            (Direction::North, "south"),
            (Direction::South, "north"),
            (Direction::East, "west"),
            (Direction::West, "east"),
        ] {
            assert_eq!(content.placed(s, face, along), half(expected), "{face:?}");
        }
        // On a floor or under a ceiling: the far side, along the view.
        assert_eq!(content.placed(s, Direction::Up, along), half("east"));
        let down_west = Vec3::new(-0.3, -0.9, 0.1).normalize();
        assert_eq!(content.placed(s, Direction::Down, down_west), half("west"));
    }

    /// Looking steeply down (placing a machine on the floor in front of
    /// you) or up, a `facing` with only the four sides takes the way you
    /// face across the ground, whichever side of the block you click.
    #[test]
    fn looking_steeply_falls_back_to_the_horizontal() {
        let content = test_mod(&[
            ("models/m.json5", "{ faces: [] }"),
            (
                "blocks/facing.json5",
                r#"{ model: "m", properties: { facing: {
                    values: ["north", "south", "east", "west"], place: "look",
                } } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let facing = content.block("test:facing").unwrap();
        let s = facing.default_state();
        for (horizontal, name) in [
            (Vec3::NEG_Z, "north"),
            (Vec3::Z, "south"),
            (Vec3::X, "east"),
            (Vec3::NEG_X, "west"),
        ] {
            let want = facing.state(&[("facing", name)]).unwrap();
            // About 60° down, and nearly straight down or up.
            for vertical in [-1.7, -50.0, 50.0] {
                let look = (horizontal + Vec3::Y * vertical).normalize();
                for face in [Direction::Up, Direction::South, Direction::West] {
                    assert_eq!(content.placed(s, face, look), want, "{name} {vertical}");
                }
            }
        }
        // The base game's battery, placed on the floor ahead looking north,
        // faces back south at you.
        let content = base();
        let battery = content.block("defaria:battery").unwrap();
        let look = Vec3::new(0.0, -1.7, -1.0).normalize();
        assert_eq!(
            content.placed(battery.default_state(), Direction::Up, look),
            battery.state(&[("facing", "south")]).unwrap()
        );
    }

    #[test]
    fn bad_placements_are_errors() {
        for (file, error) in [
            (
                r#"{ model: "m", properties: { color: { values: ["red", "blue"], place: "face" } } }"#,
                "needs `values`",
            ),
            (
                r#"{ model: "m", properties: { half: { values: ["a", "b"], place: { from: "face", values: { up: "c" } } } } }"#,
                "`c`",
            ),
            (
                r#"{ model: "m", properties: { half: { values: ["a", "b"], place: { from: "face", values: {} } } } }"#,
                "is empty",
            ),
            (
                r#"{ model: "m", properties: { axis: { values: ["x", "y"], place: "sideways" } } }"#,
                "sideways",
            ),
            (
                r#"{ model: "m", properties: { axis: { values: ["x", "y"], place: "face", item: true } } }"#,
                "item",
            ),
        ] {
            let content = test_mod(&[("blocks/thing.json5", file)]);
            assert!(
                content.errors.iter().any(|e| e.contains(error)),
                "{file}: {:?}",
                content.errors
            );
        }
        let content = test_mod(&[
            (
                "materials/m.json5",
                r##"{ palette: ["#000000", "#ffffff"] }"##,
            ),
            (
                "blocks/thing.json5",
                r#"{ model: "m", properties: { m: { materials: "m", place: "face" } } }"#,
            ),
        ]);
        assert!(
            content.errors.iter().any(|e| e.contains("plain")),
            "{:?}",
            content.errors
        );
        let content = test_mod(&[(
            "items/thing.json5",
            r#"{ properties: { axis: { values: ["x", "y"], place: "face" } } }"#,
        )]);
        assert!(
            content.errors.iter().any(|e| e.contains("only for block")),
            "{:?}",
            content.errors
        );
    }
}
