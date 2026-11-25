//! The inventory screen: every item the mods define, in a scrolling grid.
//!
//! - E opens and closes it, while no other screen is open; Escape closes it
//!   too (see [`crate::ui::screen`]).
//! - Clicking an item puts it in the active hotbar slot. The number keys
//!   and clicks on the hotbar pick which slot that is, so filling the
//!   hotbar is: pick a slot, click an item.
//! - The item in the active slot is framed in the grid.
//! - The wheel scrolls the grid when there are more items than fit.
//!
//! Items show in [`Content::items`] order: blocks' items first, then the
//! rest, each in load order.

use bevy::prelude::*;

use crate::{
    content::{Content, ItemState},
    hotbar::{ActiveSlot, Hotbar, hotbar_height},
    ui::{Clicked, ItemSlot, UiScreen, UiTheme, item_slot, scroll_panel},
};

/// Slots in a row of the grid.
pub const COLUMNS: usize = 9;
/// The most rows that show at once; more scroll.
pub const VISIBLE_ROWS: usize = 5;
/// Space between the inventory's room and the hotbar, in logical pixels.
const ABOVE_HOTBAR: f32 = 12.0;

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<CatalogSlot>()
            .register_type::<InventoryScreen>()
            .register_type::<InventoryGrid>()
            .add_observer(pick_item)
            .add_systems(Update, (toggle_inventory, frame_active_item));
    }
}

/// A slot of the inventory grid, showing its item.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct CatalogSlot(pub ItemState);

/// The inventory screen's root; despawning it closes the inventory.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
#[require(UiScreen)]
pub struct InventoryScreen;

/// The scrolling grid of slots.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct InventoryGrid;

fn toggle_inventory(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    inventories: Query<Entity, With<InventoryScreen>>,
    screens: Query<(), With<UiScreen>>,
    theme: Res<UiTheme>,
    content: Res<Content>,
    hotbar: Res<Hotbar>,
    active: Res<ActiveSlot>,
) {
    if !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    if let Some(inventory) = inventories.iter().next() {
        // Escape may close it this frame too.
        commands.entity(inventory).try_despawn();
    } else if screens.is_empty() {
        spawn_inventory(&mut commands, &theme, &content, hotbar.active(*active));
    }
}

/// Spawns the inventory, with `framed` (the active hotbar slot's item)
/// framed from its first frame.
fn spawn_inventory(
    commands: &mut Commands,
    theme: &UiTheme,
    content: &Content,
    framed: Option<ItemState>,
) {
    let slot = theme.slot_size;
    let gap = theme.slot_gap;
    let width = COLUMNS as f32 * slot + (COLUMNS - 1) as f32 * gap;
    // As many rows as there are items for, up to the most that show.
    let rows = content
        .item_states
        .len()
        .div_ceil(COLUMNS)
        .clamp(1, VISIBLE_ROWS);
    let height = rows as f32 * slot + (rows - 1) as f32 * gap;
    commands
        .spawn((
            Name::new("Inventory"),
            InventoryScreen,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::bottom(px(hotbar_height(theme) + ABOVE_HOTBAR)),
                ..default()
            },
            BackgroundColor(theme.backdrop),
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
                    Text::new("Items"),
                    TextFont::from_font_size(theme.title_font_size),
                    TextColor(theme.text),
                ));
                scroll_panel(
                    panel,
                    theme,
                    Node {
                        width: px(width),
                        height: px(height),
                        flex_wrap: FlexWrap::Wrap,
                        align_content: AlignContent::FlexStart,
                        column_gap: px(gap),
                        row_gap: px(gap),
                        ..default()
                    },
                    slot + gap,
                    (InventoryGrid, Name::new("Inventory grid")),
                    |grid| {
                        for index in 0..content.item_states.len() {
                            let item = ItemState(index as u32);
                            grid.spawn((
                                CatalogSlot(item),
                                item_slot(
                                    theme,
                                    ItemSlot {
                                        item: Some(item),
                                        selected: framed == Some(item),
                                    },
                                ),
                            ));
                        }
                    },
                );
                panel.spawn((
                    Text::new("Click an item to put it in the selected hotbar slot"),
                    TextFont::from_font_size(theme.font_size - 2.0),
                    TextColor(theme.text_dim),
                ));
            });
        });
}

/// Clicking an item puts it in the active hotbar slot.
fn pick_item(
    click: On<Clicked>,
    slots: Query<&CatalogSlot>,
    active: Res<ActiveSlot>,
    mut hotbar: ResMut<Hotbar>,
) {
    if click.button != MouseButton::Left {
        return;
    }
    if let Ok(&CatalogSlot(item)) = slots.get(click.entity) {
        hotbar.0[active.0] = Some(item);
    }
}

/// Frames the grid slot holding the active hotbar slot's item.
fn frame_active_item(
    hotbar: Res<Hotbar>,
    active: Res<ActiveSlot>,
    mut slots: Query<(&CatalogSlot, &mut ItemSlot)>,
) {
    if !hotbar.is_changed() && !active.is_changed() {
        return;
    }
    let item = hotbar.active(*active);
    for (&CatalogSlot(id), mut slot) in &mut slots {
        let selected = item == Some(id);
        if slot.selected != selected {
            slot.selected = selected;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fly_camera::FlyCamera,
        headless::{Script, run_more, settle, test_game},
        hotbar::HotbarSlot,
        player::Player,
    };
    use bevy::{
        ui::UiGlobalTransform,
        ui_widgets::Scrollbar,
        window::{CursorGrabMode, CursorOptions},
    };

    fn is_open(app: &mut App) -> bool {
        let mut screens = app.world_mut().query::<&InventoryScreen>();
        screens.iter(app.world()).count() > 0
    }

    fn cursor(app: &mut App) -> CursorGrabMode {
        let mut cursors = app.world_mut().query::<&CursorOptions>();
        cursors.single(app.world()).unwrap().grab_mode
    }

    /// Where on screen the grid slot showing `item` is, in the script's
    /// pixels (the first, if several show it).
    fn slot_at(app: &mut App, item: ItemState) -> Vec2 {
        let mut slots = app
            .world_mut()
            .query::<(&CatalogSlot, &UiGlobalTransform)>();
        slots
            .iter(app.world())
            .find(|(slot, _)| slot.0 == item)
            .expect("no slot for the item")
            .1
            .translation
    }

    fn hotbar_slot_at(app: &mut App, index: usize) -> Vec2 {
        let mut slots = app.world_mut().query::<(&HotbarSlot, &UiGlobalTransform)>();
        slots
            .iter(app.world())
            .find(|(slot, _)| slot.0 == index)
            .unwrap()
            .1
            .translation
    }

    /// The grid slots framed as the active hotbar slot's item.
    fn framed(app: &mut App) -> Vec<ItemState> {
        let mut slots = app.world_mut().query::<(&CatalogSlot, &ItemSlot)>();
        let mut framed: Vec<ItemState> = slots
            .iter(app.world())
            .filter(|(_, slot)| slot.selected)
            .map(|(catalog, _)| catalog.0)
            .collect();
        framed.dedup();
        framed
    }

    fn scrolled(app: &mut App) -> f32 {
        let mut grids = app
            .world_mut()
            .query_filtered::<&ScrollPosition, With<InventoryGrid>>();
        grids.single(app.world()).unwrap().y
    }

    fn scrollbar_shown(app: &mut App) -> bool {
        let mut bars = app.world_mut().query_filtered::<&Node, With<Scrollbar>>();
        bars.single(app.world()).unwrap().display != Display::None
    }

    fn camera(app: &mut App) -> Vec3 {
        let mut cameras = app
            .world_mut()
            .query_filtered::<&Transform, With<FlyCamera>>();
        cameras.single(app.world()).unwrap().translation
    }

    fn theme_half_slot(app: &App) -> f32 {
        app.world().resource::<UiTheme>().slot_size / 2.0
    }

    fn flying(app: &mut App) -> bool {
        let mut players = app.world_mut().query::<&Player>();
        players.single(app.world()).unwrap().flying
    }

    fn click(button: &str, at: Vec2) -> String {
        format!("cursor {} {}\nwait 1\nhold {button} 1\nwait 1", at.x, at.y)
    }

    /// Through the real game and input: E opens the inventory and frees the
    /// cursor, the player stops moving, clicking an item fills the active
    /// slot, the wheel scrolls the grid, and E closes it again.
    #[test]
    fn the_inventory_fills_the_active_hotbar_slot() {
        let mut app = test_game(Script::parse("camera 64.5 70 92.5 0 0\nwait 2").unwrap());
        // Plenty of items, so the grid scrolls: the base game's, then
        // copies of planks.
        let (planks, stone) = {
            let mut content = app.world_mut().resource_mut::<Content>();
            let planks = content.default_item("defaria:planks").unwrap();
            let copy = content.item_state(planks).unwrap().clone();
            content.item_states.extend(std::iter::repeat_n(copy, 100));
            (planks, content.default_item("defaria:stone").unwrap())
        };
        settle(&mut app);
        run_more(&mut app, "hold MouseLeft 1\nwait 1");
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
        assert!(flying(&mut app));

        run_more(&mut app, "hold KeyE 1\nwait 3");
        assert!(is_open(&mut app));
        assert_eq!(cursor(&mut app), CursorGrabMode::None);
        // Slot 1 holds stone, so stone is framed in the grid.
        assert_eq!(framed(&mut app), [stone]);
        assert!(scrollbar_shown(&mut app));

        // Walking keys and double-tapping Space do nothing now.
        let before = camera(&mut app);
        run_more(
            &mut app,
            "hold KeyW 20\nhold Space 2\nwait 2\nhold Space 2\nwait 2",
        );
        assert_eq!(camera(&mut app), before);
        assert!(flying(&mut app));

        // Slot 3, then right click planks: nothing. Left click: it goes in
        // slot 3, and the frame moves to it.
        let at = slot_at(&mut app, planks);
        run_more(&mut app, "hold Digit3 1\nwait 1");
        run_more(&mut app, &click("MouseRight", at));
        assert_ne!(app.world().resource::<Hotbar>().0[2], Some(planks));
        run_more(&mut app, &click("MouseLeft", at));
        assert_eq!(app.world().resource::<Hotbar>().0[2], Some(planks));
        assert_eq!(*app.world().resource::<ActiveSlot>(), ActiveSlot(2));
        assert_eq!(framed(&mut app), [planks]);
        // Clicking in the inventory didn't lock the cursor.
        assert_eq!(cursor(&mut app), CursorGrabMode::None);

        // Clicking a hotbar slot makes it active; right clicking empties it.
        let slot_5 = hotbar_slot_at(&mut app, 4);
        run_more(&mut app, &click("MouseLeft", slot_5));
        assert_eq!(*app.world().resource::<ActiveSlot>(), ActiveSlot(4));
        run_more(&mut app, &click("MouseRight", slot_5));
        assert_eq!(app.world().resource::<Hotbar>().0[4], None);
        assert_eq!(framed(&mut app), []);

        // The wheel over the grid scrolls it, and the slots move with it.
        run_more(
            &mut app,
            &format!("cursor {} {}\nscroll -2\nwait 2", at.x, at.y),
        );
        let theme = app.world().resource::<UiTheme>();
        let row = theme.slot_size + theme.slot_gap;
        assert_eq!(scrolled(&mut app), 2.0 * row);
        assert!(slot_at(&mut app, planks).y < at.y);
        // The wheel over the grid leaves the hotbar slot alone.
        assert_eq!(*app.world().resource::<ActiveSlot>(), ActiveSlot(4));

        // At the bottom: the row just above the view is scrolled out of
        // it, behind the panel's title. Clicking there does nothing; the
        // last item (a copy) can be clicked.
        let last = ItemState(app.world().resource::<Content>().item_states.len() as u32 - 1);
        run_more(&mut app, "scroll -100\nwait 2");
        let rows_scrolled = (scrolled(&mut app) / row).round() as usize;
        let hidden = slot_at(&mut app, ItemState(((rows_scrolled - 1) * COLUMNS) as u32));
        let grid_top = slot_at(&mut app, ItemState((rows_scrolled * COLUMNS) as u32)).y
            - theme_half_slot(&app);
        assert!(
            hidden.y > 0.0 && hidden.y < grid_top,
            "{hidden} above {grid_top}"
        );
        run_more(&mut app, &click("MouseLeft", hidden));
        assert_eq!(app.world().resource::<Hotbar>().0[4], None);
        let bottom = slot_at(&mut app, last);
        run_more(&mut app, &click("MouseLeft", bottom));
        assert_eq!(app.world().resource::<Hotbar>().0[4], Some(last));

        run_more(&mut app, "hold KeyE 1\nwait 3");
        assert!(!is_open(&mut app));
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
        let mut grids = app.world_mut().query::<&InventoryGrid>();
        assert_eq!(grids.iter(app.world()).count(), 0);
    }

    /// Escape closes it too and locks the cursor again.
    #[test]
    fn escape_closes_the_inventory() {
        let mut app = test_game(Script::parse("wait 1").unwrap());
        settle(&mut app);
        run_more(&mut app, "hold MouseLeft 1\nwait 1\nhold KeyE 1\nwait 2");
        assert!(is_open(&mut app));
        run_more(&mut app, "hold Escape 1\nwait 2");
        assert!(!is_open(&mut app));
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
    }

    /// E doesn't open the inventory over another screen.
    #[test]
    fn e_does_nothing_while_another_screen_is_open() {
        let mut app = test_game(Script::parse("wait 1").unwrap());
        settle(&mut app);
        app.world_mut().spawn(UiScreen);
        run_more(&mut app, "hold KeyE 1\nwait 2");
        assert!(!is_open(&mut app));
    }
}
