//! Chat on screen: the chat line (T) and the messages over the game.
//!
//! - T, while playing with no screen open and no text field typing,
//!   opens the chat line: a text field at the bottom left, with the
//!   keyboard. It's a screen ([`UiScreen`]), so the cursor is free and the
//!   keys type rather than move the player. Enter sends what's typed and
//!   closes it; Escape closes it.
//! - A line starting with `/` is a server command
//!   ([`ClientMessage::Command`], without the `/`); anything else is said
//!   to everyone ([`ClientMessage::Chat`]). The server checks and cleans
//!   both, so the field only stops typing at [`MAX_MESSAGE`] characters.
//! - What the server sends ([`ServerMessage::Chat`], and commands'
//!   [`ServerMessage::CommandReply`]) is kept in [`ChatLog`], the last
//!   [`MAX_HISTORY`] lines. The newest show over the game for
//!   [`SHOW_SECONDS`], fading out at the end; while the chat line is open,
//!   the last [`OPEN_LINES`] show whatever their age.

use std::collections::{HashSet, VecDeque};

use bevy::{
    input_focus::{FocusCause, InputFocus},
    prelude::*,
};

use crate::{
    hotbar_ui::hotbar_height,
    net::{FromServer, ToServer},
    session::GameState,
    ui::{TextField, UiScreen, UiTheme, playing, text_field, text_field::TextFieldInput, typing},
};
use defaria_core::{
    chat::{ChatKind, ChatLine, MAX_MESSAGE},
    net::{ClientMessage, ServerMessage},
};

/// The most lines kept.
pub const MAX_HISTORY: usize = 100;
/// The most lines shown over the game, while the chat line is closed.
pub const RECENT_LINES: usize = 10;
/// The most lines shown while it's open.
pub const OPEN_LINES: usize = 20;
/// How long a line shows over the game, fading out over the last second.
pub const SHOW_SECONDS: f32 = 10.0;
const FADE_SECONDS: f32 = 1.0;
/// How wide the chat is, in logical pixels.
const WIDTH: f32 = 520.0;
const LEFT: f32 = 8.0;

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChatLog>()
            .init_resource::<CommandIds>()
            .register_type::<ChatScreen>()
            .add_systems(Startup, spawn_chat)
            .add_systems(OnExit(GameState::Playing), forget_chat)
            .add_systems(
                Update,
                (
                    receive_lines,
                    open_chat
                        .run_if(in_state(GameState::Playing))
                        .run_if(playing)
                        .run_if(not(typing)),
                    send_line,
                    keep_focus,
                    (show_lines, fade_lines).chain(),
                    hide_out_of_play,
                )
                    .chain(),
            );
    }
}

/// What the server has sent, oldest first, with when each arrived
/// (seconds of real time).
#[derive(Resource, Debug, Default)]
pub struct ChatLog {
    pub lines: VecDeque<(ChatLine, f32)>,
}

impl ChatLog {
    fn push(&mut self, line: ChatLine, at: f32) {
        self.lines.push_back((line, at));
        while self.lines.len() > MAX_HISTORY {
            self.lines.pop_front();
        }
    }
}

/// The ids of the commands this client sends ([`ClientMessage::Command`]),
/// which their replies carry back: each sender takes one with
/// [`CommandIds::take`]. A sender whose answers aren't for the chat (a
/// headless scenario) marks them [`CommandIds::quiet`], and the chat
/// leaves their replies out.
#[derive(Resource, Debug, Default)]
pub struct CommandIds {
    last: u32,
    quiet: HashSet<u32>,
}

impl CommandIds {
    /// A new id for a command.
    pub fn take(&mut self) -> u32 {
        self.last = self.last.wrapping_add(1);
        self.last
    }

    /// A new id for a command whose reply stays out of the chat.
    pub fn quiet(&mut self) -> u32 {
        let id = self.take();
        self.quiet.insert(id);
        id
    }
}

/// The open chat line: a screen, and the text field typed into.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
#[require(UiScreen)]
pub struct ChatScreen;

/// Where chat shows: the lines, and the chat line under them when open.
#[derive(Component, Debug, Clone, Copy)]
struct ChatRoot;

/// The column of lines.
#[derive(Component, Debug, Clone, Copy)]
struct ChatLines;

/// A line on screen, and when it arrived.
#[derive(Component, Debug, Clone, Copy)]
struct ShownLine {
    at: f32,
    color: Color,
}

fn spawn_chat(mut commands: Commands, theme: Res<UiTheme>) {
    commands
        .spawn((
            Name::new("Chat"),
            ChatRoot,
            Node {
                position_type: PositionType::Absolute,
                left: px(LEFT),
                bottom: px(hotbar_height(&theme) + 8.0),
                width: px(WIDTH),
                max_width: percent(60),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|root| {
            root.spawn((
                ChatLines,
                Node {
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}

/// Chat belongs to a world: leaving it forgets the lines and closes the
/// chat line.
fn forget_chat(
    mut commands: Commands,
    mut log: ResMut<ChatLog>,
    mut ids: ResMut<CommandIds>,
    screens: Query<Entity, With<ChatScreen>>,
) {
    log.lines.clear();
    ids.quiet.clear();
    for screen in &screens {
        commands.entity(screen).try_despawn();
    }
}

fn receive_lines(
    time: Res<Time<Real>>,
    mut messages: MessageReader<FromServer>,
    mut log: ResMut<ChatLog>,
    mut ids: ResMut<CommandIds>,
) {
    let now = time.elapsed_secs();
    for FromServer(message) in messages.read() {
        let line = match message {
            ServerMessage::Chat(line) => line.clone(),
            ServerMessage::CommandReply(reply) => {
                if ids.quiet.remove(&reply.id) || reply.text.is_empty() {
                    continue;
                }
                reply.line()
            }
            _ => continue,
        };
        log.push(line, now);
    }
}

/// T opens the chat line.
fn open_chat(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    theme: Res<UiTheme>,
    root: Option<Single<Entity, With<ChatRoot>>>,
) {
    if !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    let Some(root) = root else {
        return;
    };
    commands.entity(*root).with_child((
        Name::new("Chat line"),
        ChatScreen,
        text_field(&theme, WIDTH, "Say something, or /help", MAX_MESSAGE),
    ));
}

/// Enter sends what's typed: a command if it starts with `/`; and closes
/// the chat line.
fn send_line(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    screens: Query<(Entity, &TextField), With<ChatScreen>>,
    mut to_server: MessageWriter<ToServer>,
    mut ids: ResMut<CommandIds>,
    mut focus: Option<ResMut<InputFocus>>,
) {
    if !keys.just_pressed(KeyCode::Enter) && !keys.just_pressed(KeyCode::NumpadEnter) {
        return;
    }
    for (screen, field) in &screens {
        if let Some(message) = message_for(&field.value, || ids.take()) {
            to_server.write(ToServer(message));
        }
        commands.entity(screen).try_despawn();
        if let Some(focus) = &mut focus {
            focus.clear();
        }
    }
}

/// What a typed line sends: a command for `/…`, chat otherwise, or
/// nothing for a blank line.
pub fn message_for(typed: &str, id: impl FnOnce() -> u32) -> Option<ClientMessage> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    Some(match typed.strip_prefix('/') {
        Some(command) => ClientMessage::Command {
            id: id(),
            line: command.trim().to_string(),
        },
        None => ClientMessage::Chat(typed.to_string()),
    })
}

/// While the chat line is open, it has the keyboard (clicking away
/// doesn't leave the player stuck with neither).
fn keep_focus(
    screens: Query<&Children, With<ChatScreen>>,
    inputs: Query<(), With<TextFieldInput>>,
    focus: Option<ResMut<InputFocus>>,
) {
    let Some(mut focus) = focus else {
        return;
    };
    for children in &screens {
        let Some(input) = children.iter().find(|&child| inputs.contains(child)) else {
            continue;
        };
        if focus.get() != Some(input) {
            focus.set(input, FocusCause::Auto);
        }
    }
}

/// The color a line of `kind` shows in.
fn line_color(theme: &UiTheme, kind: ChatKind) -> Color {
    match kind {
        ChatKind::Said => theme.text,
        ChatKind::Notice => Color::srgb(0.95, 0.85, 0.35),
        ChatKind::Output => theme.text_dim,
        ChatKind::Refused => Color::srgb(1.0, 0.45, 0.4),
    }
}

/// How a line reads.
pub fn line_text(line: &ChatLine) -> String {
    match &line.from {
        Some(from) => format!("<{from}> {}", line.text),
        None => line.text.clone(),
    }
}

/// Rebuilds the lines when the log changes or the chat line opens or
/// closes: the last few, or more while it's open.
fn show_lines(
    mut commands: Commands,
    theme: Res<UiTheme>,
    log: Res<ChatLog>,
    open: Query<(), With<ChatScreen>>,
    mut was_open: Local<bool>,
    column: Option<Single<Entity, With<ChatLines>>>,
) {
    let is_open = !open.is_empty();
    if !log.is_changed() && is_open == *was_open {
        return;
    }
    *was_open = is_open;
    let Some(column) = column else {
        return;
    };
    commands.entity(*column).despawn_related::<Children>();
    let count = if is_open { OPEN_LINES } else { RECENT_LINES };
    let start = log.lines.len().saturating_sub(count);
    for (line, at) in log.lines.iter().skip(start) {
        let color = line_color(&theme, line.kind);
        commands.entity(*column).with_child((
            ShownLine { at: *at, color },
            Node {
                padding: UiRect::axes(px(4), px(1)),
                ..default()
            },
            BackgroundColor(theme.slot),
            Pickable::IGNORE,
            children![(
                Text::new(line_text(line)),
                theme.text_font(),
                TextColor(color),
                Pickable::IGNORE,
            )],
        ));
    }
}

/// How visible a line that arrived `age` seconds ago is over the game.
pub fn line_alpha(age: f32) -> f32 {
    ((SHOW_SECONDS - age) / FADE_SECONDS).clamp(0.0, 1.0)
}

/// Lines fade out over the game as they age; with the chat line open,
/// they all show.
fn fade_lines(
    time: Res<Time<Real>>,
    theme: Res<UiTheme>,
    open: Query<(), With<ChatScreen>>,
    mut lines: Query<(&ShownLine, &mut BackgroundColor, &Children)>,
    mut texts: Query<&mut TextColor>,
) {
    let now = time.elapsed_secs();
    let is_open = !open.is_empty();
    for (line, mut background, children) in &mut lines {
        let alpha = if is_open {
            1.0
        } else {
            line_alpha(now - line.at)
        };
        let backdrop = theme.slot;
        background.set_if_neq(BackgroundColor(
            backdrop.with_alpha(backdrop.alpha() * alpha),
        ));
        for &child in children {
            if let Ok(mut text) = texts.get_mut(child) {
                text.set_if_neq(TextColor(line.color.with_alpha(line.color.alpha() * alpha)));
            }
        }
    }
}

/// Chat shows only in a world, not at the menu.
fn hide_out_of_play(
    state: Option<Res<State<GameState>>>,
    mut roots: Query<&mut Visibility, With<ChatRoot>>,
) {
    let wanted = match state.map(|state| *state.get()) {
        Some(GameState::Menu) => Visibility::Hidden,
        _ => Visibility::Inherited,
    };
    for mut visibility in &mut roots {
        visibility.set_if_neq(wanted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_line_is_chat_or_a_command() {
        assert_eq!(
            message_for("  hello there ", || 1),
            Some(ClientMessage::Chat("hello there".into()))
        );
        assert_eq!(
            message_for("/fill 1 2 3 4 5 6 stone", || 3),
            Some(ClientMessage::Command {
                id: 3,
                line: "fill 1 2 3 4 5 6 stone".into()
            })
        );
        assert_eq!(message_for("   ", || 1), None);
    }

    #[test]
    fn lines_fade_out_at_the_end() {
        assert_eq!(line_alpha(0.0), 1.0);
        assert_eq!(line_alpha(SHOW_SECONDS - FADE_SECONDS), 1.0);
        assert!((line_alpha(SHOW_SECONDS - FADE_SECONDS / 2.0) - 0.5).abs() < 1e-4);
        assert_eq!(line_alpha(SHOW_SECONDS + 5.0), 0.0);
    }

    #[test]
    fn the_log_keeps_the_last_lines() {
        let mut log = ChatLog::default();
        for n in 0..MAX_HISTORY + 5 {
            let line = ChatLine {
                kind: ChatKind::Said,
                from: Some("Ada".into()),
                text: n.to_string(),
            };
            log.push(line, 0.0);
        }
        assert_eq!(log.lines.len(), MAX_HISTORY);
        assert_eq!(log.lines[0].0.text, "5");
        assert_eq!(line_text(&log.lines[0].0), "<Ada> 5");
    }
}
