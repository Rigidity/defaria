//! The inventory screen: every item the mods define, in a scrolling grid.
//!
//! - E opens and closes it; Escape closes it too (see [`crate::ui::screen`]).
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
    content::{Content, ItemId},
    hotbar::{ActiveSlot, Hotbar},
    ui::{Clicked, ItemSlot, UiScreen, UiTheme, item_slot, scroll_panel},
};

/// Slots in a row of the grid.
pub const COLUMNS: usize = 9;
/// The most rows that show at once; more scroll.
pub const VISIBLE_ROWS: usize = 5;
/// Room left at the bottom of the screen for the hotbar, in logical pixels.
const HOTBAR_ROOM: f32 = 120.0;

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<CatalogSlot>()
            .add_systems(Update, (toggle_inventory, frame_active_item))
            .add_systems(OnEnter(UiScreen::Inventory), spawn_inventory);
    }
}

/// A slot of the inventory grid, showing its item.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct CatalogSlot(pub ItemId);

/// The scrolling grid of slots.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct InventoryGrid;

fn toggle_inventory(
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<State<UiScreen>>,
    mut next: ResMut<NextState<UiScreen>>,
) {
    if !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    match screen.get() {
        UiScreen::Closed => next.set(UiScreen::Inventory),
        UiScreen::Inventory => next.set(UiScreen::Closed),
    }
}

fn spawn_inventory(mut commands: Commands, theme: Res<UiTheme>, content: Res<Content>) {
    let slot = theme.slot_size;
    let gap = theme.slot_gap;
    let width = COLUMNS as f32 * slot + (COLUMNS - 1) as f32 * gap;
    // As many rows as there are items for, up to the most that show.
    let rows = content.items.len().div_ceil(COLUMNS).clamp(1, VISIBLE_ROWS);
    let height = rows as f32 * slot + (rows - 1) as f32 * gap;
    commands
        .spawn((
            Name::new("Inventory"),
            DespawnOnExit(UiScreen::Inventory),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::bottom(px(HOTBAR_ROOM)),
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
                let (_, grid) = scroll_panel(
                    panel,
                    &theme,
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
                );
                panel.commands().entity(grid).with_children(|grid| {
                    for index in 0..content.items.len() {
                        let item = ItemId(index);
                        grid.spawn((
                            CatalogSlot(item),
                            item_slot(
                                &theme,
                                ItemSlot {
                                    item: Some(item),
                                    selected: false,
                                },
                            ),
                        ))
                        .observe(pick_item);
                    }
                });
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
    added: Query<(), Added<CatalogSlot>>,
) {
    if !hotbar.is_changed() && !active.is_changed() && added.is_empty() {
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
    };
    use bevy::{
        ui::UiGlobalTransform,
        window::{CursorGrabMode, CursorOptions},
    };

    fn screen(app: &App) -> UiScreen {
        *app.world().resource::<State<UiScreen>>().get()
    }

    fn cursor(app: &mut App) -> CursorGrabMode {
        let mut cursors = app.world_mut().query::<&CursorOptions>();
        cursors.single(app.world()).unwrap().grab_mode
    }

    /// Where on screen the grid slot showing `item` is, in the script's
    /// pixels. The layout is a frame old, so update first.
    fn slot_at(app: &mut App, item: ItemId) -> Vec2 {
        let mut slots = app
            .world_mut()
            .query::<(&CatalogSlot, &UiGlobalTransform)>();
        let (_, transform) = slots
            .iter(app.world())
            .find(|(slot, _)| slot.0 == item)
            .expect("no slot for the item");
        transform.translation
    }

    fn scrolled(app: &mut App) -> f32 {
        let mut grids = app
            .world_mut()
            .query_filtered::<&ScrollPosition, With<InventoryGrid>>();
        grids.single(app.world()).unwrap().y
    }

    fn camera(app: &mut App) -> Vec3 {
        let mut cameras = app
            .world_mut()
            .query_filtered::<&Transform, With<FlyCamera>>();
        cameras.single(app.world()).unwrap().translation
    }

    /// Through the real game and input: E opens the inventory and frees the
    /// cursor, the player stops moving, clicking an item fills the active
    /// slot, the wheel scrolls the grid, and E closes it again.
    #[test]
    fn the_inventory_fills_the_active_hotbar_slot() {
        let mut app = test_game(Script::parse("camera 64.5 70 92.5 0 0\nwait 2").unwrap());
        // Plenty of items, so the grid scrolls: the base game's, then
        // copies of planks.
        let planks = {
            let mut content = app.world_mut().resource_mut::<Content>();
            let planks = content.item_id("defaria:planks").unwrap();
            let copy = content.items[planks.0].clone();
            content.items.extend(std::iter::repeat_n(copy, 100));
            planks
        };
        settle(&mut app);
        run_more(&mut app, "hold MouseLeft 1\nwait 1");
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);

        run_more(&mut app, "hold KeyE 1\nwait 3");
        assert_eq!(screen(&app), UiScreen::Inventory);
        assert_eq!(cursor(&mut app), CursorGrabMode::None);

        // Walking keys do nothing now.
        let before = camera(&mut app);
        run_more(&mut app, "hold KeyW 20\nwait 1");
        assert_eq!(camera(&mut app), before);

        // Slot 3, then click planks: it goes in slot 3.
        let at = slot_at(&mut app, planks);
        run_more(
            &mut app,
            &format!(
                "hold Digit3 1\ncursor {} {}\nwait 1\nhold MouseLeft 1\nwait 1",
                at.x, at.y
            ),
        );
        let hotbar = app.world().resource::<Hotbar>();
        assert_eq!(hotbar.0[2], Some(planks));
        assert_eq!(*app.world().resource::<ActiveSlot>(), ActiveSlot(2));
        // Clicking in the inventory didn't lock the cursor.
        assert_eq!(cursor(&mut app), CursorGrabMode::None);

        // The wheel over the grid scrolls it, and the slots move with it.
        run_more(&mut app, "scroll -2\nwait 2");
        let rows = scrolled(&mut app);
        let line = app.world().resource::<UiTheme>();
        assert_eq!(rows, 2.0 * (line.slot_size + line.slot_gap));
        assert!(slot_at(&mut app, planks).y < at.y);
        // The last item (a copy) comes into view at the bottom.
        let last = ItemId(app.world().resource::<Content>().items.len() - 1);
        run_more(&mut app, "scroll -100\nwait 2");
        let bottom = slot_at(&mut app, last);
        run_more(
            &mut app,
            &format!(
                "cursor {} {}\nwait 1\nhold MouseLeft 1\nwait 1",
                bottom.x, bottom.y
            ),
        );
        assert_eq!(app.world().resource::<Hotbar>().0[2], Some(last));

        run_more(&mut app, "hold KeyE 1\nwait 3");
        assert_eq!(screen(&app), UiScreen::Closed);
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
        let mut grids = app.world_mut().query::<&InventoryGrid>();
        assert_eq!(grids.iter(app.world()).count(), 0);
    }

    /// Escape closes it too.
    #[test]
    fn escape_closes_the_inventory() {
        let mut app = test_game(Script::parse("wait 1").unwrap());
        settle(&mut app);
        run_more(&mut app, "hold KeyE 1\nwait 2");
        assert_eq!(screen(&app), UiScreen::Inventory);
        run_more(&mut app, "hold Escape 1\nwait 2");
        assert_eq!(screen(&app), UiScreen::Closed);
    }
}
