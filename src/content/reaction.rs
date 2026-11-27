//! Reactions as content: `reactions/<name>.json5`, what two fluids do
//! where they meet (see "Reactions" in `docs/modding.md`). Flow carries
//! them out (`fluid::flow`).

use super::{BlockState, Content, FULL_FLUID, qualify, schema::ReactionDef};

/// What two fluids do where they meet, baked.
#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    /// Its file's id.
    pub id: String,
    /// The fluid block whose voxel is used up whole, as an index into
    /// [`Content::blocks`].
    pub fluid: usize,
    /// The fluid block it meets.
    pub with: usize,
    /// What `fluid`'s voxel turns into, if it holds at least `at_least`.
    pub makes: BlockState,
    /// In thousandths of a block, as fluid amounts are.
    pub at_least: u16,
    /// How much of `with`'s voxel it uses up for each block of `fluid`, in
    /// thousandths.
    pub uses: u16,
}

/// Bakes a reaction from mod `namespace`.
pub fn bake_reaction(
    id: &str,
    namespace: &str,
    def: &ReactionDef,
    content: &Content,
) -> Result<Reaction, String> {
    let fluid = |field: &str, reference: &str| {
        let id = qualify(reference, namespace)?;
        let index = content
            .block_index
            .get(&id)
            .copied()
            .ok_or_else(|| format!("`{field}`: no block `{id}`"))?;
        let block = &content.blocks[index];
        if !content.state(block.default_state()).is_fluid() {
            return Err(format!("`{field}`: `{id}` isn't a fluid"));
        }
        Ok(index)
    };
    let amount = |field: &str, blocks: f32| {
        if !(0.0..=1.0).contains(&blocks) {
            return Err(format!(
                "`{field}` is {blocks}; it must be from 0 to 1 block"
            ));
        }
        Ok((blocks * FULL_FLUID as f32).round() as u16)
    };
    let (with, fluid) = (fluid("with", &def.with)?, fluid("fluid", &def.fluid)?);
    if with == fluid {
        return Err("`fluid` and `with` are the same fluid".into());
    }
    let makes_id = qualify(&def.makes, namespace)?;
    let makes = content
        .default_state(&makes_id)
        .ok_or_else(|| format!("`makes`: no block `{makes_id}`"))?;
    if makes.is_air() || content.state(makes).is_fluid() {
        return Err(format!("`makes`: `{makes_id}` isn't a solid block"));
    }
    Ok(Reaction {
        id: id.into(),
        fluid,
        with,
        makes,
        at_least: amount("at_least", def.at_least)?,
        uses: amount("uses", def.uses)?,
    })
}

impl Content {
    /// Adds a reaction, unless the same two fluids already react.
    pub(super) fn add_reaction(&mut self, reaction: Reaction) -> Result<(), String> {
        let pair = (reaction.fluid, reaction.with);
        if let Some(&other) = self.reaction_index.get(&pair) {
            return Err(format!(
                "these two fluids already react, in `{}`",
                self.reactions[other].id
            ));
        }
        let index = self.reactions.len();
        self.reaction_index.insert(pair, index);
        self.reaction_index.insert((pair.1, pair.0), index);
        self.reactions.push(reaction);
        Ok(())
    }

    /// What fluids `a` and `b` (block states) do where they meet, either
    /// way round; `None` if they don't react.
    pub fn reaction(&self, a: BlockState, b: BlockState) -> Option<&Reaction> {
        let pair = (self.state(a).block, self.state(b).block);
        self.reaction_index
            .get(&pair)
            .map(|&index| &self.reactions[index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{base, block};

    /// The base game's lava turns to stone where it meets water, either
    /// way round, using up a quarter of a block of the water.
    #[test]
    fn lava_and_water_react_into_stone() {
        let content = base();
        for (a, b) in [("lava", "water"), ("water", "lava")] {
            let reaction = content.reaction(block(a), block(b)).expect("they react");
            assert_eq!(reaction.fluid, content.state(block("lava")).block);
            assert_eq!(reaction.with, content.state(block("water")).block);
            assert_eq!(reaction.makes, block("stone"));
            assert_eq!(reaction.at_least, FULL_FLUID / 2);
            assert_eq!(reaction.uses, FULL_FLUID / 4);
        }
        assert!(content.reaction(block("water"), block("water")).is_none());
    }

    fn def(fluid: &str, with: &str, makes: &str, at_least: f32, uses: f32) -> ReactionDef {
        ReactionDef {
            fluid: fluid.into(),
            with: with.into(),
            makes: makes.into(),
            at_least,
            uses,
        }
    }

    /// Reactions only between two different fluids, making a solid block,
    /// with amounts within a block; anything else is an error naming what's
    /// wrong.
    #[test]
    fn bad_reactions_are_errors() {
        let content = base();
        let bake = |def: ReactionDef| bake_reaction("test:r", "defaria", &def, content);
        assert!(bake(def("lava", "water", "stone", 0.5, 0.25)).is_ok());
        for (bad, says) in [
            (def("stone", "water", "stone", 0.5, 0.25), "isn't a fluid"),
            (def("lava", "lava", "stone", 0.5, 0.25), "the same fluid"),
            (
                def("lava", "water", "water", 0.5, 0.25),
                "isn't a solid block",
            ),
            (def("lava", "water", "air", 0.5, 0.25), "no block"),
            (def("lava", "nothing", "stone", 0.5, 0.25), "no block"),
            (def("lava", "water", "stone", 1.5, 0.25), "`at_least`"),
            (def("lava", "water", "stone", 0.5, -1.0), "`uses`"),
        ] {
            let error = bake(bad.clone()).unwrap_err();
            assert!(error.contains(says), "{bad:?}: {error}");
        }
    }

    /// A second reaction between the same two fluids, either way round,
    /// is an error.
    #[test]
    fn two_fluids_react_one_way_only() {
        let mut content = base().clone();
        let again = bake_reaction(
            "test:again",
            "defaria",
            &def("water", "lava", "stone", 0.5, 0.25),
            &content,
        )
        .unwrap();
        let error = content.add_reaction(again).unwrap_err();
        assert!(error.contains("defaria:lava_water"), "{error}");
    }
}
