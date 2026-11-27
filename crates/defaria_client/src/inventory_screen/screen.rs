//! The inventory screen (E): what the player carries, and the creative
//! catalog of every item.
//!
//! - E opens and closes it, while no other screen is open; Escape closes it
//!   too (see [`crate::ui::screen`]).
//! - In creative, the catalog, on top, lists every item the mods define,
//!   scrolling when there are more than fit.
//! - Under it, the player's inventory lists what they carry, sorted, with
//!   how many of each and the weight they carry out of what they can.
//! - The hotbar below stays where it is and takes part: items are dragged
//!   onto it, and its slots dragged around.
//! - Items move between the two lists with the mouse as `gesture`
//!   describes. Moving them into the catalog throws them away.
//! - The search box on top narrows both lists to what matches (see
//!   `search`).

use bevy::prelude::*;

use super::{
    list::{ItemList, ListSource, ListSystems, Owner, item_list},
    search::SearchBox,
};
use crate::{
    hotbar_ui::hotbar_height,
    ui::{UiScreen, UiTheme, text_field, typing},
};
use defaria_core::inventory::Inventory;
use defaria_core::{content::Content, game_mode::GameMode, player::LocalPlayer};

/// The catalog's rows that show at once; more scroll.
pub const CATALOG_ROWS: usize = 4;
/// The inventory's rows that show at once; more scroll.
pub const INVENTORY_ROWS: usize = 3;
/// Space between the inventory's room and the hotbar, in logical pixels.
pub const ABOVE_HOTBAR: f32 = 12.0;

pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<InventoryScreen>()
            .register_type::<WeightLabel>()
            .add_systems(
                Update,
                (
                    // Before the lists fill, so they're full on the first
                    // frame the screen shows.
                    toggle_inventory.run_if(not(typing)).before(ListSystems),
                    show_weight.after(toggle_inventory),
                ),
            );
    }
}

/// The inventory screen's root; despawning it closes the inventory.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
#[require(UiScreen)]
pub struct InventoryScreen;

/// How wide the panel's contents are: a list and its scrollbar.
pub fn panel_width(theme: &UiTheme) -> f32 {
    let columns = super::list::COLUMNS as f32;
    columns * theme.slot_size
        + (columns - 1.0) * theme.slot_gap
        + theme.scrollbar_gap
        + theme.scrollbar_width
}

/// The line showing how much the player carries.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct WeightLabel;

pub fn toggle_inventory(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    inventories: Query<Entity, With<InventoryScreen>>,
    screens: Query<(), With<UiScreen>>,
    theme: Res<UiTheme>,
    player: Option<Single<&GameMode, With<LocalPlayer>>>,
) {
    if !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    if let Some(inventory) = inventories.iter().next() {
        // Escape may close it this frame too.
        commands.entity(inventory).try_despawn();
    } else if screens.is_empty() {
        let creative = player.is_none_or(|mode| mode.is_creative());
        spawn_inventory(&mut commands, &theme, creative);
    }
}

/// A section's heading row: its title, and `extra` at its right, with
/// `above` logical pixels of space above it.
pub fn heading(theme: &UiTheme, title: &str, extra: impl Bundle, above: f32) -> impl Bundle {
    (
        Node {
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            margin: UiRect::top(px(above)),
            ..default()
        },
        children![
            (Text::new(title), theme.title_font(), TextColor(theme.text)),
            extra
        ],
    )
}

/// Spawns the screen, with the catalog in `creative`.
pub fn spawn_inventory(commands: &mut Commands, theme: &UiTheme, creative: bool) {
    let player = ListSource::Inventory(Owner::LocalPlayer);
    commands
        .spawn((
            Name::new("Inventory"),
            InventoryScreen,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexEnd,
                padding: UiRect::bottom(px(hotbar_height(theme) + ABOVE_HOTBAR)),
                ..default()
            },
            BackgroundColor(theme.backdrop),
            // Over the hotbar, which stays clickable.
            Pickable::IGNORE,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(px(theme.panel_padding)),
                    border: UiRect::all(px(1)),
                    row_gap: px(10),
                    ..default()
                },
                BackgroundColor(theme.panel),
                BorderColor::all(theme.panel_border),
            ))
            .with_children(|panel| {
                panel.spawn((
                    SearchBox,
                    text_field(
                        theme,
                        panel_width(theme),
                        "Search: words, category:, material:",
                        64,
                    ),
                ));
                if creative {
                    panel.spawn(heading(theme, "Items", (), 0.0));
                    item_list(
                        panel,
                        theme,
                        ItemList {
                            source: ListSource::Catalog,
                            sends_to: Some(player),
                        },
                        CATALOG_ROWS,
                        Name::new("Catalog"),
                    );
                }
                panel.spawn(heading(
                    theme,
                    "Inventory",
                    (
                        WeightLabel,
                        Text::default(),
                        theme.text_font(),
                        TextColor(theme.text_dim),
                    ),
                    if creative { 6.0 } else { 0.0 },
                ));
                // In survival there's nowhere for the items to go.
                let sends_to = creative.then_some(ListSource::Catalog);
                item_list(
                    panel,
                    theme,
                    ItemList {
                        source: player,
                        sends_to,
                    },
                    INVENTORY_ROWS,
                    Name::new("Inventory list"),
                );
            });
        });
}

/// A weight for people: kilograms, or tonnes from a thousand.
pub fn format_weight(kilograms: f32) -> String {
    if kilograms < 1000.0 {
        format!("{} kg", kilograms.round())
    } else {
        let tonnes = format!("{:.1}", kilograms / 1000.0);
        format!("{} t", tonnes.trim_end_matches(".0"))
    }
}

/// What the weight line says for `carried` of `max` kilograms.
pub fn weight_text(carried: f32, max: Option<f32>) -> String {
    match max {
        Some(max) => format!("{} of {}", format_weight(carried), format_weight(max)),
        None => format_weight(carried),
    }
}

/// Keeps the weight line up to date.
pub fn show_weight(
    content: Res<Content>,
    player: Option<Single<&Inventory, With<LocalPlayer>>>,
    mut labels: Query<&mut Text, With<WeightLabel>>,
) {
    let Some(inventory) = player else {
        return;
    };
    let text = weight_text(inventory.weight(&content), inventory.max_weight);
    for mut label in &mut labels {
        if label.0 != text {
            label.0.clone_from(&text);
        }
    }
}
