//! Fluid blocks: their amounts, and the box they're drawn as.

use bevy::prelude::*;

use super::{Def, model::model};
use crate::{
    content::{
        FULL_FLUID, Fluid, Model,
        schema::{BlockDef, FluidDef, ModelDef},
    },
    fluid::flow::MIN_RIPPLE,
};

/// The box every fluid is drawn as (`fluid_box.json5`), built into the
/// engine rather than taken from a mod.
pub(super) fn fluid_box() -> &'static Model {
    static BOX: std::sync::LazyLock<Model> = std::sync::LazyLock::new(|| {
        let def: ModelDef = json5::from_str(include_str!("../fluid_box.json5"))
            .expect("the fluid box is valid JSON5");
        model(&Def {
            id: "fluid box".into(),
            namespace: String::new(),
            file: "src/content/fluid_box.json5".into(),
            def,
        })
        .expect("the fluid box is a valid model")
    });
    &BOX
}

/// A block's `fluid_room`, in [`FULL_FLUID`] units.
pub(super) fn fluid_room(def: &BlockDef) -> Result<u16, String> {
    let Some(room) = def.fluid_room else {
        return Ok(0);
    };
    if def.fluid.is_some() {
        return Err("a fluid can't have `fluid_room`".into());
    }
    if !(0.0..=1.0).contains(&room) {
        return Err(format!(
            "`fluid_room` is {room}; it must be from 0 to 1 block"
        ));
    }
    Ok((room * FULL_FLUID as f32).round() as u16)
}

/// A block's `fluid`, with its amounts turned from blocks into
/// [`FULL_FLUID`] units.
pub(super) fn bake_fluid(def: &FluidDef) -> Result<Fluid, String> {
    let units = |name: &str, blocks: f32| -> Result<u16, String> {
        if !(0.0..=1.0).contains(&blocks) {
            return Err(format!(
                "fluid `{name}` is {blocks}; it must be from 0 to 1 block"
            ));
        }
        Ok((blocks * FULL_FLUID as f32).round() as u16)
    };
    if !(def.rate > 0.0 && def.rate.is_finite()) {
        return Err(format!("fluid `rate` is {}; it must be above 0", def.rate));
    }
    if !(0.0..=1.0).contains(&def.move_speed) {
        return Err(format!(
            "fluid `move_speed` is {}; it must be from 0 to 1",
            def.move_speed
        ));
    }
    if !(def.view.distance > 0.0 && def.view.distance.is_finite()) {
        return Err(format!(
            "fluid view `distance` is {}; it must be above 0",
            def.view.distance
        ));
    }
    let ripple = units("ripple", def.ripple)?;
    if ripple < MIN_RIPPLE {
        return Err(format!(
            "fluid `ripple` is {}; it must be at least {}, so that levelling off ends",
            def.ripple,
            MIN_RIPPLE as f32 / FULL_FLUID as f32
        ));
    }
    let color = Srgba::hex(&def.view.color)
        .map_err(|_| format!("`{}` isn't a hex color", def.view.color))?;
    Ok(Fluid {
        rate: def.rate,
        fall: units("fall", def.fall)?.max(1),
        film: units("film", def.film)?,
        ripple,
        move_speed: def.move_speed,
        view_color: color.into(),
        view_distance: def.view.distance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{BlockState, bake::test_util::*, test_mod as content};

    /// A fluid's amounts are turned from blocks into thousandths, and a
    /// `fall` too small to move anything moves one.
    #[test]
    fn fluid_amounts_are_baked_to_thousandths() {
        let content = content(&[(
            "blocks/pool.json5",
            r##"{ textures: { all: "a" }, fluid: {
                rate: 4, fall: 0.0001, film: 0.25, ripple: 0.05, move_speed: 0.3,
                view: { color: "#102030", distance: 3 },
            } }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let state = content.state(content.default_state("test:pool").unwrap());
        let fluid = state.fluid.as_ref().unwrap();
        assert_eq!((fluid.fall, fluid.film, fluid.ripple), (1, 250, 50));
        assert_eq!(
            (fluid.rate, fluid.move_speed, fluid.view_distance),
            (4.0, 0.3, 3.0)
        );
        assert_eq!(fluid.view_color, Srgba::hex("#102030").unwrap().into());
    }

    /// `fluid_room` is baked to thousandths of a block, from 0 to 1, and
    /// a fluid can't have one. Air has a whole block's; the base game's
    /// slabs half and its tuft 0.9.
    #[test]
    fn fluid_room_is_baked_and_checked() {
        let content = content(&[
            (
                "blocks/half.json5",
                r#"{ model: "quad", textures: { all: "a" }, fluid_room: 0.5 }"#,
            ),
            (
                "blocks/over.json5",
                r#"{ model: "quad", textures: { all: "a" }, fluid_room: 1.5 }"#,
            ),
            (
                "blocks/wet.json5",
                &format!(r#"{{ textures: {{ all: "a" }}, fluid_room: 0.5, {TEST_FLUID} }}"#),
            ),
        ]);
        let half = content.default_state("test:half").unwrap();
        assert_eq!(content.state(half).fluid_room, 500);
        assert!(content.block("test:over").is_none() && content.block("test:wet").is_none());
        let errors = errors(&content);
        assert!(errors.contains("`fluid_room` is 1.5"), "{errors}");
        assert!(
            errors.contains("a fluid can't have `fluid_room`"),
            "{errors}"
        );
        assert_eq!(content.state(BlockState::AIR).fluid_room, FULL_FLUID);
        let base = crate::content::base();
        let room = |id: &str| base.state(base.default_state(id).unwrap()).fluid_room;
        assert_eq!(room("defaria:stone_slab"), 500);
        assert_eq!(room("defaria:planks_slab"), 500);
        assert_eq!(room("defaria:tuft"), 900);
        assert_eq!(room("defaria:stone"), 0);
    }
}
