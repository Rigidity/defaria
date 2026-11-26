//! Networks: conduits (wires, pipes, tubes) joined into networks of one
//! kind each, kept up to date as blocks change and chunks load and unload.
//! Nothing moves along them here; that's the solvers' (energy, fluid,
//! items), which keep what each network holds with [`NetworkState`].
//!
//! A conduit is a block with a `conduit` (see "Conduits and networks" in
//! `docs/modding.md`). Two conduits are on the same network when their
//! connection properties join them, which happens when they're on the same
//! network kind and group ([`Content::accepts`]). The networks follow those
//! properties, so this runs after the `connections` plugin sets them, in
//! `Update`, for:
//!
//! - every voxel that changed ([`VoxelChanged`]) or had its properties
//!   updated ([`PropertiesUpdated`]), and the six around it;
//! - every conduit in a chunk that just loaded, and those on the faces of
//!   the loaded chunks next to it;
//! - every conduit in a chunk that unloaded, which leaves its networks.
//!
//! A network whose conduit is joined on a face into a chunk that isn't
//! loaded is frozen ([`Network::is_frozen`]): it may go on out there, so
//! its solver skips it, keeping what it holds, until the chunk loads.
//! Networks are rebuilt from the chunks as they load; nothing extra is
//! saved. See `docs/decisions.md`.

mod graph;
mod state;

use bevy::prelude::*;

pub use graph::{Network, NetworkChanged, NetworkId, Networks, Port, Share};
pub use state::{NetworkAppExt, NetworkMap, NetworkState, divide, divide_by_share};

use crate::{
    connections::{ConnectionSystems, face_voxel},
    content::{BlockState, Content, Direction},
    terrain::{ChunkCoord, ChunkMap, PropertiesUpdated, VoxelChanged},
    voxel::{CHUNK_SIZE, Chunk},
};

pub struct NetworkPlugin;

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Networks>()
            .init_resource::<LoadedChunks>()
            .add_observer(note_loaded_chunk)
            .add_message::<NetworkChanged>()
            .add_systems(
                Update,
                update_networks
                    .in_set(NetworkSystems)
                    .after(ConnectionSystems),
            );
    }
}

/// Where the networks are brought up to date: after the connection
/// properties are. Systems that keep something per network run after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetworkSystems;

/// Chunks spawned since [`update_networks`] last ran, noted as they're
/// added: an `Added<Chunk>` query would look at every loaded chunk on
/// every frame.
#[derive(Resource, Debug, Default)]
struct LoadedChunks(Vec<Entity>);

fn note_loaded_chunk(add: On<Add<Chunk>>, mut loaded: ResMut<LoadedChunks>) {
    loaded.0.push(add.entity);
}

/// Re-evaluates the voxels that may have changed a network.
fn update_networks(
    mut edits: MessageReader<VoxelChanged>,
    mut updates: MessageReader<PropertiesUpdated>,
    mut loaded: ResMut<LoadedChunks>,
    coords: Query<&ChunkCoord>,
    chunks: Query<&Chunk>,
    map: Res<ChunkMap>,
    content: Res<Content>,
    mut networks: ResMut<Networks>,
    mut changed: MessageWriter<NetworkChanged>,
) {
    let world = |pos: IVec3| -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = chunks.get(map.get(coord)?).ok()?;
        Some(chunk.at(local))
    };
    let mut dirty = Vec::new();
    let around = |pos: IVec3, dirty: &mut Vec<IVec3>| {
        dirty.push(pos);
        dirty.extend(Direction::ALL.map(|side| pos + side.normal()));
    };
    for edit in edits.read() {
        around(edit.pos, &mut dirty);
    }
    for update in updates.read() {
        around(update.pos, &mut dirty);
    }
    // Chunks that unloaded: their conduits leave, and the links into them
    // open.
    let gone: Vec<IVec3> = networks
        .chunks()
        .filter(|&coord| map.get(ChunkCoord(coord)).is_none())
        .collect();
    for coord in gone {
        dirty.extend(networks.in_chunk(coord));
    }
    // And a conduit joined to a block's port in a chunk that unloaded
    // (a machine isn't a conduit, so its chunk may have none) opens.
    if map.is_changed() {
        for (_, network) in networks.iter() {
            for port in network.ports() {
                if map.get(ChunkCoord::split(port.pos).0).is_none() {
                    dirty.push(port.pos + port.face.normal());
                }
            }
        }
    }
    // Chunks that loaded: their conduits join, and the faces open into
    // them close.
    let last = CHUNK_SIZE as i32 - 1;
    for coord in loaded
        .0
        .drain(..)
        .filter_map(|entity| coords.get(entity).ok())
    {
        let Some(chunk) = map.get(*coord).and_then(|entity| chunks.get(entity).ok()) else {
            continue;
        };
        let origin = coord.origin();
        for (index, &state) in chunk.voxels().iter().enumerate() {
            if content.conduit(state).is_some() {
                dirty.push(origin + Chunk::position_of(index));
            }
        }
        for side in Direction::ALL {
            let next = ChunkCoord(coord.0 + side.normal());
            for a in 0..=last {
                for b in 0..=last {
                    let pos = next.origin() + face_voxel(side.opposite(), a, b, last);
                    if networks.has(pos) {
                        dirty.push(pos);
                    }
                }
            }
        }
    }
    if dirty.is_empty() {
        return;
    }
    // Most voxels that change aren't conduits, or leave them as they were:
    // only a real change marks the networks changed, for the indexes built
    // from them.
    let revision = networks.revision();
    let changes = networks
        .bypass_change_detection()
        .update(dirty, &content, world);
    if networks.revision() != revision {
        networks.set_changed();
    }
    changed.write_batch(changes);
}

/// The `network` line headless scripts print for the conduit at `pos`.
pub fn describe(networks: &Networks, content: &Content, pos: IVec3) -> String {
    let at = format!("{} {} {}", pos.x, pos.y, pos.z);
    let Some(id) = networks.at(pos) else {
        return format!("network at {at}: none");
    };
    let network = networks.get(id).expect("a conduit's network exists");
    let kind = &content.networks[network.kind()];
    let chunks = network.chunks().count();
    let mut line = format!(
        "network at {at}: #{} {} ({:?}), {} conduit{}, {} chunk{}, capacity {}",
        id.0,
        kind.id,
        kind.solver,
        network.conduits().len(),
        if network.conduits().len() == 1 {
            ""
        } else {
            "s"
        },
        chunks,
        if chunks == 1 { "" } else { "s" },
        network.capacity(),
    );
    if network.is_frozen() {
        line += &format!(
            ", frozen ({} open face{})",
            network.open_faces(),
            if network.open_faces() == 1 { "" } else { "s" }
        );
    } else {
        line += ", running";
    }
    let mut ports: Vec<&Port> = network.ports().iter().collect();
    ports.sort_by_key(|port| (port.pos.to_array(), port.face as u8));
    if ports.is_empty() {
        line += ", no ports";
    } else {
        let names: Vec<String> = ports
            .iter()
            .map(|port| {
                format!(
                    "{} {} {} {}",
                    port.pos.x,
                    port.pos.y,
                    port.pos.z,
                    port.face.name()
                )
            })
            .collect();
        line += &format!(", ports: {}", names.join(", "));
    }
    line
}

#[cfg(test)]
mod tests;
