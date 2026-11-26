//! The numbers blocks and items work out per state, from their materials'
//! stats: hardness, weight and `stats`.

use std::collections::BTreeMap;

use super::material::{check_amount, check_stat_name};
use crate::content::{BUILT_IN_STATS, Content, NONE, Property, schema::Attribute};

/// The numbers each state of a block or item has: its hardness, weight and
/// `stats`.
#[derive(Debug, Clone, Default)]
pub(super) struct Numbers {
    pub(super) hardness: f32,
    pub(super) weight: f32,
    pub(super) stats: BTreeMap<String, f32>,
}

/// A block's or item's hardness, weight and `stats`, checked against its
/// properties, to work out per state. Hardness and weight are `None` where
/// the file leaves them out.
pub(super) struct Attributes {
    hardness: Option<Mix>,
    weight: Option<Mix>,
    stats: Vec<(String, Mix)>,
}

impl Attributes {
    /// `block` is the block's properties when these are its item's.
    pub(super) fn new(
        hardness: Option<&Attribute>,
        weight: Option<&Attribute>,
        stats: &BTreeMap<String, Attribute>,
        properties: &[Property],
        block: Option<&[Property]>,
        content: &Content,
    ) -> Result<Self, String> {
        let mix = |field: &str, stat: &str, def: &Attribute| {
            Mix::new(field, stat, def, properties, block, content)
        };
        Ok(Attributes {
            hardness: hardness
                .map(|def| mix("hardness", "hardness", def))
                .transpose()?,
            weight: weight.map(|def| mix("weight", "weight", def)).transpose()?,
            stats: stats
                .iter()
                .map(|(name, def)| {
                    check_stat_name(name)?;
                    Ok((name.clone(), mix(&format!("stats.{name}"), name, def)?))
                })
                .collect::<Result<_, String>>()?,
        })
    }

    /// The numbers of the state with these value indices: `fallback`'s
    /// hardness and weight where the file leaves one out, and its stats
    /// under the file's own.
    pub(super) fn values(
        &self,
        fallback: &Numbers,
        properties: &[Property],
        index: &[usize],
        content: &Content,
    ) -> Numbers {
        let value = |mix: &Option<Mix>, fallback| {
            mix.as_ref()
                .map_or(fallback, |mix| mix.value(properties, index, content))
        };
        let mut stats = fallback.stats.clone();
        for (name, mix) in &self.stats {
            stats.insert(name.clone(), mix.value(properties, index, content));
        }
        Numbers {
            hardness: value(&self.hardness, fallback.hardness),
            weight: value(&self.weight, fallback.weight),
            stats,
        }
    }
}

/// An attribute, checked against a block's or item's properties: a number,
/// or a base times a weighted average of one stat of material properties'
/// materials. Any number a block or item works out per state from its
/// materials uses this, so each field reads stats the same way.
pub(in crate::content) struct Mix {
    base: f32,
    /// The materials' stat it averages.
    stat: String,
    /// The value for a material without the stat.
    default: Option<f32>,
    /// Property index and how much it counts.
    shares: Vec<(usize, f32)>,
}

impl Mix {
    /// `field` names the attribute, for errors, and `stat` is the stat it
    /// averages unless `def` names one. `block` is the block's properties
    /// when `properties` are its item's.
    ///
    /// Every state must have a material in at least one counted property:
    /// `base` alone is a multiplier (a volume, a fraction of a block), not
    /// a value of its own. Every material a counted property can be must
    /// have the stat, unless the attribute gives a `default`.
    pub(in crate::content) fn new(
        field: &str,
        stat: &str,
        def: &Attribute,
        properties: &[Property],
        block: Option<&[Property]>,
        content: &Content,
    ) -> Result<Self, String> {
        let mixed = match def {
            Attribute::Fixed(value) => {
                check_amount(field, *value)?;
                return Ok(Mix {
                    base: *value,
                    stat: stat.into(),
                    default: None,
                    shares: Vec::new(),
                });
            }
            Attribute::Mixed(mixed) => mixed,
        };
        check_amount(field, mixed.base)?;
        let stat = mixed.stat.as_deref().unwrap_or(stat);
        let known = BUILT_IN_STATS.contains(&stat)
            || content
                .materials
                .values()
                .any(|m| m.stats.contains_key(stat));
        if !known {
            return Err(format!(
                "`{field}` averages stat `{stat}`, which no material has"
            ));
        }
        if let Some(default) = mixed.default {
            check_amount(&format!("{field}` `default"), default)?;
        }
        if mixed.materials.is_empty() {
            return Err(format!(
                "`{field}` has no `materials`; for a fixed value, write a number"
            ));
        }
        let mut shares = Vec::new();
        for (property, &share) in &mixed.materials {
            let p = properties
                .iter()
                .position(|prop| &prop.name == property)
                .ok_or_else(|| {
                    let item_false =
                        block.is_some_and(|block| block.iter().any(|p| &p.name == property));
                    if item_false {
                        format!(
                            "`{field}` counts `{property}`, which isn't one of the item's \
                             properties: the block marks it `item: false`"
                        )
                    } else {
                        format!("`{field}` counts `{property}`, which isn't a property")
                    }
                })?;
            if !properties[p].materials {
                return Err(format!(
                    "`{field}` counts `{property}`, which isn't a material property"
                ));
            }
            if !(share.is_finite() && share > 0.0) {
                return Err(format!(
                    "`{field}` counts `{property}` {share}; a share must be more than 0"
                ));
            }
            if mixed.default.is_none() {
                let lacking = properties[p]
                    .values
                    .iter()
                    .filter(|v| *v != NONE)
                    .find(|v| content.materials[*v].stat(stat).is_none());
                if let Some(material) = lacking {
                    return Err(format!(
                        "`{field}` averages stat `{stat}` of `{property}`, but material \
                         `{material}` has no `{stat}`; give it one, or give `{field}` a \
                         `default`"
                    ));
                }
            }
            shares.push((p, share));
        }
        // States are every combination, so one has them all at `none`
        // exactly when each can be.
        if shares
            .iter()
            .all(|&(p, _)| properties[p].values.iter().any(|v| v == NONE))
        {
            let counted: Vec<String> = shares
                .iter()
                .map(|&(p, _)| format!("{}=none", properties[p].name))
                .collect();
            return Err(format!(
                "`{field}`: in states with {}, none of its counted properties has a \
                 material; count a property that isn't optional, or write a number",
                counted.join(",")
            ));
        }
        Ok(Mix {
            base: mixed.base,
            stat: stat.into(),
            default: mixed.default,
            shares,
        })
    }

    /// The value for the state with these value indices: a fixed value's
    /// base, or the base times the shares' weighted average of their
    /// materials' stat, skipping properties at `none` (`new` checked that
    /// not every one is, and that each material has the stat or there's a
    /// default).
    pub(in crate::content) fn value(
        &self,
        properties: &[Property],
        index: &[usize],
        content: &Content,
    ) -> f32 {
        if self.shares.is_empty() {
            // A fixed value.
            return self.base;
        }
        let (mut sum, mut total) = (0.0, 0.0);
        for &(p, share) in &self.shares {
            let value = &properties[p].values[index[p]];
            if value == NONE {
                continue;
            }
            let stat = content.materials[value].stat(&self.stat).or(self.default);
            sum += share * stat.unwrap_or_default();
            total += share;
        }
        debug_assert!(total > 0.0, "every counted property is `none`");
        self.base * sum / total
    }
}

#[cfg(test)]
mod tests;
