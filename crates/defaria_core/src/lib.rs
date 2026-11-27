//! What the game is, apart from how it's shown and played: content and
//! mods, the world and its chunks, terrain, light, fluids, networks,
//! machines, scripts, players and their intents, and saving. Nothing here
//! draws, shows UI or reads input; that's `defaria_client`.

pub mod block_edit;
pub mod block_entity;
pub mod connections;
pub mod content;
pub mod fluid;
pub mod fluid_container;
pub mod fluid_network;
pub mod game_mode;
pub mod hotbar;
pub mod inventory;
pub mod light;
pub mod machine;
pub mod mining;
pub mod net;
pub mod network;
pub mod perlin;
pub mod physics;
pub mod player;
pub mod recipe;
pub mod replication;
pub mod save;
pub mod script;
pub mod server;
pub mod streaming;
pub mod support;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod use_action;
pub mod world;
pub mod worldgen;
pub mod wrench;
