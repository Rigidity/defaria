//! Machines' parts, as both sides know them: what a machine's block
//! entity keeps ([`EnergyBuffer`], [`Tanks`], [`Slots`], [`MachineActive`]),
//! which the client is sent for a machine's screen, and [`exchange`],
//! moving items between a machine and the player. Giving machines their
//! parts and keeping them up to date is the server's
//! (`defaria_server::machine`).

pub mod exchange;
mod parts;

pub use parts::{EnergyBuffer, MachineActive, SLOT_SIZE, SlotGroup, Slots, Tank, Tanks, slot_size};

/// Whether `machine` has a screen: slots, tanks or recipes. Its energy
/// shows on the screen of one that has, but on its own isn't worth taking
/// right click from placing.
pub fn has_screen(machine: &crate::content::Machine) -> bool {
    !machine.slots.is_empty() || !machine.tanks.is_empty() || machine.recipes.is_some()
}
