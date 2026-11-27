//! Machine screens: using (right clicking) a machine opens a screen laid
//! out from its definition, so every mod's machine gets one without
//! writing UI.
//!
//! - **Opening:** right click on a machine with slots, tanks or recipes
//!   opens its screen, instead of placing the held block against it (its
//!   step of a use, [`UseStep::Screen`]). With Shift held, right click
//!   places as usual.
//!   A machine with only an energy buffer (a lamp, a battery) has nothing
//!   to do on a screen, so blocks place against it as against any other.
//! - **Closing:** E or Escape, as the inventory; and on its own when the
//!   machine's block entity goes, because the machine was broken (or
//!   replaced by another block) or its chunk unloaded.
//! - **Layout:** a row of the machine's slot groups (a recipe machine's
//!   inputs, then its progress and status, then its outputs and any other
//!   groups), a bar per tank in its fluid's color, and its energy, each
//!   named, with its numbers under it; under that, the player's inventory
//!   as the inventory screen shows it, with how much they carry. A slot
//!   group of more than four is a square-ish grid (nine is three by
//!   three). The machine's part is at least as tall as three rows of
//!   slots, so the panel's top stays put from one machine to the next.
//!   A machine too wide for the window wraps its parts onto more lines,
//!   and its part scrolls if they're then too tall.
//! - **Live:** slots, bars and status follow the machine every frame
//!   while it's open.
//! - **Moving items:** as in the inventory (see `inventory::gesture`):
//!   left click on an entry of the player's inventory moves one into the
//!   machine's input groups, Shift moves them all, right click half;
//!   dragging one onto the machine moves them all (half with the right
//!   button), into the slot group it's dropped on first, if it's dropped
//!   on a slot (any group but a recipe machine's outputs). The same clicks on a machine's slot move its items to the
//!   player. Moves never make or lose items, and keep to the player's
//!   weight limit and the slots' sizes ([`super::exchange`]).

use bevy::{
    picking::{
        events::{PointerClick, PointerDragDrop},
        pointer::PointerButton,
    },
    prelude::*,
};

use crate::{
    hotbar_ui::hotbar_height,
    inventory_screen::{
        gesture::amount_of as amount,
        list::{ItemList, ListEntry, ListSource, Owner, item_list},
        screen::{INVENTORY_ROWS, WeightLabel},
    },
    net::{FromServer, ToServer},
    ui::slot::slot_fluid,
    ui::{
        Bar, ItemSlot, NO_FLUID, PointerTravel, Tooltip, UiScreen, UiTheme, bar, fluid_color,
        fluid_name, format_fluid, item_slot, scroll::Scrollable, scroll_panel, typing,
    },
};
use defaria_core::{
    content::{BlockState, Content},
    game_mode::item_for_block,
    machine::{EnergyBuffer, MachineActive, Slots, Tanks, exchange::input_groups},
    net::{ClientMessage, MachineContents, ServerMessage, Transfer},
    recipe::{RecipeProgress, RecipeStatus},
};

/// Space between the panel and the hotbar, in logical pixels.
pub const ABOVE_HOTBAR: f32 = 12.0;
/// Slot groups up to this size are one row; bigger ones are square-ish.
pub const GROUP_ROW: usize = 4;
/// The most slots in a row of a slot group; more wrap.
pub const GROUP_COLUMNS: usize = 9;
/// Rows of slots the machine's part of the screen is at least as tall as,
/// and the most a slot group shows at once (more scroll).
pub const MACHINE_ROWS: f32 = 3.0;
/// A tank's or the energy buffer's bar, in logical pixels.
pub const LEVEL_WIDTH: f32 = 24.0;
/// The recipe progress bar, in logical pixels.
pub const PROGRESS_WIDTH: f32 = 112.0;
pub const PROGRESS_HEIGHT: f32 = 14.0;
pub struct MachineScreenPlugin;

impl Plugin for MachineScreenPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MachineScreen>()
            .register_type::<MachineSlot>()
            .register_type::<TankBar>()
            .register_type::<EnergyBar>()
            .register_type::<ProgressBar>()
            .add_observer(click_machine_slot)
            .add_observer(click_player_entry)
            .add_observer(drop_on_machine)
            .add_observer(tell_the_server_it_closed)
            .add_systems(
                Update,
                (
                    // The view a screen opens with is there for its contents.
                    (open_machine_screen, show_machine_contents, close_on_server).chain(),
                    close_machine_screen.run_if(not(typing)),
                    (show_slots, show_tanks, show_energy, show_progress),
                )
                    .chain(),
            );
    }
}

/// A machine screen's root: the machine it shows. Despawning it closes the
/// screen.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq)]
#[reflect(Component)]
#[require(UiScreen)]
pub struct MachineScreen {
    /// What the server sends of the machine: a [`MachineView`].
    pub machine: Entity,
    pub pos: IVec3,
    /// The machine's state when the screen opened, for its definition
    /// (which a block's states share).
    pub state: BlockState,
}

/// One of the machine's slots on its screen: slot `index` of its slot
/// group `group`, indices into [`Slots`].
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct MachineSlot {
    pub group: usize,
    pub index: usize,
}

/// The bar of the machine's tank with this index into [`Tanks`].
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct TankBar(pub usize);

/// The bar of the machine's energy buffer.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct EnergyBar;

/// The bar of the machine's recipe progress.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct ProgressBar;

/// The numbers under a tank's bar: what it holds, of how much.
#[derive(Component, Debug, Clone, Copy)]
pub struct TankText(usize);

/// The numbers under the energy bar.
#[derive(Component, Debug, Clone, Copy)]
pub struct EnergyText;

/// The line under the progress bar saying what the machine is doing.
#[derive(Component, Debug, Clone, Copy)]
pub struct StatusText;

/// The machine's half of the screen, where the player's items are dropped.
#[derive(Component, Debug, Clone, Copy)]
pub struct MachinePanel;

/// On the player's inventory list in a machine screen: clicks send its
/// items to the machine.
#[derive(Component, Debug, Clone, Copy)]
pub struct SendsToMachine;

/// The client's copy of the machine whose screen is open: the parts the
/// server sends of it ([`Slots`], [`Tanks`], [`EnergyBuffer`],
/// [`RecipeProgress`], [`MachineActive`]), as it sends them.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct MachineView;

/// Opens the screen of the machine the server says the local player used
/// (`defaria_core::server` claims the use).
pub fn open_machine_screen(
    mut commands: Commands,
    mut messages: MessageReader<FromServer>,
    content: Res<Content>,
    theme: Res<UiTheme>,
    open: Query<Entity, With<MachineScreen>>,
) {
    for FromServer(message) in messages.read() {
        let ServerMessage::OpenScreen { pos, state } = *message else {
            continue;
        };
        for screen in &open {
            commands.entity(screen).despawn();
        }
        let view = commands
            .spawn((Name::new("Machine view"), MachineView))
            .id();
        let screen = MachineScreen {
            machine: view,
            pos,
            state,
        };
        spawn_machine_screen(&mut commands, &theme, &content, screen);
    }
}

/// Puts what the server sends of the open machine on its view.
pub fn show_machine_contents(
    mut commands: Commands,
    mut messages: MessageReader<FromServer>,
    views: Query<Entity, With<MachineView>>,
) {
    for FromServer(message) in messages.read() {
        let ServerMessage::MachineContents(contents) = message else {
            continue;
        };
        let MachineContents {
            slots,
            tanks,
            energy,
            progress,
            active,
        } = contents.clone();
        for view in &views {
            let mut view = commands.entity(view);
            match slots.clone() {
                Some(slots) => view.insert(slots),
                None => view.remove::<Slots>(),
            };
            match tanks.clone() {
                Some(tanks) => view.insert(tanks),
                None => view.remove::<Tanks>(),
            };
            match energy.clone() {
                Some(energy) => view.insert(energy),
                None => view.remove::<EnergyBuffer>(),
            };
            match progress.clone() {
                Some(progress) => view.insert(progress),
                None => view.remove::<RecipeProgress>(),
            };
            match active {
                Some(active) => view.insert(active),
                None => view.remove::<MachineActive>(),
            };
        }
    }
}

/// The screen closes when the server says so: its machine went.
pub fn close_on_server(
    mut commands: Commands,
    mut messages: MessageReader<FromServer>,
    screens: Query<Entity, With<MachineScreen>>,
) {
    if messages
        .read()
        .any(|FromServer(message)| matches!(message, ServerMessage::CloseScreen))
    {
        for screen in &screens {
            commands.entity(screen).try_despawn();
        }
    }
}

/// E closes the screen, as it does the inventory.
pub fn close_machine_screen(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    screens: Query<Entity, With<MachineScreen>>,
) {
    if keys.just_pressed(KeyCode::KeyE) {
        for root in &screens {
            // Escape may close it this frame too.
            commands.entity(root).try_despawn();
        }
    }
}

/// However the screen closed, its view goes, and the server stops sending
/// the machine.
fn tell_the_server_it_closed(
    closed: On<Remove<MachineScreen>>,
    mut commands: Commands,
    screens: Query<&MachineScreen>,
    mut to_server: MessageWriter<ToServer>,
) {
    if let Ok(screen) = screens.get(closed.entity) {
        commands.entity(screen.machine).try_despawn();
    }
    to_server.write(ToServer(ClientMessage::CloseScreen));
}

/// `name` with its first letter in capitals: `input` is `Input`.
pub fn title_case(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// What a machine in `state` is called: its item's name, or its block's id.
pub fn machine_name(content: &Content, state: BlockState) -> String {
    item_for_block(content, state)
        .and_then(|item| content.item_name(item))
        .map_or_else(|| content.block_of(state).id.clone(), str::to_owned)
}

/// A named part of the machine's row: its name above `contents`.
pub fn section(
    row: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    name: &str,
    contents: impl FnOnce(&mut ChildSpawnerCommands),
) {
    row.spawn(Node {
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: px(6),
        ..default()
    })
    .with_children(|section| {
        section.spawn((
            Text::new(name),
            theme.text_font(),
            TextColor(theme.text_dim),
        ));
        contents(section);
    });
}

/// The slots of slot group `group`, `count` of them, under its name. A
/// group of more than [`MACHINE_ROWS`] rows scrolls, that many rows tall,
/// so a big one still fits on the screen.
pub fn group_section(
    row: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    name: &str,
    group: usize,
    count: usize,
) {
    let columns = group_columns(count);
    let rows = count.div_ceil(columns);
    let width = columns as f32 * theme.slot_size + (columns as f32 - 1.0) * theme.slot_gap;
    let grid = Node {
        width: px(width),
        flex_wrap: FlexWrap::Wrap,
        align_content: AlignContent::FlexStart,
        column_gap: px(theme.slot_gap),
        row_gap: px(theme.slot_gap),
        ..default()
    };
    let slots = move |slots: &mut ChildSpawnerCommands| {
        for index in 0..count {
            slots.spawn((
                MachineSlot { group, index },
                item_slot(theme, ItemSlot::default()),
            ));
        }
    };
    section(row, theme, &title_case(name), |section| {
        if rows as f32 > MACHINE_ROWS {
            let height = MACHINE_ROWS * theme.slot_size + (MACHINE_ROWS - 1.0) * theme.slot_gap;
            let grid = Node {
                height: px(height),
                ..grid
            };
            let line = theme.slot_size + theme.slot_gap;
            scroll_panel(section, theme, grid, line, Name::new("Slot group"), slots);
        } else {
            section.spawn(grid).with_children(slots);
        }
    });
}

/// How many columns a slot group of `count` slots is laid out in: one row
/// up to [`GROUP_ROW`], then as square as it goes (nine is three by three).
pub fn group_columns(count: usize) -> usize {
    if count <= GROUP_ROW {
        count.max(1)
    } else {
        let side = count.isqrt();
        let side = if side * side < count { side + 1 } else { side };
        side.min(GROUP_COLUMNS)
    }
}

/// A bar's numbers: `amount` and, on a line under it, of how much.
pub fn level_numbers(amount: String, capacity: String) -> String {
    format!("{amount}\nof {capacity}")
}

/// The text under a bar, centred, saying its numbers.
pub fn numbers_text(theme: &UiTheme) -> impl Bundle {
    (
        Text::default(),
        TextLayout::justify(Justify::Center),
        theme.text_font(),
        TextColor(theme.text_dim),
    )
}

/// How tall a tank's or the energy buffer's bar is: two slots.
pub fn level_height(theme: &UiTheme) -> f32 {
    2.0 * theme.slot_size + theme.slot_gap
}

/// Spawns the screen `screen` names.
pub fn spawn_machine_screen(
    commands: &mut Commands,
    theme: &UiTheme,
    content: &Content,
    screen: MachineScreen,
) {
    let Some(machine) = content.machine(screen.state) else {
        return;
    };
    let name = machine_name(content, screen.state);
    // Inputs, the progress, then the outputs and the rest.
    let inputs = input_groups(machine);
    let outputs = machine
        .recipes
        .as_ref()
        .map_or_else(Vec::new, |recipes| recipes.output_slots.clone());
    let others = (0..machine.slots.len()).filter(|i| !inputs.contains(i) && !outputs.contains(i));
    // A group that's both an input and an output is drawn once, with the
    // inputs.
    let after: Vec<usize> = outputs
        .iter()
        .copied()
        .filter(|i| !inputs.contains(i))
        .chain(others)
        .collect();
    let group = |row: &mut ChildSpawnerCommands, index: usize| {
        let spec = &machine.slots[index];
        group_section(row, theme, &spec.name, index, spec.count as usize);
    };
    let player = ListSource::Inventory(Owner::LocalPlayer);
    commands
        .spawn((
            Name::new(format!("{name} screen")),
            screen,
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
                    // No bigger than the window: a wide machine's parts
                    // wrap, and scroll if that makes them too tall.
                    max_width: percent(100),
                    max_height: percent(100),
                    padding: UiRect::all(px(theme.panel_padding)),
                    border: UiRect::all(px(1)),
                    row_gap: px(10),
                    ..default()
                },
                BackgroundColor(theme.panel),
                BorderColor::all(theme.panel_border),
            ))
            .with_children(|panel| {
                panel.spawn((Text::new(name), theme.title_font(), TextColor(theme.text)));
                panel
                    .spawn((
                        MachinePanel,
                        Scrollable {
                            line: theme.slot_size + theme.slot_gap,
                        },
                        Node {
                            overflow: Overflow::scroll_y(),
                            justify_content: JustifyContent::Center,
                            // Every part's name on one line.
                            align_items: AlignItems::FlexStart,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: px(28),
                            row_gap: px(12),
                            padding: UiRect::vertical(px(8)),
                            // The same height for most machines, so the
                            // panel doesn't jump between them.
                            // (Names are a line of text: a little over
                            // the font's size.)
                            min_height: px(16.0
                                + 1.5 * theme.font_size
                                + 6.0
                                + MACHINE_ROWS * theme.slot_size
                                + (MACHINE_ROWS - 1.0) * theme.slot_gap),
                            ..default()
                        },
                    ))
                    .with_children(|row| {
                        for &index in &inputs {
                            group(row, index);
                        }
                        if machine.recipes.is_some() {
                            progress_section(row, theme);
                        }
                        for &index in &after {
                            group(row, index);
                        }
                        for (index, tank) in machine.tanks.iter().enumerate() {
                            section(row, theme, &title_case(&tank.name), |section| {
                                section.spawn((TankBar(index), level_bar(theme, NO_FLUID)));
                                section.spawn((TankText(index), numbers_text(theme)));
                            });
                        }
                        if machine.energy.is_some() {
                            section(row, theme, "Energy", |section| {
                                section.spawn((EnergyBar, level_bar(theme, theme.energy)));
                                section.spawn((EnergyText, numbers_text(theme)));
                            });
                        }
                    });
                panel.spawn((
                    Node {
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    children![
                        (
                            Text::new("Inventory"),
                            theme.title_font(),
                            TextColor(theme.text)
                        ),
                        (
                            WeightLabel,
                            Text::default(),
                            theme.text_font(),
                            TextColor(theme.text_dim),
                        )
                    ],
                ));
                item_list(
                    panel,
                    theme,
                    ItemList {
                        source: player,
                        // Clicks go to the machine, not another list.
                        sends_to: None,
                    },
                    INVENTORY_ROWS,
                    (Name::new("Inventory list"), SendsToMachine),
                );
            });
        });
}

/// A vertical bar for a tank or the energy buffer, empty to start with.
pub fn level_bar(theme: &UiTheme, color: Color) -> impl Bundle {
    (
        bar(
            theme,
            Bar {
                fraction: 0.0,
                color,
                vertical: true,
            },
            LEVEL_WIDTH,
            level_height(theme),
        ),
        Tooltip::default(),
    )
}

/// The recipe's progress bar and the status under it.
pub fn progress_section(row: &mut ChildSpawnerCommands, theme: &UiTheme) {
    row.spawn(Node {
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: px(6),
        // Level with the middle of the slots beside it, under their names.
        margin: UiRect::top(px(theme.font_size
            + 6.0
            + (theme.slot_size - PROGRESS_HEIGHT) / 2.0)),
        ..default()
    })
    .with_children(|section| {
        section.spawn((
            ProgressBar,
            bar(
                theme,
                Bar {
                    fraction: 0.0,
                    color: theme.progress,
                    vertical: false,
                },
                PROGRESS_WIDTH,
                PROGRESS_HEIGHT,
            ),
            Tooltip::default(),
        ));
        section.spawn((
            StatusText,
            Text::default(),
            theme.text_font(),
            TextColor(theme.text_dim),
        ));
    });
}

/// How much energy, for people: joules, kilojoules or megajoules.
pub fn format_energy(joules: u64) -> String {
    let short = |value: f64, unit: &str| {
        let text = format!("{value:.1}");
        format!("{} {unit}", text.trim_end_matches(".0"))
    };
    match joules {
        0..1000 => format!("{joules} J"),
        1000..1_000_000 => short(joules as f64 / 1000.0, "kJ"),
        _ => short(joules as f64 / 1_000_000.0, "MJ"),
    }
}

/// What the status line says for `progress`.
pub fn status_text(progress: &RecipeProgress) -> String {
    match progress.status {
        RecipeStatus::Idle => "Idle".to_owned(),
        RecipeStatus::Running => format!("{}%", (progress.fraction() * 100.0).floor()),
        RecipeStatus::NoEnergy => "Needs energy".to_owned(),
        RecipeStatus::OutputFull => "Output full".to_owned(),
    }
}

/// How full, from 0 to 1.
pub fn fraction(amount: u64, capacity: u64) -> f32 {
    if capacity == 0 {
        0.0
    } else {
        (amount as f64 / capacity as f64) as f32
    }
}

pub fn show_slots(
    content: Res<Content>,
    screen: Option<Single<&MachineScreen>>,
    machines: Query<&Slots>,
    mut slots: Query<(&MachineSlot, &mut ItemSlot)>,
) {
    let Some(held) = screen.and_then(|screen| machines.get(screen.machine).ok()) else {
        return;
    };
    for (at, mut slot) in &mut slots {
        let stack = held
            .0
            .get(at.group)
            .and_then(|group| group.slots.get(at.index))
            .and_then(Option::as_ref);
        slot.set_if_neq(ItemSlot {
            item: stack.map(|stack| stack.item),
            count: stack.map_or(0, |stack| stack.count),
            fluid: stack.and_then(|stack| slot_fluid(&content, stack)),
            ..default()
        });
    }
}

pub fn show_tanks(
    content: Res<Content>,
    screen: Option<Single<&MachineScreen>>,
    machines: Query<&Tanks>,
    mut bars: Query<(&TankBar, &mut Bar, &mut Tooltip)>,
    mut numbers: Query<(&TankText, &mut Text)>,
) {
    let Some(tanks) = screen.and_then(|screen| machines.get(screen.machine).ok()) else {
        return;
    };
    for (&TankBar(index), mut bar, mut tooltip) in &mut bars {
        let Some(tank) = tanks.0.get(index) else {
            continue;
        };
        bar.set_if_neq(Bar {
            fraction: fraction(tank.amount, tank.capacity),
            color: fluid_color(
                &content,
                tank.fluid
                    .as_deref()
                    .and_then(|id| content.default_state(id)),
            ),
            vertical: true,
        });
        let fluid = tank
            .fluid
            .as_deref()
            .map_or_else(|| "Empty".to_owned(), |id| fluid_name(&content, id));
        let text = format!(
            "{} tank: {fluid}\n{} of {}",
            title_case(&tank.name),
            format_fluid(tank.amount),
            format_fluid(tank.capacity)
        );
        if tooltip.0 != text {
            tooltip.0 = text;
        }
    }
    for (&TankText(index), mut line) in &mut numbers {
        let Some(tank) = tanks.0.get(index) else {
            continue;
        };
        let text = level_numbers(format_fluid(tank.amount), format_fluid(tank.capacity));
        if line.0 != text {
            line.0 = text;
        }
    }
}

pub fn show_energy(
    screen: Option<Single<&MachineScreen>>,
    machines: Query<&EnergyBuffer>,
    mut bars: Query<(&mut Bar, &mut Tooltip), With<EnergyBar>>,
    mut numbers: Query<&mut Text, With<EnergyText>>,
) {
    let Some(energy) = screen.and_then(|screen| machines.get(screen.machine).ok()) else {
        return;
    };
    for (mut bar, mut tooltip) in &mut bars {
        let wanted = fraction(energy.stored, energy.capacity);
        if bar.fraction != wanted {
            bar.fraction = wanted;
        }
        let text = format!(
            "Energy\n{} of {}",
            format_energy(energy.stored),
            format_energy(energy.capacity)
        );
        if tooltip.0 != text {
            tooltip.0 = text;
        }
    }
    let text = level_numbers(format_energy(energy.stored), format_energy(energy.capacity));
    for mut line in &mut numbers {
        if line.0 != text {
            line.0.clone_from(&text);
        }
    }
}

pub fn show_progress(
    screen: Option<Single<&MachineScreen>>,
    machines: Query<&RecipeProgress>,
    mut bars: Query<(&mut Bar, &mut Tooltip), With<ProgressBar>>,
    mut lines: Query<&mut Text, With<StatusText>>,
) {
    let Some(progress) = screen.and_then(|screen| machines.get(screen.machine).ok()) else {
        return;
    };
    let status = status_text(progress);
    for (mut bar, mut tooltip) in &mut bars {
        let wanted = progress.fraction();
        if bar.fraction != wanted {
            bar.fraction = wanted;
        }
        let text = format!("Progress: {}%", (wanted * 100.0).floor());
        if tooltip.0 != text {
            tooltip.0 = text;
        }
    }
    for mut line in &mut lines {
        if line.0 != status {
            line.0.clone_from(&status);
        }
    }
}

pub fn shift(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
}

/// A click on one of the machine's slots moves its items to the player.
pub fn click_machine_slot(
    click: On<PointerClick>,
    keys: Res<ButtonInput<KeyCode>>,
    travel: Res<PointerTravel>,
    slots: Query<&MachineSlot>,
    screen: Option<Single<&MachineScreen>>,
    machines: Query<&Slots>,
    mut to_server: MessageWriter<ToServer>,
) {
    let Ok(&MachineSlot { group, index }) = slots.get(click.entity) else {
        return;
    };
    if click.button == PointerButton::Middle || !travel.is_click() {
        return;
    }
    let Some(screen) = screen else {
        return;
    };
    let Ok(held) = machines.get(screen.machine) else {
        return;
    };
    let Some(Some(stack)) = held.0.get(group).and_then(|g| g.slots.get(index)) else {
        return;
    };
    let count = amount(stack.count, click.button, shift(&keys));
    to_server.write(ToServer(ClientMessage::Transfer(Transfer::OutOfMachine {
        group,
        index,
        count,
    })));
}

/// Asks to move up to `count` of `entry`'s kind from the player into the
/// machine whose screen is open: into slot group `first` first, if one's
/// chosen.
pub fn send_to_machine(
    to_server: &mut MessageWriter<ToServer>,
    entry: &ListEntry,
    count: u32,
    first: Option<usize>,
) {
    to_server.write(ToServer(ClientMessage::Transfer(Transfer::IntoMachine {
        kind: entry.stack.clone(),
        count,
        first,
    })));
}

/// A click on an entry of the player's inventory, in a machine screen,
/// moves its items into the machine.
pub fn click_player_entry(
    click: On<PointerClick>,
    keys: Res<ButtonInput<KeyCode>>,
    travel: Res<PointerTravel>,
    entries: Query<(&ListEntry, &ChildOf)>,
    lists: Query<(), With<SendsToMachine>>,
    screen: Option<Single<&MachineScreen>>,
    mut to_server: MessageWriter<ToServer>,
) {
    let Ok((entry, parent)) = entries.get(click.entity) else {
        return;
    };
    if !lists.contains(parent.parent())
        || click.button == PointerButton::Middle
        || !travel.is_click()
    {
        return;
    }
    if screen.is_none() {
        return;
    }
    let count = amount(entry.stack.count, click.button, shift(&keys));
    send_to_machine(&mut to_server, entry, count, None);
}

/// An entry of the player's inventory dropped on the machine's half of the
/// screen goes in: all of it, or half with the right button. Dropped on
/// one of its slots, it goes into that slot's group first.
pub fn drop_on_machine(
    drop: On<PointerDragDrop>,
    entries: Query<&ListEntry>,
    panels: Query<(), With<MachinePanel>>,
    machine_slots: Query<&MachineSlot>,
    parents: Query<&ChildOf>,
    screen: Option<Single<&MachineScreen>>,
    mut to_server: MessageWriter<ToServer>,
) {
    // A drop bubbles up; only the node it landed on counts.
    if drop.entity != drop.original_event_target() {
        return;
    }
    let Ok(entry) = entries.get(drop.dropped) else {
        return;
    };
    if entry.source != ListSource::Inventory(Owner::LocalPlayer) {
        return;
    }
    let under: Vec<Entity> = std::iter::once(drop.entity)
        .chain(parents.iter_ancestors(drop.entity))
        .collect();
    let on_machine = under.iter().any(|&entity| panels.contains(entity));
    let group = under
        .iter()
        .find_map(|&entity| machine_slots.get(entity).ok().map(|slot| slot.group));
    if !on_machine || screen.is_none() {
        return;
    }
    let count = amount(
        entry.stack.count,
        drop.button,
        drop.button == PointerButton::Primary,
    );
    send_to_machine(&mut to_server, entry, count, group);
}
