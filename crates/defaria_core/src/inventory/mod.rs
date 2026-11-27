//! Inventories: what's carried (the screen for it is `inventory_screen`).
//!
//! - [`model`]: [`Inventory`], how many of each item ([`ItemStack`]s),
//!   limited only by an optional weight, as a component anything can
//!   have.
//! - [`player`]: what a new player starts with, and its weight limit
//!   (the server gives players theirs and saves them:
//!   `defaria_server::inventory`).
//! - [`transfer`]: moving items between inventories and the catalog.

pub mod model;
pub mod player;
pub mod transfer;

pub use model::{Inventory, ItemStack};
