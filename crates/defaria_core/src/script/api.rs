//! What hooks on either side are handed to read the world with: block
//! handles ([`BlockRef`]), the `world` global, and plain values to and
//! from Lua. Each side adds its own writes: the server's changes blocks
//! and keeps their `data` (`defaria_server::script::api`); a client's
//! world is read-only ([`call_read_only`]). Documented for modders in
//! `docs/scripting.md`.

mod block;
mod value;
mod world;

pub use block::{
    BlockRef, Located, add_block_fields, add_block_methods, face_named, handle_pos, position,
    vector,
};
pub use value::{from_lua, table_to_lua, to_lua};
pub(crate) use world::world_table;
pub use world::{
    ChunkReader, Op, WorldView, call_read_only, current_serial, read, state_table, with_world,
    world_op,
};
