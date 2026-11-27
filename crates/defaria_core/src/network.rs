//! The network clock's rate, which both sides count in: the server's
//! networks and machines run on network ticks (`defaria_server::network`),
//! and recipes' times are baked into ticks with the content.

/// Network ticks a second, for every solver (see `docs/decisions.md`).
pub const TICKS_PER_SECOND: u32 = 20;
