//! What only the server app runs: generating terrain, streaming chunks in
//! and out and saving them, light propagation, fluid flow, networks and
//! their solvers, machines and recipes, mod scripts and their hooks,
//! mining, placing and what breaks with it, and each client's connection
//! ([`server`]). The types both sides share are `defaria_core`; a module
//! here named as one there has that one's systems.

pub mod block_edit;
pub mod block_entity;
pub mod connections;
pub mod fluid;
pub mod fluid_container;
pub mod fluid_network;
pub mod game_mode;
pub mod inventory;
pub mod light;
pub mod machine;
pub mod mining;
pub mod network;
pub mod recipe;
pub mod save;
pub mod script;
pub mod server;
pub mod streaming;
pub mod support;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod worldgen;
pub mod wrench;
