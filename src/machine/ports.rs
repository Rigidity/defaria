//! Which machine ports each network is joined to, and what they do: the
//! index the energy, fluid and item solvers iterate.
//!
//! A network keeps the machine faces its conduits are joined to
//! ([`Network::ports`](crate::network::Network::ports)). This adds what
//! each one is: its role, its tank or slot group, and the machine's block
//! entity, in a list per network, sorted by position and face so solvers
//! run in the same order every time.

use bevy::{platform::collections::HashMap, prelude::*};

use super::Ports;
use crate::{
    block_entity::{BlockEntities, BlockEntity},
    content::{BlockState, Content, Direction, PortRole},
    network::{NetworkId, Networks, Port},
    terrain::{ChunkCoord, ChunkMap},
    voxel::Chunk,
};

/// A machine port on a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachinePortRef {
    /// The machine's voxel.
    pub pos: IVec3,
    /// Its face the network's conduit is joined to.
    pub face: Direction,
    pub role: PortRole,
    /// For a fluid port, its tank, an index into the machine's
    /// [`Tanks`](super::Tanks).
    pub tank: Option<usize>,
    /// For an item port, its slot group, an index into the machine's
    /// [`Slots`](super::Slots).
    pub slots: Option<usize>,
    /// The machine's block entity, with its buffer, tanks and slots.
    /// `None` only for the frame it's placed on, before it's made.
    pub entity: Option<Entity>,
}

/// The machine ports on each network, with their roles.
#[derive(Resource, Debug, Default)]
pub struct MachinePorts {
    by_network: HashMap<NetworkId, Vec<MachinePortRef>>,
}

impl MachinePorts {
    /// The machine ports on network `id`, sorted by position and face.
    pub fn on(&self, id: NetworkId) -> &[MachinePortRef] {
        self.by_network.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Every network with machine ports, and its ports.
    pub fn iter(&self) -> impl Iterator<Item = (NetworkId, &[MachinePortRef])> {
        self.by_network
            .iter()
            .map(|(&id, ports)| (id, ports.as_slice()))
    }

    /// The role of the port `port` on network `id`, if it's a machine's.
    pub fn role(&self, id: NetworkId, port: Port) -> Option<PortRole> {
        self.on(id)
            .iter()
            .find(|p| p.pos == port.pos && p.face == port.face)
            .map(|p| p.role)
    }

    /// Builds the index from the networks, the world and the block
    /// entities.
    pub fn build(
        networks: &Networks,
        content: &Content,
        world: impl Fn(IVec3) -> Option<BlockState>,
        entity: impl Fn(IVec3) -> Option<Entity>,
    ) -> Self {
        let mut by_network = HashMap::new();
        for (id, network) in networks.iter() {
            let mut ports: Vec<MachinePortRef> = network
                .ports()
                .iter()
                .filter_map(|port| {
                    let machine = content.machine(world(port.pos)?)?;
                    let found = machine.port(network.kind(), port.face)?;
                    Some(MachinePortRef {
                        pos: port.pos,
                        face: port.face,
                        role: found.role,
                        tank: found.tank,
                        slots: found.slots,
                        entity: entity(port.pos),
                    })
                })
                .collect();
            if ports.is_empty() {
                continue;
            }
            ports.sort_by_key(|port| (port.pos.to_array(), port.face as u8));
            by_network.insert(id, ports);
        }
        Self { by_network }
    }
}

/// Rebuilds [`MachinePorts`] when the networks or the block entities
/// change, and writes each machine's networks into its [`Ports`].
pub(super) fn index_ports(
    networks: Res<Networks>,
    entities: Res<BlockEntities>,
    content: Res<Content>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
    mut index: ResMut<MachinePorts>,
    mut machines: Query<(&BlockEntity, &mut Ports)>,
) {
    let added = machines.iter_mut().any(|(_, ports)| ports.is_added());
    if !networks.is_changed() && !entities.is_changed() && !added {
        return;
    }
    let world = |pos: IVec3| -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        Some(chunks.get(map.get(coord)?).ok()?.at(local))
    };
    *index = MachinePorts::build(&networks, &content, world, |pos| entities.get(pos));
    for (entity, mut ports) in &mut machines {
        let networks_now: Vec<Option<NetworkId>> = ports
            .0
            .iter()
            .map(|link| {
                let next = entity.pos + link.face.normal();
                let id = networks.at(next)?;
                let network = networks.get(id)?;
                let port = Port {
                    pos: entity.pos,
                    face: link.face,
                };
                (content.networks[network.kind()].id == link.kind
                    && network.ports().contains(&port))
                .then_some(id)
            })
            .collect();
        let same = ports
            .0
            .iter()
            .zip(&networks_now)
            .all(|(link, now)| link.network == *now);
        if !same {
            for (link, now) in ports.0.iter_mut().zip(networks_now) {
                link.network = now;
            }
        }
    }
}
