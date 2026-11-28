//! The Multiplayer screen: the saved servers (`servers.json5` in the data
//! folder; `defaria_core::server_list`), in their order, each with its
//! name and address. Clicking one joins it; **Edit** beside it renames it,
//! gives it another address or removes it; **Add server** adds one.
//!
//! Joining asks the game to connect ([`Connect`]) and shows how that's
//! going ([`ConnectStatus`]) until play starts: **Cancel** or Escape calls
//! it off. A failure says why, with **Try again**, and for a server whose
//! key changed, **Trust new key**, which forgets the old one and connects.
//! A server that goes while the player's there says why the same way.

use std::path::Path;

use bevy::{picking::events::PointerClick, prelude::*};
use defaria_core::{
    save::DataFolder,
    server_list::{MAX_ADDRESS, MAX_NAME, SavedServer, ServerList},
};

use super::{MenuClick, MenuError, MenuPage, back_to_main, open_page};
use crate::{
    connecting::{CancelConnect, Connect, ConnectStatus, ServerTarget},
    session::GameState,
    ui::{UiTheme, button, scroll_panel, text_field_holding, typing},
};

pub struct MultiplayerMenuPlugin;

impl Plugin for MultiplayerMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (follow_connect_status, cancel_on_escape.run_if(not(typing)))
                .run_if(in_state(GameState::Menu)),
        );
    }
}

/// How tall the server list is at most, in logical pixels, before it
/// scrolls: about five servers.
const SERVER_LIST_HEIGHT: f32 = 300.0;

/// How wide the server screens' fields are, and their error text at most.
const FIELD_WIDTH: f32 = 300.0;
const TEXT_WIDTH: f32 = 420.0;

/// The server a list entry (or its **Edit**) is for.
#[derive(Component, Debug, Clone)]
pub struct ServerEntry(pub SavedServer);

/// The add and edit screens' name field.
#[derive(Component, Debug, Clone, Copy)]
pub struct ServerNameField;

/// The add and edit screens' address field.
#[derive(Component, Debug, Clone, Copy)]
pub struct AddressField;

/// The edit screen's root: the address of the server it edits.
#[derive(Component, Debug, Clone)]
pub struct EditingServer(pub String);

/// The connecting screen's line saying how far it's got.
#[derive(Component, Debug, Clone, Copy)]
pub struct ConnectStepText;

/// The error screen's text: why connecting failed, or the server went.
#[derive(Component, Debug, Clone, Copy)]
pub struct ConnectErrorText;

/// The server the connecting or error screen is about.
#[derive(Component, Debug, Clone)]
pub struct StatusServer(pub ServerTarget);

fn data_folder<'a>(folder: &'a Option<Res<DataFolder>>) -> Option<&'a Path> {
    folder.as_deref().map(|folder| folder.0.as_path())
}

/// **Multiplayer**, from the main menu.
pub(super) fn show_servers(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<DataFolder>>,
    pages: Query<Entity, With<MenuPage>>,
) {
    if menu.counts(&click) {
        servers_page(&mut commands, &theme, &pages, data_folder(&folder));
    }
}

/// **Back** to the list, from a server's screens, done with what they said.
fn back_to_servers(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<DataFolder>>,
    pages: Query<Entity, With<MenuPage>>,
    mut status: ResMut<ConnectStatus>,
) {
    if !menu.counts(&click) {
        return;
    }
    if !status.is_connecting() {
        *status = ConnectStatus::Idle;
    }
    servers_page(&mut commands, &theme, &pages, data_folder(&folder));
}

/// The list of saved servers, read from `folder`'s `servers.json5`.
pub(super) fn servers_page(
    commands: &mut Commands,
    theme: &UiTheme,
    pages: &Query<Entity, With<MenuPage>>,
    folder: Option<&Path>,
) {
    let list = ServerList::load(folder);
    open_page(
        commands,
        theme,
        pages,
        MenuPage::Servers,
        "Multiplayer",
        |panel, theme| {
            let servers = match &list {
                Ok(list) => list.servers().to_vec(),
                Err(error) => {
                    panel.spawn(wrapped(
                        theme,
                        format!("Can't read the server list: {error}"),
                    ));
                    Vec::new()
                }
            };
            if servers.is_empty() && list.is_ok() {
                panel.spawn((
                    Text::new("No servers yet"),
                    theme.text_font(),
                    TextColor(theme.text_dim),
                ));
            }
            if !servers.is_empty() {
                let node = Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(10),
                    max_height: px(SERVER_LIST_HEIGHT),
                    flex_grow: 1.0,
                    ..default()
                };
                scroll_panel(panel, theme, node, 62.0, Name::new("Server list"), |list| {
                    for server in servers {
                        server_row(list, theme, server);
                    }
                });
            }
            // Adding changes the file, which can't be read.
            if list.is_ok() {
                panel
                    .spawn((Name::new("Add server"), button(theme, "Add server")))
                    .observe(show_add_server);
            }
            panel
                .spawn((Name::new("Back"), button(theme, "Back")))
                .observe(back_to_main);
        },
    );
}

/// A server on the list: its name and address, which join it, and
/// **Edit**.
fn server_row(list: &mut ChildSpawnerCommands, theme: &UiTheme, server: SavedServer) {
    let name = server.shown_name().to_string();
    list.spawn(Node {
        column_gap: px(8),
        ..default()
    })
    .with_children(|row| {
        row.spawn((
            Name::new(format!("Server: {name}")),
            ServerEntry(server.clone()),
            button(theme, name.clone()),
        ))
        // Two lines: the name, and the address under it, dim (a level
        // down, so the button doesn't colour it as its label).
        .insert(Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexStart,
            flex_grow: 1.0,
            min_width: px(240),
            padding: UiRect::axes(px(theme.button_padding), px(theme.button_padding / 2.0)),
            border: UiRect::all(px(theme.slot_border)),
            ..default()
        })
        .with_children(|entry| {
            entry
                .spawn((Node::default(), Pickable::IGNORE))
                .with_child((
                    Text::new(server.address.clone()),
                    theme.text_font(),
                    TextColor(theme.text_dim),
                    Pickable::IGNORE,
                ));
        })
        .observe(join_server);
        row.spawn((
            Name::new(format!("Edit: {name}")),
            ServerEntry(server),
            button(theme, "Edit"),
        ))
        .observe(show_edit_server);
    });
}

/// Text that wraps rather than widening the panel.
fn wrapped(theme: &UiTheme, text: impl Into<String>) -> impl Bundle {
    (
        Text::new(text),
        theme.text_font(),
        TextColor(theme.text_dim),
        Node {
            max_width: px(TEXT_WIDTH),
            ..default()
        },
    )
}

fn show_add_server(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    pages: Query<Entity, With<MenuPage>>,
) {
    if menu.counts(&click) {
        server_form(&mut commands, &theme, &pages, None);
    }
}

fn show_edit_server(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    pages: Query<Entity, With<MenuPage>>,
    entries: Query<&ServerEntry>,
) {
    if !menu.counts(&click) {
        return;
    }
    if let Ok(ServerEntry(server)) = entries.get(click.entity) {
        server_form(&mut commands, &theme, &pages, Some(server));
    }
}

/// The add screen, or with `editing`, the edit screen for that server:
/// its name and address, and what to do with them.
fn server_form(
    commands: &mut Commands,
    theme: &UiTheme,
    pages: &Query<Entity, With<MenuPage>>,
    editing: Option<&SavedServer>,
) {
    let (page, title) = match editing {
        Some(_) => (MenuPage::EditServer, "Edit server"),
        None => (MenuPage::AddServer, "Add server"),
    };
    let (name, address) = editing.map_or((String::new(), String::new()), |server| {
        (server.name.clone(), server.address.clone())
    });
    let root = open_page(commands, theme, pages, page, title, |panel, theme| {
        let label = |text: &str| {
            (
                Text::new(text),
                theme.text_font(),
                TextColor(theme.text_dim),
            )
        };
        panel.spawn(label("Name"));
        panel.spawn((
            Name::new("Server name field"),
            ServerNameField,
            text_field_holding(theme, FIELD_WIDTH, "A server", MAX_NAME, &name),
        ));
        panel.spawn(label("Address"));
        panel.spawn((
            Name::new("Address field"),
            AddressField,
            text_field_holding(theme, FIELD_WIDTH, "host:port", MAX_ADDRESS, &address),
        ));
        panel.spawn((MenuError, wrapped(theme, "")));
        match editing {
            Some(_) => {
                panel
                    .spawn((Name::new("Save"), button(theme, "Save")))
                    .observe(save_server);
                panel
                    .spawn((Name::new("Remove"), button(theme, "Remove")))
                    .observe(remove_server);
            }
            None => {
                panel
                    .spawn((Name::new("Add"), button(theme, "Add")))
                    .observe(add_server);
            }
        }
        panel
            .spawn((Name::new("Back"), button(theme, "Back")))
            .observe(back_to_servers);
    });
    if let Some(server) = editing {
        commands
            .entity(root)
            .insert(EditingServer(server.address.clone()));
    }
}

/// What the add or edit screen's fields say: the name and the address.
#[derive(bevy::ecs::system::SystemParam)]
struct ServerForm<'w, 's> {
    names: Query<'w, 's, &'static crate::ui::TextField, With<ServerNameField>>,
    addresses: Query<'w, 's, &'static crate::ui::TextField, With<AddressField>>,
    errors: Query<'w, 's, &'static mut Text, With<MenuError>>,
}

impl ServerForm<'_, '_> {
    fn values(&self) -> (String, String) {
        let value =
            |field: Option<&crate::ui::TextField>| field.map_or(String::new(), |f| f.value.clone());
        (
            value(self.names.iter().next()),
            value(self.addresses.iter().next()),
        )
    }

    fn show_error(&mut self, error: &str) {
        error!("{error}");
        for mut text in &mut self.errors {
            text.0 = error.to_string();
        }
    }
}

/// **Add**: the server goes at the end of the list, and the list shows.
fn add_server(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<DataFolder>>,
    pages: Query<Entity, With<MenuPage>>,
    mut form: ServerForm,
) {
    if !menu.counts(&click) {
        return;
    }
    let (name, address) = form.values();
    let added =
        ServerList::load(data_folder(&folder)).and_then(|mut list| list.add(&name, &address));
    match added {
        Ok(_) => servers_page(&mut commands, &theme, &pages, data_folder(&folder)),
        Err(error) => form.show_error(&format!("Can't add it: {error}")),
    }
}

/// **Save**: the server keeps its place, with its new name and address.
fn save_server(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<DataFolder>>,
    pages: Query<Entity, With<MenuPage>>,
    editing: Query<&EditingServer>,
    mut form: ServerForm,
) {
    if !menu.counts(&click) {
        return;
    }
    let Some(EditingServer(was)) = editing.iter().next() else {
        return;
    };
    let (name, address) = form.values();
    let saved =
        ServerList::load(data_folder(&folder)).and_then(|mut list| list.edit(was, &name, &address));
    match saved {
        Ok(_) => servers_page(&mut commands, &theme, &pages, data_folder(&folder)),
        Err(error) => form.show_error(&format!("Can't save it: {error}")),
    }
}

/// **Remove**: off the list, and its key forgotten.
fn remove_server(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<DataFolder>>,
    pages: Query<Entity, With<MenuPage>>,
    editing: Query<&EditingServer>,
    mut form: ServerForm,
) {
    if !menu.counts(&click) {
        return;
    }
    let Some(EditingServer(address)) = editing.iter().next() else {
        return;
    };
    let removed = ServerList::load(data_folder(&folder)).and_then(|mut list| list.remove(address));
    match removed {
        Ok(()) => servers_page(&mut commands, &theme, &pages, data_folder(&folder)),
        Err(error) => form.show_error(&format!("Can't remove it: {error}")),
    }
}

/// A server on the list, clicked: join it.
fn join_server(
    click: On<PointerClick>,
    menu: MenuClick,
    entries: Query<&ServerEntry>,
    mut connect: MessageWriter<Connect>,
) {
    if !menu.counts(&click) {
        return;
    }
    if let Ok(ServerEntry(server)) = entries.get(click.entity) {
        connect.write(Connect(ServerTarget {
            name: server.shown_name().to_string(),
            address: server.address.clone(),
        }));
    }
}

/// The connecting screen, or the error screen, for how joining is going.
pub(super) fn status_page(
    commands: &mut Commands,
    theme: &UiTheme,
    pages: &Query<Entity, With<MenuPage>>,
    status: &ConnectStatus,
) {
    match status {
        ConnectStatus::Connecting { server, step } => {
            let root = open_page(
                commands,
                theme,
                pages,
                MenuPage::Connecting,
                "Connecting",
                |panel, theme| {
                    panel.spawn(wrapped(theme, server.name.clone()));
                    panel.spawn((
                        ConnectStepText,
                        Text::new(step.describe()),
                        theme.text_font(),
                        TextColor(theme.text),
                    ));
                    panel
                        .spawn((Name::new("Cancel"), button(theme, "Cancel")))
                        .observe(cancel_connecting);
                },
            );
            commands.entity(root).insert(StatusServer(server.clone()));
        }
        ConnectStatus::Failed {
            server,
            error,
            changed_key,
        } => error_page(
            commands,
            theme,
            pages,
            "Can't connect",
            server,
            error,
            *changed_key,
        ),
        ConnectStatus::Lost { server, reason } => error_page(
            commands,
            theme,
            pages,
            "Disconnected",
            server,
            reason,
            false,
        ),
        ConnectStatus::Idle | ConnectStatus::Playing { .. } => {}
    }
}

/// Why joining `server` failed (or it went): `why`, and what to do next.
fn error_page(
    commands: &mut Commands,
    theme: &UiTheme,
    pages: &Query<Entity, With<MenuPage>>,
    title: &str,
    server: &ServerTarget,
    why: &str,
    changed_key: bool,
) {
    let root = open_page(
        commands,
        theme,
        pages,
        MenuPage::ConnectError,
        title,
        |panel, theme| {
            panel.spawn((
                Text::new(server.name.clone()),
                theme.text_font(),
                TextColor(theme.text),
            ));
            panel.spawn((ConnectErrorText, wrapped(theme, capitalized(why))));
            if changed_key {
                panel
                    .spawn((Name::new("Trust new key"), button(theme, "Trust new key")))
                    .observe(trust_new_key);
            }
            panel
                .spawn((Name::new("Try again"), button(theme, "Try again")))
                .observe(try_again);
            panel
                .spawn((Name::new("Back"), button(theme, "Back")))
                .observe(back_to_servers);
        },
    );
    commands.entity(root).insert(StatusServer(server.clone()));
}

/// `text` with its first letter a capital, as a sentence on its own.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The screens follow how joining goes: connecting shows how far it's
/// got, and a failure why (play starting closes the menu).
fn follow_connect_status(
    status: Res<ConnectStatus>,
    mut commands: Commands,
    theme: Res<UiTheme>,
    pages: Query<Entity, With<MenuPage>>,
    open: Query<&MenuPage>,
    mut steps: Query<&mut Text, With<ConnectStepText>>,
) {
    if !status.is_changed() {
        return;
    }
    let page = open.iter().next().copied();
    match &*status {
        ConnectStatus::Connecting { step, .. } if page == Some(MenuPage::Connecting) => {
            for mut text in &mut steps {
                text.0 = step.describe();
            }
        }
        ConnectStatus::Connecting { .. }
        | ConnectStatus::Failed { .. }
        | ConnectStatus::Lost { .. } => {
            status_page(&mut commands, &theme, &pages, &status);
        }
        ConnectStatus::Idle | ConnectStatus::Playing { .. } => {}
    }
}

/// **Cancel**, while connecting.
fn cancel_connecting(
    click: On<PointerClick>,
    menu: MenuClick,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<DataFolder>>,
    pages: Query<Entity, With<MenuPage>>,
    mut cancel: MessageWriter<CancelConnect>,
) {
    if menu.counts(&click) {
        cancel.write(CancelConnect);
        servers_page(&mut commands, &theme, &pages, data_folder(&folder));
    }
}

/// Escape, while connecting, cancels too.
fn cancel_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    open: Query<&MenuPage>,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<DataFolder>>,
    pages: Query<Entity, With<MenuPage>>,
    mut cancel: MessageWriter<CancelConnect>,
) {
    if keys.just_pressed(KeyCode::Escape) && open.iter().any(|page| *page == MenuPage::Connecting) {
        cancel.write(CancelConnect);
        servers_page(&mut commands, &theme, &pages, data_folder(&folder));
    }
}

/// **Try again**, after a failure.
fn try_again(
    click: On<PointerClick>,
    menu: MenuClick,
    servers: Query<&StatusServer>,
    mut connect: MessageWriter<Connect>,
) {
    if !menu.counts(&click) {
        return;
    }
    if let Some(StatusServer(server)) = servers.iter().next() {
        connect.write(Connect(server.clone()));
    }
}

/// **Trust new key**, for a server whose key changed: forgets the one
/// remembered, and connects, trusting the one it shows.
fn trust_new_key(
    click: On<PointerClick>,
    menu: MenuClick,
    folder: Option<Res<DataFolder>>,
    servers: Query<&StatusServer>,
    mut texts: Query<&mut Text, With<ConnectErrorText>>,
    mut connect: MessageWriter<Connect>,
) {
    if !menu.counts(&click) {
        return;
    }
    let Some(StatusServer(server)) = servers.iter().next() else {
        return;
    };
    let forgot = ServerList::load(data_folder(&folder))
        .and_then(|mut list| list.forget_key(&server.address));
    match forgot {
        Ok(()) => {
            info!("forgot the key of {}, to trust its new one", server.address);
            connect.write(Connect(server.clone()));
        }
        Err(error) => {
            error!("{error}");
            for mut text in &mut texts {
                text.0 = format!("Can't forget its key: {error}");
            }
        }
    }
}
