//! What both sides of the game know: content and mods, the world's chunks
//! and how they're read and written ([`world::Voxels`]), physics and the
//! player model, the messages between server and client ([`net`]) and
//! what they carry (inventories, the hotbar, machines' parts, light), and
//! the rules a client predicts by. What only the server runs (terrain,
//! streaming and saving, light, fluid flow, networks, machines, scripts,
//! players' connections) is `defaria_server`; drawing, UI and input are
//! `defaria_client`. Nothing here draws, shows UI or reads input.

pub mod block_edit;
pub mod block_entity;
pub mod content;
pub mod fluid;
pub mod fluid_container;
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
pub mod streaming;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod use_action;
pub mod world;
