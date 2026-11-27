//! How fluid is shown: amounts in litres, a fluid's color and its name,
//! the same in a machine's tank bars and a fluid container's slot.

use bevy::prelude::*;

use defaria_core::content::{BlockState, Content, title_case};

/// The color shown for no fluid (an empty tank).
pub const NO_FLUID: Color = Color::srgb(0.5, 0.5, 0.55);

/// An amount of fluid for people, in litres: a full block is a cubic
/// metre, a thousand of them, as the amounts are counted.
pub fn format_fluid(amount: u64) -> String {
    let digits = amount.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    format!("{out} L")
}

/// The color of the fluid block `fluid`: the color of the fog inside it,
/// or [`NO_FLUID`] for none.
pub fn fluid_color(content: &Content, fluid: Option<BlockState>) -> Color {
    fluid
        .and_then(|state| content.state(state).fluid.as_ref())
        .map_or(NO_FLUID, |fluid| fluid.view_color)
}

/// The name of the fluid block `id` ("Water"), or, for one that isn't
/// loaded, the id's name in title case.
pub fn fluid_name(content: &Content, id: &str) -> String {
    match content.default_state(id) {
        Some(state) => content.state(state).name.clone(),
        None => title_case(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defaria_core::content::{base, block};

    #[test]
    fn fluids_are_named_and_colored() {
        assert_eq!(
            [0, 400, 4000, 1_234_567].map(format_fluid),
            ["0 L", "400 L", "4,000 L", "1,234,567 L"]
        );
        assert_eq!(fluid_name(base(), "defaria:water"), "Water");
        assert_eq!(fluid_name(base(), "gone:hot_oil"), "Hot oil");
        assert_eq!(fluid_color(base(), None), NO_FLUID);
        let lava = base()
            .state(block("lava"))
            .fluid
            .as_ref()
            .unwrap()
            .view_color;
        assert_eq!(fluid_color(base(), Some(block("lava"))), lava);
    }
}
