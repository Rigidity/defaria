//! A block's `support`: what it must stand on, for plants. Worked out
//! once every block is in, as it names other blocks, into the sorted list
//! of states it can stand on ([`Block::support`](super::Block::support)).

use super::{BlockState, Content, bake::When, index_of, qualify, schema::SupportDef, state_count};

/// Works out block `id`'s `support` and keeps it on the block. An error
/// leaves the block without one.
pub(super) fn bake(
    id: &str,
    rules: &[SupportDef],
    namespace: &str,
    content: &mut Content,
) -> Result<(), String> {
    if rules.is_empty() {
        return Err("`support` lists no blocks".into());
    }
    let mut states = Vec::new();
    for rule in rules {
        let below = qualify(&rule.block, namespace)?;
        let block = content
            .block(&below)
            .ok_or_else(|| format!("`support` names `{below}`, which isn't a block"))?;
        let when = When::new(&rule.when, &block.properties, namespace, content)
            .map_err(|error| format!("`support` on `{below}`: {error}"))?;
        states.extend(
            (0..state_count(&block.properties))
                .filter(|&offset| when.matches(&index_of(&block.properties, offset)))
                .map(|offset| BlockState(block.first_state + offset as u16)),
        );
    }
    states.sort_unstable();
    states.dedup();
    let index = content.blocks.iter().position(|b| b.id == id);
    if let Some(index) = index {
        content.blocks[index].support = Some(states);
    }
    Ok(())
}

impl Content {
    /// Whether `block` can stand on `below`: always, unless its block has
    /// a `support`, which must list `below`.
    pub fn supports(&self, block: BlockState, below: BlockState) -> bool {
        self.block_of(block)
            .support
            .as_ref()
            .is_none_or(|states| states.binary_search(&below).is_ok())
    }

    /// Whether `block`'s block has a `support`, so it breaks when what's
    /// under it stops supporting it.
    pub fn needs_support(&self, block: BlockState) -> bool {
        self.block_of(block).support.is_some()
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
        assert!(content.needs_support(tuft));
        assert!(content.supports(tuft, grass));
        assert!(content.supports(tuft, dirt));
        assert!(!content.supports(tuft, crate::content::sand()));
        assert!(!content.supports(tuft, block("stone")));
        assert!(!content.supports(tuft, crate::content::BlockState::AIR));
        assert!(!content.supports(tuft, tuft));
        // Blocks without a `support` stand on anything, air included.
        assert!(!content.needs_support(block("stone")));
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

    #[test]
    fn bad_support_is_reported() {
        for (support, error) in [
            ("[]", "lists no blocks"),
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
