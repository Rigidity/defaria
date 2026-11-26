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

use bevy::{prelude::*, ui_widgets::ValueChange};

use super::{
    Inventory,
    list::{ItemList, ListFilter, ListSource, ListSystems, Owner, item_list},
    search::ItemQuery,
};
use crate::{
    content::Content,
    game_mode::GameMode,
    hotbar::hotbar_height,
    player::LocalPlayer,
    ui::{UiScreen, UiTheme, text_field, typing},
};

/// The catalog's rows that show at once; more scroll.
pub const CATALOG_ROWS: usize = 4;
/// The inventory's rows that show at once; more scroll.
pub const INVENTORY_ROWS: usize = 3;
/// Space between the inventory's room and the hotbar, in logical pixels.
const ABOVE_HOTBAR: f32 = 12.0;

pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<InventoryScreen>()
            .register_type::<WeightLabel>()
            .register_type::<SearchBox>()
            .add_observer(search_lists)
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
fn panel_width(theme: &UiTheme) -> f32 {
    let columns = super::list::COLUMNS as f32;
    columns * theme.slot_size
        + (columns - 1.0) * theme.slot_gap
        + theme.scrollbar_gap
        + theme.scrollbar_width
}

/// A screen's search box.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct SearchBox;

/// Typing in a search box narrows the lists on its screen to what
/// matches.
fn search_lists(
    change: On<ValueChange<String>>,
    boxes: Query<(), With<SearchBox>>,
    screens: Query<(), With<UiScreen>>,
    parents: Query<&ChildOf>,
    mut lists: Query<(Entity, &mut ListFilter)>,
) {
    if !boxes.contains(change.source) {
        return;
    }
    let Some(screen) = parents
        .iter_ancestors(change.source)
        .find(|&entity| screens.contains(entity))
    else {
        return;
    };
    let query = ItemQuery::parse(&change.value);
    for (list, mut filter) in &mut lists {
        if parents.iter_ancestors(list).any(|entity| entity == screen) {
            filter.set_if_neq(ListFilter(query.clone()));
        }
    }
}

/// The line showing how much the player carries.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct WeightLabel;

fn toggle_inventory(
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
fn heading(theme: &UiTheme, title: &str, extra: impl Bundle, above: f32) -> impl Bundle {
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
fn spawn_inventory(commands: &mut Commands, theme: &UiTheme, creative: bool) {
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
fn show_weight(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content::ItemState,
        fly_camera::FlyCamera,
        headless::{Script, run_more, settle, test_game},
        hotbar::{ActiveSlot, Hotbar, HotbarSlot},
        inventory::{
            gesture::{CATALOG_BATCH, CATALOG_HALF},
            list::{ItemList, ListEntry},
            player::STARTING_COUNT,
        },
        player::Player,
    };
    use bevy::{
        ui::UiGlobalTransform,
        ui_widgets::Scrollbar,
        window::{CursorGrabMode, CursorOptions},
    };

    fn inventory(app: &mut App) -> Inventory {
        let mut players = app
            .world_mut()
            .query_filtered::<&Inventory, With<LocalPlayer>>();
        players.single(app.world()).unwrap().clone()
    }

    fn hotbar(app: &mut App) -> Hotbar {
        let mut players = app
            .world_mut()
            .query_filtered::<&Hotbar, With<LocalPlayer>>();
        players.single(app.world()).unwrap().clone()
    }

    fn active(app: &mut App) -> ActiveSlot {
        let mut players = app
            .world_mut()
            .query_filtered::<&ActiveSlot, With<LocalPlayer>>();
        *players.single(app.world()).unwrap()
    }

    fn is_open(app: &mut App) -> bool {
        let mut screens = app.world_mut().query::<&InventoryScreen>();
        screens.iter(app.world()).count() > 0
    }

    fn cursor(app: &mut App) -> CursorGrabMode {
        let mut cursors = app.world_mut().query::<&CursorOptions>();
        cursors.single(app.world()).unwrap().grab_mode
    }

    /// Where on screen the entry for `item` in the list of `source` is.
    fn entry_at(app: &mut App, source: ListSource, item: ItemState) -> Vec2 {
        let mut entries = app.world_mut().query::<(&ListEntry, &UiGlobalTransform)>();
        entries
            .iter(app.world())
            .find(|(entry, _)| entry.source == source && entry.stack.item == item)
            .expect("no entry for the item")
            .1
            .translation
    }

    fn catalog_at(app: &mut App, item: ItemState) -> Vec2 {
        entry_at(app, ListSource::Catalog, item)
    }

    fn mine_at(app: &mut App, item: ItemState) -> Vec2 {
        entry_at(app, ListSource::Inventory(Owner::LocalPlayer), item)
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

    /// The middle of the player's inventory list.
    fn my_list_at(app: &mut App) -> Vec2 {
        let mut lists = app.world_mut().query::<(&ItemList, &UiGlobalTransform)>();
        lists
            .iter(app.world())
            .find(|(list, _)| list.source == ListSource::Inventory(Owner::LocalPlayer))
            .unwrap()
            .1
            .translation
    }

    fn scrolled(app: &mut App) -> f32 {
        let mut lists = app.world_mut().query::<(&ItemList, &ScrollPosition)>();
        lists
            .iter(app.world())
            .find(|(list, _)| list.source == ListSource::Catalog)
            .unwrap()
            .1
            .y
    }

    fn scrollbars_shown(app: &mut App) -> usize {
        let mut bars = app
            .world_mut()
            .query_filtered::<&Visibility, With<Scrollbar>>();
        bars.iter(app.world())
            .filter(|visibility| **visibility != Visibility::Hidden)
            .count()
    }

    fn camera(app: &mut App) -> Vec3 {
        let mut cameras = app
            .world_mut()
            .query_filtered::<&Transform, With<FlyCamera>>();
        cameras.single(app.world()).unwrap().translation
    }

    fn flying(app: &mut App) -> bool {
        let mut players = app.world_mut().query::<&Player>();
        players.single(app.world()).unwrap().flying
    }

    fn click(button: &str, at: Vec2) -> String {
        format!("cursor {} {}\nwait 1\nhold {button} 1\nwait 1", at.x, at.y)
    }

    /// Pressing at `from`, moving to `to` and letting go there.
    fn drag(button: &str, from: Vec2, to: Vec2) -> String {
        format!(
            "cursor {} {}\nwait 1\npress {button}\nwait 1\ncursor {} {}\nwait 1\ncursor {} {}\nwait 1\nrelease {button}\nwait 2",
            from.x,
            from.y,
            (from.x + to.x) / 2.0,
            (from.y + to.y) / 2.0,
            to.x,
            to.y
        )
    }

    /// A game with the inventory open, the cursor locked first so closing
    /// it locks it again.
    fn open_inventory() -> App {
        let mut app = test_game(Script::parse("camera 64.5 70 92.5 0 0\nwait 2").unwrap());
        settle(&mut app);
        run_more(&mut app, "hold MouseLeft 1\nwait 1\nhold KeyE 1\nwait 3");
        assert!(is_open(&mut app));
        app
    }

    fn item(app: &App, id: &str) -> ItemState {
        app.world().resource::<Content>().default_item(id).unwrap()
    }

    /// The item for plain soil of `material` (`defaria:sand`, say).
    fn soil(app: &App, material: &str) -> ItemState {
        let content = app.world().resource::<Content>();
        let soil = content.item("defaria:soil").unwrap();
        soil.state(&[("soil", material)]).unwrap()
    }

    /// E opens it and frees the cursor, the player stops moving, the
    /// catalog scrolls under the wheel, and E closes it again.
    #[test]
    fn e_opens_and_closes_the_inventory() {
        let mut app = test_game(Script::parse("camera 64.5 70 92.5 0 0\nwait 2").unwrap());
        // Plenty of items, so the catalog scrolls: the base game's, then
        // copies of planks.
        {
            let mut content = app.world_mut().resource_mut::<Content>();
            let planks = content.default_item("defaria:planks").unwrap();
            let copy = content.item_state(planks).unwrap().clone();
            content.item_states.extend(std::iter::repeat_n(copy, 100));
        }
        settle(&mut app);
        run_more(&mut app, "hold MouseLeft 1\nwait 1");
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
        assert!(flying(&mut app));

        run_more(&mut app, "hold KeyE 1\nwait 3");
        assert!(is_open(&mut app));
        assert_eq!(cursor(&mut app), CursorGrabMode::None);
        assert!(scrollbars_shown(&mut app) >= 1);

        // Walking keys and double-tapping Space do nothing now.
        let before = camera(&mut app);
        run_more(
            &mut app,
            "hold KeyW 20\nhold Space 2\nwait 2\nhold Space 2\nwait 2",
        );
        assert_eq!(camera(&mut app), before);
        assert!(flying(&mut app));

        // The wheel over the catalog scrolls it, and leaves the hotbar's
        // active slot alone.
        let stone = item(&app, "defaria:stone");
        let at = catalog_at(&mut app, stone);
        run_more(
            &mut app,
            &format!("cursor {} {}\nscroll -2\nwait 2", at.x, at.y),
        );
        let theme = app.world().resource::<UiTheme>();
        let row = theme.slot_size + theme.slot_gap;
        assert_eq!(scrolled(&mut app), 2.0 * row);
        assert_eq!(active(&mut app), ActiveSlot(0));

        run_more(&mut app, "hold KeyE 1\nwait 3");
        assert!(!is_open(&mut app));
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
    }

    /// Clicks move items between the catalog and the inventory: one, a
    /// batch with Shift, a few with the right button; and back into the
    /// catalog, which throws them away.
    #[test]
    fn clicks_move_items_in_and_out() {
        let mut app = open_inventory();
        let sand = soil(&app, "defaria:sand");
        let glass = soil(&app, "defaria:gravel");
        let start = inventory(&mut app).count(sand);
        let at = catalog_at(&mut app, sand);
        run_more(&mut app, &click("MouseLeft", at));
        assert_eq!(inventory(&mut app).count(sand), start + 1);
        run_more(&mut app, &click("MouseRight", at));
        assert_eq!(inventory(&mut app).count(sand), start + 1 + CATALOG_HALF);
        run_more(&mut app, "press ShiftLeft\nwait 1");
        run_more(&mut app, &click("MouseLeft", at));
        run_more(&mut app, "release ShiftLeft\nwait 1");
        let have = start + 1 + CATALOG_HALF + CATALOG_BATCH;
        assert_eq!(inventory(&mut app).count(sand), have);

        // Something new shows up in the list.
        let at = catalog_at(&mut app, glass);
        run_more(&mut app, &click("MouseLeft", at));
        assert_eq!(inventory(&mut app).count(glass), 1);

        // From the inventory: one, half, then all, into the catalog.
        let mine = mine_at(&mut app, sand);
        run_more(&mut app, &click("MouseLeft", mine));
        assert_eq!(inventory(&mut app).count(sand), have - 1);
        run_more(&mut app, &click("MouseRight", mine));
        let left = have - 1 - (have - 1).div_ceil(2);
        assert_eq!(inventory(&mut app).count(sand), left);
        let mine = mine_at(&mut app, sand);
        run_more(&mut app, "press ShiftLeft\nwait 1");
        run_more(&mut app, &click("MouseLeft", mine));
        run_more(&mut app, "release ShiftLeft\nwait 1");
        assert_eq!(inventory(&mut app).count(sand), 0);
        // Out of sand: its hotbar slot stays, at 0.
        assert_eq!(hotbar(&mut app).0[3], Some(sand));
    }

    /// Dragging: catalog items onto the inventory list (a batch) and onto a
    /// hotbar slot (it goes there); hotbar slots onto each other (they
    /// swap) and off the hotbar (cleared); and the number keys over an
    /// entry put it in the hotbar.
    #[test]
    fn dragging_moves_items_and_fills_the_hotbar() {
        let mut app = open_inventory();
        let (glass, stone, dirt) = (
            soil(&app, "defaria:gravel"),
            item(&app, "defaria:stone"),
            soil(&app, "defaria:dirt"),
        );
        let (from, to) = (catalog_at(&mut app, glass), my_list_at(&mut app));
        run_more(&mut app, &drag("MouseLeft", from, to));
        assert_eq!(inventory(&mut app).count(glass), CATALOG_BATCH);

        let slot_9 = hotbar_slot_at(&mut app, 8);
        run_more(&mut app, &drag("MouseLeft", from, slot_9));
        assert_eq!(hotbar(&mut app).0[8], Some(glass));
        assert_eq!(inventory(&mut app).count(glass), CATALOG_BATCH);

        let (slot_1, slot_2) = (hotbar_slot_at(&mut app, 0), hotbar_slot_at(&mut app, 1));
        run_more(&mut app, &drag("MouseLeft", slot_1, slot_2));
        assert_eq!(hotbar(&mut app).0[..2], [Some(dirt), Some(stone)]);
        run_more(&mut app, &drag("MouseLeft", slot_1, to));
        assert_eq!(hotbar(&mut app).0[0], None);
        assert_eq!(inventory(&mut app).count(dirt), STARTING_COUNT);

        // Digit 3 over the glass in the inventory: slot 3 gets glass (and
        // slot 9 loses it), and the active slot (1) stays.
        let mine = mine_at(&mut app, glass);
        run_more(
            &mut app,
            &format!(
                "cursor {} {}\nwait 1\nhold Digit3 1\nwait 2",
                mine.x, mine.y
            ),
        );
        assert_eq!(hotbar(&mut app).0[2], Some(glass));
        assert_eq!(hotbar(&mut app).0[8], None);
        assert_eq!(active(&mut app), ActiveSlot(0));
    }

    /// A drag that comes back where it started isn't a click: nothing
    /// moves. A click that wobbles a pixel or two still is one.
    #[test]
    fn a_drag_back_to_the_start_moves_nothing() {
        let mut app = open_inventory();
        let sand = soil(&app, "defaria:sand");
        let start = inventory(&mut app).count(sand);
        let at = catalog_at(&mut app, sand);
        let away = at + Vec2::new(0.0, 200.0);
        run_more(
            &mut app,
            &format!(
                // A first small step, as a real mouse makes, then away and
                // back.
                "cursor {} {}\nwait 1\npress MouseRight\nwait 1\ncursor {} {}\nwait 1\ncursor {} {}\nwait 1\ncursor {} {}\nwait 1\nrelease MouseRight\nwait 2",
                at.x,
                at.y,
                at.x + 2.0,
                at.y + 1.0,
                away.x,
                away.y,
                at.x,
                at.y
            ),
        );
        assert_eq!(inventory(&mut app).count(sand), start);
        run_more(
            &mut app,
            &format!(
                "cursor {} {}\nwait 1\npress MouseLeft\nwait 1\ncursor {} {}\nwait 1\nrelease MouseLeft\nwait 2",
                at.x,
                at.y,
                at.x + 2.0,
                at.y + 1.0
            ),
        );
        assert_eq!(inventory(&mut app).count(sand), start + 1);
    }

    /// Dragging hotbar slots: a right drag does nothing (not even away and
    /// back), and a left drag that swaps two slots leaves the active slot
    /// as it was.
    #[test]
    fn dragging_hotbar_slots_keeps_what_is_in_hand() {
        let mut app = open_inventory();
        let before = hotbar(&mut app);
        let (slot_3, slot_7) = (hotbar_slot_at(&mut app, 2), hotbar_slot_at(&mut app, 6));
        run_more(&mut app, &drag("MouseRight", slot_3, slot_7));
        let away = slot_3 - Vec2::new(0.0, 200.0);
        run_more(
            &mut app,
            &format!(
                "cursor {} {}\nwait 1\npress MouseRight\nwait 1\ncursor {} {}\nwait 1\ncursor {} {}\nwait 1\nrelease MouseRight\nwait 2",
                slot_3.x, slot_3.y, away.x, away.y, slot_3.x, slot_3.y
            ),
        );
        assert_eq!(hotbar(&mut app), before);
        let (slot_1, slot_8) = (hotbar_slot_at(&mut app, 0), hotbar_slot_at(&mut app, 7));
        run_more(&mut app, &drag("MouseLeft", slot_8, slot_1));
        assert_eq!(hotbar(&mut app).0[0], before.0[7]);
        assert_eq!(hotbar(&mut app).0[7], before.0[0]);
        assert_eq!(active(&mut app), ActiveSlot(0));
    }

    /// A click on a hotbar slot that moves a pixel or two on the frame
    /// it's let go is still a click: the slot is made active, not cleared.
    #[test]
    fn a_jittery_click_keeps_the_hotbar_slot() {
        let mut app = open_inventory();
        let before = hotbar(&mut app);
        let slot_3 = hotbar_slot_at(&mut app, 2);
        run_more(
            &mut app,
            &format!(
                "cursor {} {}\nwait 1\npress MouseLeft\nwait 1\ncursor {} {}\nrelease MouseLeft\nwait 2",
                slot_3.x,
                slot_3.y,
                slot_3.x + 2.0,
                slot_3.y
            ),
        );
        assert_eq!(hotbar(&mut app), before);
        assert_eq!(active(&mut app), ActiveSlot(2));
    }

    /// The lists are full on the first frame the screen shows.
    #[test]
    fn lists_are_full_when_the_screen_opens() {
        let mut app = test_game(Script::parse("wait 1").unwrap());
        settle(&mut app);
        run_more(&mut app, "hold MouseLeft 1\nwait 1\npress KeyE");
        assert!(is_open(&mut app));
        let mut entries = app.world_mut().query::<&ListEntry>();
        assert!(entries.iter(app.world()).count() > 0);
    }

    /// With the screen open, clicking a hotbar slot makes it active, and
    /// right clicking clears it.
    #[test]
    fn hotbar_slots_are_clicked_in_the_screen() {
        let mut app = open_inventory();
        let slot_3 = hotbar_slot_at(&mut app, 2);
        run_more(&mut app, &click("MouseLeft", slot_3));
        assert_eq!(active(&mut app), ActiveSlot(2));
        assert!(hotbar(&mut app).0[2].is_some());
        run_more(&mut app, &click("MouseRight", slot_3));
        assert_eq!(hotbar(&mut app).0[2], None);
    }

    /// Escape closes it too and locks the cursor again.
    #[test]
    fn escape_closes_the_inventory() {
        let mut app = open_inventory();
        run_more(&mut app, "hold Escape 1\nwait 2");
        assert!(!is_open(&mut app));
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
    }

    /// While playing, the pointer doesn't reach the HUD: right clicking
    /// with the cursor over a hotbar slot doesn't clear it.
    #[test]
    fn the_hotbar_isnt_clicked_while_playing() {
        let mut app = test_game(Script::parse("wait 1").unwrap());
        settle(&mut app);
        let before = hotbar(&mut app);
        let slot_2 = hotbar_slot_at(&mut app, 1);
        run_more(&mut app, &click("MouseLeft", slot_2));
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
        run_more(&mut app, &click("MouseRight", slot_2));
        assert_eq!(hotbar(&mut app), before);
        let mut slots = app
            .world_mut()
            .query::<(&HotbarSlot, &bevy::picking::hover::Hovered)>();
        assert!(slots.iter(app.world()).all(|(_, hovered)| !hovered.get()));
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

    /// In survival the screen is just what's carried: no catalog, and
    /// clicking an entry moves nothing.
    #[test]
    fn survival_has_no_catalog() {
        let mut app = test_game(Script::parse("wait 1").unwrap());
        settle(&mut app);
        run_more(
            &mut app,
            "hold MouseLeft 1\nwait 1\nhold KeyC 1\nwait 2\nhold KeyE 1\nwait 3",
        );
        assert!(is_open(&mut app));
        let mut lists = app.world_mut().query::<&ItemList>();
        let sources: Vec<ListSource> = lists.iter(app.world()).map(|list| list.source).collect();
        assert_eq!(sources, [ListSource::Inventory(Owner::LocalPlayer)]);
        let before = inventory(&mut app);
        let stone = item(&app, "defaria:stone");
        let at = mine_at(&mut app, stone);
        run_more(&mut app, &click("MouseLeft", at));
        assert_eq!(inventory(&mut app), before);
    }

    /// Both lists group their items under headings: the catalog all four
    /// of the base game's, the inventory those of what's carried.
    /// A list's children in the order they're drawn: `#Name` for a
    /// heading, the item's name for an entry; and their entities.
    fn shown(app: &mut App, source: ListSource) -> (Vec<String>, Vec<Entity>) {
        let mut lists = app.world_mut().query::<(&ItemList, &Children)>();
        let children: Vec<Entity> = lists
            .iter(app.world())
            .find(|(list, _)| list.source == source)
            .map(|(_, children)| children.iter().collect())
            .unwrap();
        let content = app.world().resource::<Content>();
        let names = children
            .iter()
            .map(|&child| {
                if let Some(entry) = app.world().get::<ListEntry>(child) {
                    content.item_name(entry.stack.item).unwrap().to_owned()
                } else {
                    format!("#{}", app.world().get::<Text>(child).unwrap().0)
                }
            })
            .collect();
        (names, children)
    }

    /// What the player's list should show, from the model.
    fn expected(app: &mut App) -> Vec<String> {
        let content = app.world().resource::<Content>().clone();
        let entries = crate::inventory::list::sorted_entries(&inventory(app));
        crate::inventory::list::sections(&content, entries)
            .into_iter()
            .flat_map(|section| {
                let heading = section.category.map_or("Other".to_owned(), |index| {
                    content.categories[index].name.clone()
                });
                std::iter::once(format!("#{heading}")).chain(
                    section
                        .entries
                        .into_iter()
                        .map(|stack| content.item_name(stack.item).unwrap().to_owned()),
                )
            })
            .collect()
    }

    /// The player's list stays in step with the inventory, in order:
    /// a count changing keeps the same entities; a category emptied and
    /// filled again (Building, between Natural and Lighting) comes back in
    /// its place.
    #[test]
    fn lists_stay_in_order_as_the_inventory_changes() {
        let mut app = open_inventory();
        let mine = ListSource::Inventory(Owner::LocalPlayer);
        let (names, entities) = shown(&mut app, mine);
        assert_eq!(names, expected(&mut app));

        // One more stone: same entities, same order.
        let stone = item(&app, "defaria:stone");
        let at = catalog_at(&mut app, stone);
        run_more(&mut app, &click("MouseLeft", at));
        let (names, again) = shown(&mut app, mine);
        assert_eq!(names, expected(&mut app));
        assert_eq!(again, entities);

        // Empty Building (planks and the stone slab), then add planks back.
        let (planks, slab) = (
            item(&app, "defaria:planks"),
            item(&app, "defaria:stone_slab"),
        );
        let change = |app: &mut App, change: &dyn Fn(&mut Inventory)| {
            let mut players = app
                .world_mut()
                .query_filtered::<&mut Inventory, With<LocalPlayer>>();
            change(&mut players.single_mut(app.world_mut()).unwrap());
            run_more(app, "wait 2");
        };
        change(&mut app, &|inventory| {
            let (p, s) = (inventory.count(planks), inventory.count(slab));
            inventory.remove(planks, p);
            inventory.remove(slab, s);
        });
        let (names, _) = shown(&mut app, mine);
        assert!(!names.contains(&"#Building".to_owned()), "{names:?}");
        assert_eq!(names, expected(&mut app));
        let content = app.world().resource::<Content>().clone();
        change(&mut app, &|inventory| {
            inventory.add(&content, crate::inventory::ItemStack::one(planks));
        });
        let (names, _) = shown(&mut app, mine);
        assert_eq!(names, expected(&mut app));
        let position = |name: &str| names.iter().position(|n| n == name).unwrap();
        assert!(position("#Natural") < position("#Building"));
        assert!(position("#Building") < position("#Lighting"));
    }

    #[test]
    fn lists_show_category_headings() {
        let mut app = open_inventory();
        let mut headings = app
            .world_mut()
            .query_filtered::<(&Text, &ChildOf), With<crate::inventory::list::ListHeading>>();
        let found: Vec<(String, Entity)> = headings
            .iter(app.world())
            .map(|(text, parent)| (text.0.clone(), parent.parent()))
            .collect();
        let mut lists = app.world_mut().query::<(Entity, &ItemList)>();
        let catalog = lists
            .iter(app.world())
            .find(|(_, list)| list.source == ListSource::Catalog)
            .unwrap()
            .0;
        let names = |of_catalog: bool| -> Vec<&str> {
            found
                .iter()
                .filter(|(_, parent)| (*parent == catalog) == of_catalog)
                .map(|(name, _)| name.as_str())
                .collect()
        };
        assert_eq!(names(true), ["Natural", "Building", "Lighting", "Tools"]);
        // The new player's hotbar items: natural blocks, building blocks
        // and lamps.
        assert_eq!(names(false), ["Natural", "Building", "Lighting"]);
    }

    fn entries_of(app: &mut App, source: ListSource) -> Vec<ItemState> {
        let mut entries = app.world_mut().query::<&ListEntry>();
        entries
            .iter(app.world())
            .filter(|entry| entry.source == source)
            .map(|entry| entry.stack.item)
            .collect()
    }

    fn heading_texts(app: &mut App) -> Vec<String> {
        let mut headings = app
            .world_mut()
            .query_filtered::<&Text, With<crate::inventory::list::ListHeading>>();
        headings
            .iter(app.world())
            .map(|text| text.0.clone())
            .collect()
    }

    /// Typing in the search box narrows both lists as you type, with the
    /// screen staying open and the game's keys typed rather than acted on
    /// (a digit over an entry doesn't put it in the hotbar); a search that
    /// matches nothing says so; and the next time the screen opens it's
    /// cleared.
    #[test]
    fn searching_narrows_the_lists() {
        let mut app = open_inventory();
        let catalog = ListSource::Catalog;
        let mine = ListSource::Inventory(Owner::LocalPlayer);
        let all = entries_of(&mut app, catalog).len();
        let at = {
            let mut boxes = app
                .world_mut()
                .query_filtered::<&UiGlobalTransform, With<SearchBox>>();
            boxes.single(app.world()).unwrap().translation
        };
        run_more(&mut app, &click("MouseLeft", at));
        run_more(&mut app, "type stone\nwait 2");
        let content = app.world().resource::<Content>().clone();
        let named = |items: &[ItemState], word: &str| {
            items.iter().all(|&item| {
                content
                    .item_name(item)
                    .unwrap()
                    .to_lowercase()
                    .contains(word)
            })
        };
        let stones = entries_of(&mut app, catalog);
        assert!(!stones.is_empty() && stones.len() < all && named(&stones, "stone"));
        let carried = entries_of(&mut app, mine);
        assert_eq!(carried.len(), 2, "stone and stone slab");
        assert!(named(&carried, "stone"));

        // With the pointer over an entry, a digit is typed: the hotbar
        // stays as it was.
        let before = hotbar(&mut app);
        let over = entry_at(&mut app, catalog, stones[0]);
        run_more(
            &mut app,
            &format!("cursor {} {}\nwait 2\ntype 2\nwait 2", over.x, over.y),
        );
        assert_eq!(hotbar(&mut app), before);
        assert!(
            entries_of(&mut app, catalog).is_empty(),
            "\"stone2\" matches nothing"
        );
        run_more(&mut app, "hold Backspace 1\nwait 2");

        // Typing goes on, and the screen stays open.
        run_more(&mut app, "hold Space 1\nwait 1\ntype slab\nwait 2");
        assert!(is_open(&mut app));
        assert_eq!(entries_of(&mut app, mine).len(), 1);

        run_more(&mut app, "type zzz\nwait 2");
        assert!(entries_of(&mut app, catalog).is_empty());
        assert!(entries_of(&mut app, mine).is_empty());
        assert_eq!(
            heading_texts(&mut app),
            ["No items match", "No items match"]
        );

        run_more(&mut app, "hold Escape 1\nwait 2\nhold KeyE 1\nwait 3");
        assert_eq!(entries_of(&mut app, catalog).len(), all);
    }

    #[test]
    fn weights_read_naturally() {
        assert_eq!(format_weight(0.4), "0 kg");
        assert_eq!(format_weight(950.0), "950 kg");
        assert_eq!(format_weight(2500.0), "2.5 t");
        assert_eq!(format_weight(10_000_000.0), "10000 t");
        assert_eq!(weight_text(2500.0, Some(10_000.0)), "2.5 t of 10 t");
        assert_eq!(weight_text(2500.0, None), "2.5 t");
    }
}
