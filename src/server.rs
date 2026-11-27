//! The server app: built here for the server in the game's own process
//! (`integrated`), a dedicated server (`dedicated`) and tests.

use bevy::{app::PluginsState, prelude::*};

use crate::ServerPlugin;

/// Adds the server's plugins to `app`, which has whatever settings and
/// content it should start with already, and finishes setting it up, ready
/// to update.
pub fn build_server(app: &mut App) {
    app.add_plugins((
        // Sixty frames a second, run on its own (a dedicated server); the
        // server in the game's own process is stepped by the client.
        MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        )),
        TransformPlugin,
        AssetPlugin::default(),
        ServerPlugin,
    ));
    while app.plugins_state() == PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
}
