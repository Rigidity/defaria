//! The commands' one parser: a chat line's after its `/`, the console's,
//! and a headless scenario's (whose script parser hands these lines on).

use bevy::prelude::*;

/// A server command, parsed. See [`HELP`] for what each does.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerCommand {
    /// Set every voxel in a box (its lowest and highest corners) to a
    /// block, named as in a save (see `scene::fill_state`).
    Fill {
        min: IVec3,
        max: IVec3,
        block: String,
    },
    /// Put `amount` blocks of a fluid in a voxel.
    Pour {
        pos: IVec3,
        fluid: String,
        amount: f32,
    },
    /// Put items in a machine's slot group, or fluid in its tank.
    Put {
        pos: IVec3,
        /// The slot group or tank.
        name: String,
        /// Items, or blocks of fluid.
        amount: f32,
        /// The item or fluid, named as for `fill`.
        what: String,
    },
    /// Put an item in the player's hotbar slot (from 0) and in hand.
    Hotbar { slot: usize, item: String },
    /// Put items in the player's inventory, each holding a fluid if one's
    /// given (a fluid container's), and how many blocks of it.
    Give {
        item: String,
        count: u32,
        fluid: Option<(String, f32)>,
    },
    /// Put the player's eyes here.
    Teleport(Vec3),
    /// Voxels picked for random ticks in each chunk each tick.
    RandomTicks(u32),
    /// How many times faster than their rate films evaporate.
    Evaporation(f32),
    /// Count the fluid in a box, by its corners (min, max), or the
    /// player's window.
    Fluids(Option<(IVec3, IVec3)>),
    /// Describe the network of the conduit at a voxel.
    Network(IVec3),
    /// Save the world now.
    Save,
    /// Save the world and stop the server.
    Stop,
    /// Who's playing.
    List,
    /// What the commands are.
    Help,
    /// Say something to everyone, as the server (from the console).
    Say(String),
    /// Let a player (an online player's name, or an id) run commands.
    Op(String),
    /// Stop letting them.
    Deop(String),
}

/// What `help` says.
pub const HELP: &str = "commands: \
fill <x0 y0 z0 x1 y1 z1> <block>, \
pour <x y z> <fluid> <amount>, \
put <x y z> <slot group or tank> <amount> <item or fluid>, \
hotbar <slot 1-9> <item>, \
give <item> <count> [<fluid> <amount>], \
tp <x y z>, \
random-ticks <n>, evaporation <speed>, \
fluids [<x0 y0 z0 x1 y1 z1>], network <x y z>, \
save, stop, list, say <text>, op <player>, deop <player>, help";

/// The words commands start with, for a scenario's parser to tell its
/// own lines from these.
pub const NAMES: &[&str] = &[
    "fill",
    "pour",
    "put",
    "hotbar",
    "give",
    "tp",
    "random-ticks",
    "evaporation",
    "fluids",
    "network",
    "save",
    "stop",
    "list",
    "help",
    "say",
    "op",
    "deop",
];

impl ServerCommand {
    /// Whether anyone may run it: only `list` and `help`, which change
    /// nothing and tell nothing secret. The rest are for those allowed
    /// (see `ops`).
    pub fn open_to_all(&self) -> bool {
        matches!(self, ServerCommand::List | ServerCommand::Help)
    }
}

/// The command `line` says (a leading `/` is allowed).
pub fn parse(line: &str) -> Result<ServerCommand, String> {
    let line = line.trim();
    let line = line.strip_prefix('/').unwrap_or(line).trim_start();
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some(&name) = words.first() else {
        return Err("no command given; `help` lists them".into());
    };
    let args = &words[1..];
    let expect = |count: usize| {
        if args.len() == count {
            Ok(())
        } else {
            Err(format!("`{name}` takes {count} argument(s): {line}"))
        }
    };
    let rest = || line[name.len()..].trim().to_string();
    Ok(match name {
        "fill" => {
            expect(7)?;
            let (min, max) = parse_box(&args[..6])?;
            ServerCommand::Fill {
                min,
                max,
                block: args[6].to_string(),
            }
        }
        "pour" => {
            expect(5)?;
            ServerCommand::Pour {
                pos: parse_pos(&args[..3])?,
                fluid: args[3].to_string(),
                amount: parse_number(args[4])?,
            }
        }
        "put" => {
            expect(6)?;
            ServerCommand::Put {
                pos: parse_pos(&args[..3])?,
                name: args[3].to_string(),
                amount: parse_number(args[4])?,
                what: args[5].to_string(),
            }
        }
        "hotbar" => {
            expect(2)?;
            let slot: usize = parse_number(args[0])?;
            if !(1..=defaria_core::hotbar::HOTBAR_SLOTS).contains(&slot) {
                return Err(format!("`hotbar` slots are 1 to 9: {line}"));
            }
            ServerCommand::Hotbar {
                slot: slot - 1,
                item: args[1].to_string(),
            }
        }
        "give" => {
            if args.len() != 2 && args.len() != 4 {
                return Err(format!(
                    "`give` takes an item and a count, and maybe a fluid and an amount: {line}"
                ));
            }
            let fluid = match args.get(2..4) {
                Some([fluid, amount]) => Some((fluid.to_string(), parse_number(amount)?)),
                _ => None,
            };
            ServerCommand::Give {
                item: args[0].to_string(),
                count: parse_number(args[1])?,
                fluid,
            }
        }
        "tp" => {
            expect(3)?;
            let at = Vec3::new(
                parse_number(args[0])?,
                parse_number(args[1])?,
                parse_number(args[2])?,
            );
            if !at.is_finite() {
                return Err(format!("`tp` takes a place: {line}"));
            }
            ServerCommand::Teleport(at)
        }
        "random-ticks" => {
            expect(1)?;
            ServerCommand::RandomTicks(parse_number(args[0])?)
        }
        "evaporation" => {
            expect(1)?;
            let speed: f32 = parse_number(args[0])?;
            if !(speed.is_finite() && speed >= 0.0) {
                return Err(format!("`{}` isn't a speed", args[0]));
            }
            ServerCommand::Evaporation(speed)
        }
        "fluids" => ServerCommand::Fluids(match args.len() {
            0 => None,
            6 => Some(parse_box(args)?),
            _ => return Err(format!("`fluids` takes no arguments or 6: {line}")),
        }),
        "network" => {
            expect(3)?;
            ServerCommand::Network(parse_pos(args)?)
        }
        "save" => {
            expect(0)?;
            ServerCommand::Save
        }
        "stop" => {
            expect(0)?;
            ServerCommand::Stop
        }
        "list" => {
            expect(0)?;
            ServerCommand::List
        }
        "help" => ServerCommand::Help,
        "say" => {
            let text = rest();
            if text.is_empty() {
                return Err("`say` takes something to say".into());
            }
            ServerCommand::Say(text)
        }
        "op" | "deop" => {
            let who = rest();
            if who.is_empty() {
                return Err(format!("`{name}` takes a player's name or id"));
            }
            if name == "op" {
                ServerCommand::Op(who)
            } else {
                ServerCommand::Deop(who)
            }
        }
        other => return Err(format!("unknown command `{other}`; `help` lists them")),
    })
}

/// A voxel, `x y z`.
fn parse_pos(args: &[&str]) -> Result<IVec3, String> {
    Ok(IVec3::new(
        parse_number(args[0])?,
        parse_number(args[1])?,
        parse_number(args[2])?,
    ))
}

/// Two corners, `x0 y0 z0 x1 y1 z1`, as the box's lowest and highest.
fn parse_box(args: &[&str]) -> Result<(IVec3, IVec3), String> {
    let (a, b) = (parse_pos(&args[..3])?, parse_pos(&args[3..6])?);
    Ok((a.min(b), a.max(b)))
}

fn parse_number<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("`{text}` isn't a valid number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        let cases = [
            (
                "fill 3 9 -2 1 4 5 red_lamp",
                ServerCommand::Fill {
                    min: IVec3::new(1, 4, -2),
                    max: IVec3::new(3, 9, 5),
                    block: "red_lamp".into(),
                },
            ),
            (
                "/pour 4 -5 6 water 0.25",
                ServerCommand::Pour {
                    pos: IVec3::new(4, -5, 6),
                    fluid: "water".into(),
                    amount: 0.25,
                },
            ),
            (
                "put 1 2 3 input 4 stone[ore=defaria:iron]",
                ServerCommand::Put {
                    pos: IVec3::new(1, 2, 3),
                    name: "input".into(),
                    amount: 4.0,
                    what: "stone[ore=defaria:iron]".into(),
                },
            ),
            (
                "hotbar 3 wrench",
                ServerCommand::Hotbar {
                    slot: 2,
                    item: "wrench".into(),
                },
            ),
            (
                "give bucket 2",
                ServerCommand::Give {
                    item: "bucket".into(),
                    count: 2,
                    fluid: None,
                },
            ),
            (
                "give bucket 1 water 0.5",
                ServerCommand::Give {
                    item: "bucket".into(),
                    count: 1,
                    fluid: Some(("water".into(), 0.5)),
                },
            ),
            (
                "tp 1 2.5 -3",
                ServerCommand::Teleport(Vec3::new(1.0, 2.5, -3.0)),
            ),
            ("random-ticks 64", ServerCommand::RandomTicks(64)),
            ("evaporation 20", ServerCommand::Evaporation(20.0)),
            ("fluids", ServerCommand::Fluids(None)),
            (
                "fluids 4 -5 6 1 2 3",
                ServerCommand::Fluids(Some((IVec3::new(1, -5, 3), IVec3::new(4, 2, 6)))),
            ),
            (
                "network 1 -2 3",
                ServerCommand::Network(IVec3::new(1, -2, 3)),
            ),
            ("save", ServerCommand::Save),
            (" / stop ", ServerCommand::Stop),
            ("list", ServerCommand::List),
            ("help", ServerCommand::Help),
            ("say  hello  all ", ServerCommand::Say("hello  all".into())),
            ("op Ada Lovelace", ServerCommand::Op("Ada Lovelace".into())),
            ("deop Ada", ServerCommand::Deop("Ada".into())),
        ];
        for (line, command) in cases {
            assert_eq!(parse(line), Ok(command), "{line}");
            let name = line.trim().trim_start_matches(['/', ' ']);
            let name = name.split_whitespace().next().unwrap();
            assert!(NAMES.contains(&name), "{name}");
        }
    }

    #[test]
    fn mistakes_are_errors() {
        for bad in [
            "",
            "/",
            "jump",
            "fill 1 2 3 4 5 6",
            "fill 1 2 3 4 5 x stone",
            "pour 1 2 3 water",
            "pour 1 2 3 water lots",
            "hotbar 0 wrench",
            "hotbar 10 wrench",
            "hotbar 1",
            "give bucket",
            "give bucket 1 water",
            "give bucket lots",
            "tp 1 2",
            "tp 1 2 inf",
            "random-ticks many",
            "evaporation -1",
            "evaporation NaN",
            "fluids all",
            "fluids 1 2 3",
            "network 1 2",
            "save now",
            "say",
            "op",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
