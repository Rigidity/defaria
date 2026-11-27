//! The script format and its parser. See the [module docs](super) for
//! the commands.

use std::path::PathBuf;

use bevy::{
    prelude::*,
    reflect::enums::{DynamicEnum, DynamicVariant},
};

/// A parsed script. See the module docs for the format.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Script {
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Wait(u32),
    Press(Button),
    Release(Button),
    MoveMouse(Vec2),
    Cursor(Vec2),
    Scroll(f32),
    Screenshot(PathBuf),
    Grain,
    Inspect(String),
    Fill {
        min: IVec3,
        max: IVec3,
        block: String,
    },
    Pour {
        pos: IVec3,
        fluid: String,
        /// In blocks.
        amount: f32,
    },
    /// Count the fluid in a box, by its corners (min, max), or the camera's
    /// window.
    Fluids(Option<(IVec3, IVec3)>),
    Hotbar {
        /// From 0.
        slot: usize,
        item: String,
    },
    /// Put items in the player's inventory.
    Give {
        item: String,
        count: u32,
        /// For a fluid container, the fluid each holds and how many
        /// blocks of it.
        fluid: Option<(String, f32)>,
    },
    Network(IVec3),
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
    Camera {
        position: Vec3,
        /// Degrees left of north.
        yaw: f32,
        /// Degrees up.
        pitch: f32,
    },
    Timing,
    /// Report this many of the busiest frame's systems.
    Systems(usize),
    Settle,
    Memory,
    Lookups {
        min: IVec3,
        max: IVec3,
    },
    /// Type this text, a key press and release per character.
    Type(String),
    /// Voxels picked for random ticks in each chunk each tick.
    RandomTicks(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Button {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl Script {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("can't read {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut commands = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            parse_line(line, &mut commands)
                .map_err(|error| format!("line {}: {error}", index + 1))?;
        }
        Ok(Self { commands })
    }
}

fn parse_line(line: &str, commands: &mut Vec<Command>) -> Result<(), String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let args = &words[1..];
    let expect = |count: usize| {
        if args.len() == count {
            Ok(())
        } else {
            Err(format!("`{}` takes {count} argument(s): {line}", words[0]))
        }
    };
    match words[0] {
        "type" => {
            let text = line.trim_start()["type".len()..].trim();
            if text.is_empty() {
                return Err(format!("`type` takes some text: {line}"));
            }
            commands.push(Command::Type(text.to_owned()));
        }
        "wait" => {
            expect(1)?;
            commands.push(Command::Wait(parse_number(args[0])?));
        }
        "press" => {
            expect(1)?;
            commands.push(Command::Press(parse_button(args[0])?));
        }
        "release" => {
            expect(1)?;
            commands.push(Command::Release(parse_button(args[0])?));
        }
        "hold" => {
            expect(2)?;
            let button = parse_button(args[0])?;
            commands.extend([
                Command::Press(button),
                Command::Wait(parse_number(args[1])?),
                Command::Release(button),
            ]);
        }
        "mouse" => {
            expect(2)?;
            commands.push(Command::MoveMouse(Vec2::new(
                parse_number(args[0])?,
                parse_number(args[1])?,
            )));
        }
        "cursor" => {
            expect(2)?;
            commands.push(Command::Cursor(Vec2::new(
                parse_number(args[0])?,
                parse_number(args[1])?,
            )));
        }
        "scroll" => {
            expect(1)?;
            commands.push(Command::Scroll(parse_number(args[0])?));
        }
        "screenshot" => {
            expect(1)?;
            commands.push(Command::Screenshot(PathBuf::from(args[0])));
        }
        "grain" => {
            expect(0)?;
            commands.push(Command::Grain);
        }
        "inspect" => {
            expect(1)?;
            commands.push(Command::Inspect(args[0].to_string()));
        }
        "fill" => {
            expect(7)?;
            let (min, max) = parse_box(&args[..6])?;
            commands.push(Command::Fill {
                min,
                max,
                block: args[6].to_string(),
            });
        }
        "pour" => {
            expect(5)?;
            commands.push(Command::Pour {
                pos: IVec3::new(
                    parse_number(args[0])?,
                    parse_number(args[1])?,
                    parse_number(args[2])?,
                ),
                fluid: args[3].to_string(),
                amount: parse_number(args[4])?,
            });
        }
        "put" => {
            expect(6)?;
            commands.push(Command::Put {
                pos: IVec3::new(
                    parse_number(args[0])?,
                    parse_number(args[1])?,
                    parse_number(args[2])?,
                ),
                name: args[3].to_string(),
                amount: parse_number(args[4])?,
                what: args[5].to_string(),
            });
        }
        "fluids" => {
            let region = match args.len() {
                0 => None,
                6 => Some(parse_box(args)?),
                _ => return Err(format!("`fluids` takes no arguments or 6: {line}")),
            };
            commands.push(Command::Fluids(region));
        }
        "hotbar" => {
            expect(2)?;
            let slot: usize = parse_number(args[0])?;
            if !(1..=crate::hotbar::HOTBAR_SLOTS).contains(&slot) {
                return Err(format!("`hotbar` slots are 1 to 9: {line}"));
            }
            commands.push(Command::Hotbar {
                slot: slot - 1,
                item: args[1].to_string(),
            });
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
            commands.push(Command::Give {
                item: args[0].to_string(),
                count: parse_number(args[1])?,
                fluid,
            });
        }
        "random-ticks" => {
            expect(1)?;
            commands.push(Command::RandomTicks(parse_number(args[0])?));
        }
        "network" => {
            expect(3)?;
            commands.push(Command::Network(IVec3::new(
                parse_number(args[0])?,
                parse_number(args[1])?,
                parse_number(args[2])?,
            )));
        }
        "camera" => {
            expect(5)?;
            commands.push(Command::Camera {
                position: Vec3::new(
                    parse_number(args[0])?,
                    parse_number(args[1])?,
                    parse_number(args[2])?,
                ),
                yaw: parse_number(args[3])?,
                pitch: parse_number(args[4])?,
            });
        }
        "timing" => {
            expect(0)?;
            commands.push(Command::Timing);
        }
        "systems" => {
            let count = match args {
                [] => 12,
                [count] => parse_number(count)?,
                _ => return Err(format!("`systems` takes a count or nothing: {line}")),
            };
            commands.push(Command::Systems(count));
        }
        "settle" => {
            expect(0)?;
            commands.push(Command::Settle);
        }
        "memory" => {
            expect(0)?;
            commands.push(Command::Memory);
        }
        "lookups" => {
            expect(6)?;
            let (min, max) = parse_box(args)?;
            commands.push(Command::Lookups { min, max });
        }
        other => return Err(format!("unknown command `{other}`")),
    }
    Ok(())
}

/// Two corners, `x0 y0 z0 x1 y1 z1`, as the box's lowest and highest.
fn parse_box(args: &[&str]) -> Result<(IVec3, IVec3), String> {
    let corner = |at: usize| -> Result<IVec3, String> {
        Ok(IVec3::new(
            parse_number(args[at])?,
            parse_number(args[at + 1])?,
            parse_number(args[at + 2])?,
        ))
    };
    let (a, b) = (corner(0)?, corner(3)?);
    Ok((a.min(b), a.max(b)))
}

fn parse_number<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("`{text}` isn't a valid number"))
}

pub(super) fn parse_button(name: &str) -> Result<Button, String> {
    match name {
        "MouseLeft" => return Ok(Button::Mouse(MouseButton::Left)),
        "MouseRight" => return Ok(Button::Mouse(MouseButton::Right)),
        "MouseMiddle" => return Ok(Button::Mouse(MouseButton::Middle)),
        _ => {}
    }
    // Any unit variant of KeyCode, by name, via reflection.
    KeyCode::from_reflect(&DynamicEnum::new(name, DynamicVariant::Unit))
        .map(Button::Key)
        .ok_or_else(|| {
            format!("unknown button `{name}` (use KeyCode names like `KeyW`, or `MouseLeft`)")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        let script = Script::parse(
            "# comment\n\
             wait 2\n\
             press KeyW # trailing comment\n\
             release Space\n\
             hold MouseLeft 3\n\
             mouse 10 -5.5\n\
             cursor 640 360.5\n\
             scroll -2\n\
             \n\
             screenshot target/a.png\n\
             grain\n\
             inspect Transform\n\
             fill 3 9 -2 1 4 5 red_lamp\n\
             camera 1 2.5 3 90 -45\n\
             timing\n\
             systems\n\
             systems 5\n\
             settle\n\
             pour 4 -5 6 water 0.25\n\
             fluids\n\
             fluids 4 -5 6 1 2 3\n\
             network 1 -2 3\n\
             put 1 2 3 input 4 stone[ore=defaria:iron]
             type  Iron ore 2 \n\
             random-ticks 64\n\
             memory\n\
             lookups 5 0 -1 1 2 3\n\
             give bucket 2\n\
             give bucket 1 water 0.5\n",
        )
        .unwrap();
        assert_eq!(
            script.commands,
            vec![
                Command::Wait(2),
                Command::Press(Button::Key(KeyCode::KeyW)),
                Command::Release(Button::Key(KeyCode::Space)),
                Command::Press(Button::Mouse(MouseButton::Left)),
                Command::Wait(3),
                Command::Release(Button::Mouse(MouseButton::Left)),
                Command::MoveMouse(Vec2::new(10.0, -5.5)),
                Command::Cursor(Vec2::new(640.0, 360.5)),
                Command::Scroll(-2.0),
                Command::Screenshot(PathBuf::from("target/a.png")),
                Command::Grain,
                Command::Inspect("Transform".into()),
                Command::Fill {
                    min: IVec3::new(1, 4, -2),
                    max: IVec3::new(3, 9, 5),
                    block: "red_lamp".into(),
                },
                Command::Camera {
                    position: Vec3::new(1.0, 2.5, 3.0),
                    yaw: 90.0,
                    pitch: -45.0,
                },
                Command::Timing,
                Command::Systems(12),
                Command::Systems(5),
                Command::Settle,
                Command::Pour {
                    pos: IVec3::new(4, -5, 6),
                    fluid: "water".into(),
                    amount: 0.25,
                },
                Command::Fluids(None),
                Command::Fluids(Some((IVec3::new(1, -5, 3), IVec3::new(4, 2, 6)))),
                Command::Network(IVec3::new(1, -2, 3)),
                Command::Put {
                    pos: IVec3::new(1, 2, 3),
                    name: "input".into(),
                    amount: 4.0,
                    what: "stone[ore=defaria:iron]".into(),
                },
                Command::Type("Iron ore 2".into()),
                Command::RandomTicks(64),
                Command::Memory,
                Command::Lookups {
                    min: IVec3::new(1, 0, -1),
                    max: IVec3::new(5, 2, 3),
                },
                Command::Give {
                    item: "bucket".into(),
                    count: 2,
                    fluid: None,
                },
                Command::Give {
                    item: "bucket".into(),
                    count: 1,
                    fluid: Some(("water".into(), 0.5)),
                },
            ]
        );
    }

    #[test]
    fn parse_errors_name_the_line() {
        let error = Script::parse("wait 1\npress KeyQQ").unwrap_err();
        assert!(error.starts_with("line 2:"), "{error}");
        assert!(Script::parse("jump").is_err());
        assert!(Script::parse("type").is_err());
        assert!(Script::parse("wait").is_err());
        assert!(Script::parse("wait soon").is_err());
        assert!(Script::parse("grain 3").is_err());
        assert!(Script::parse("network 1 2").is_err());
        assert!(Script::parse("hotbar 0 wrench").is_err());
        assert!(Script::parse("hotbar 10 wrench").is_err());
        assert!(Script::parse("hotbar 1").is_err());
        assert!(Script::parse("fill 1 2 3 4 5 6").is_err());
        assert!(Script::parse("fill 1 2 3 4 5 x stone").is_err());
        assert!(Script::parse("camera 1 2 3 4").is_err());
        assert!(Script::parse("timing now").is_err());
        assert!(Script::parse("settle 5").is_err());
        assert!(Script::parse("cursor 4").is_err());
        assert!(Script::parse("scroll").is_err());
        assert!(Script::parse("pour 1 2 3 water").is_err());
        assert!(Script::parse("pour 1 2 3 water lots").is_err());
        assert!(Script::parse("fluids all").is_err());
        assert!(Script::parse("systems all").is_err());
        assert!(Script::parse("systems 1 2").is_err());
        assert!(Script::parse("fluids 1 2 3").is_err());
        assert!(Script::parse("fluids 1 2 3 4 5 six").is_err());
        assert!(Script::parse("random-ticks").is_err());
        assert!(Script::parse("random-ticks many").is_err());
        assert!(Script::parse("memory all").is_err());
        assert!(Script::parse("lookups 1 2 3 4 5").is_err());
        assert!(Script::parse("give bucket").is_err());
        assert!(Script::parse("give bucket 1 water").is_err());
        assert!(Script::parse("give bucket lots").is_err());
    }
}
