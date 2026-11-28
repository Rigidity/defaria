//! The server's side of mod scripts: the hooks only the server calls,
//! and what its scripts can change. The runtime they run in, what every
//! side can read, and the message channel are `defaria_core::script`'s;
//! the format and the rules for modders are in `docs/scripting.md`.
//!
//! [`api`] is what the server's hooks are handed besides what every side
//! reads (writing blocks, blocks' `data`, machines' handles, players and
//! items), and [`ScriptWorld::call`] calls a hook with it; [`hooks`]
//! calls blocks' and items' scripts as things happen to them,
//! [`random_tick`] gives blocks random ticks, [`machine`] calls
//! machines' `on_tick` and `on_recipe_finished`, and [`message`] calls
//! `on_message` for what clients' scripts send and sends what the
//! server's send.
//!
//! Scripts only ever change game state through what they're handed, never
//! rendering or UI. Calls into them belong in `FixedUpdate`, with the
//! simulation (messages aside, which are taken as they arrive).

pub mod api;
pub mod hooks;
pub mod machine;
pub mod message;
pub mod random_tick;
pub mod using;

use bevy::prelude::*;

pub use api::ScriptWorld;
use defaria_core::script::{ScriptRuntimePlugin, Side};
pub use hooks::{BlockUsed, HookSystems, ItemUsed, ScriptIndex};

pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ScriptRuntimePlugin { side: Side::Server });
        hooks::plugin(app);
        using::plugin(app);
        message::plugin(app);
    }
}
