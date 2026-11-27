//! The networks themselves: which conduit voxels are joined into which
//! network, kept up to date one voxel at a time.
//!
//! A conduit is a node, and its six sides are edges, each one of:
//!
//! - nothing, where the conduit isn't joined (its connection property on
//!   that side is `false`);
//! - a link, where it's joined to a conduit of the same network kind and
//!   group that's joined back;
//! - open, where it's joined to a voxel in a chunk that isn't loaded, so
//!   the network may go on there: the network is frozen;
//! - a port, where it's joined to any other block: a machine's face on
//!   the network (#114).
//!
//! The edges follow the connection properties the `connections` plugin
//! sets, so anything that changes what joins (a wrench blocking a face,
//! #121) changes the networks too, with no rule of its own here.
//!
//! [`Networks::update`] re-evaluates the voxels it's given. A link that
//! forms merges the two networks, the smaller into the larger, so the work
//! is proportional to the smaller. A link that goes, or a conduit that
//! does, may split a network: a search starts from each conduit that lost
//! a link, all in step, one conduit each in turn. Searches that meet are
//! one part. A search that runs out of conduits without meeting another
//! has found a separate part, which moves to a new network; once only one
//! search is left, the rest keep the network's id, unsearched. A loop
//! broken in one place is found whole when its two searches meet, half way
//! round; a line broken in the middle costs a search of both halves.

use std::collections::VecDeque;

use bevy::{
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use defaria_core::{
    content::{BlockState, Content, Direction},
    world::ChunkCoord,
};

/// A network, for as long as it lasts: ids are never reused. A network that
/// merges into another is gone; one that splits keeps its id for one part.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct NetworkId(pub u64);

/// A block's face that a network's conduit is joined to: a machine's port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Port {
    /// The block with the port.
    pub pos: IVec3,
    /// Its face the conduit is on.
    pub face: Direction,
}

/// How much of a network some conduits are, for solvers dividing what
/// a network holds when it splits.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Share {
    pub conduits: u32,
    /// The sum of their capacities.
    pub capacity: f64,
}

impl Share {
    fn add(&mut self, other: Share) {
        self.conduits += other.conduits;
        self.capacity += other.capacity;
    }
}

/// How networks changed, in order, for solvers that keep something per
/// network (see `NetworkState`). A network that's new has nothing yet; one
/// that only grew sends nothing.
#[derive(Message, Debug, Clone, PartialEq)]
pub enum NetworkChanged {
    /// `from` joined `into`, and is gone.
    Merged { into: NetworkId, from: NetworkId },
    /// `from` lost conduits (broken, replaced, or in a chunk that
    /// unloaded) or links, and is now `parts`, each with its share. `from`
    /// is one of them unless none is left. `removed` is the share of the
    /// conduits it lost.
    Split {
        from: NetworkId,
        parts: Vec<(NetworkId, Share)>,
        removed: Share,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Edge {
    #[default]
    None,
    Link,
    Open,
    Port,
}

#[derive(Debug, Clone)]
struct Node {
    network: NetworkId,
    state: BlockState,
    edges: [Edge; 6],
}

/// One network: the conduits joined together, and what they're joined to.
#[derive(Debug, Clone)]
pub struct Network {
    kind: usize,
    conduits: HashSet<IVec3>,
    ports: HashSet<Port>,
    /// Faces joined into chunks that aren't loaded.
    open: u32,
    /// Conduits in each chunk, by chunk coordinate.
    chunks: HashMap<IVec3, u32>,
    capacity: f64,
}

impl Network {
    fn new(kind: usize) -> Self {
        Self {
            kind,
            conduits: default(),
            ports: default(),
            open: 0,
            chunks: default(),
            capacity: 0.0,
        }
    }

    /// The network kind, as an index into `Content::networks`.
    pub fn kind(&self) -> usize {
        self.kind
    }

    pub fn conduits(&self) -> &HashSet<IVec3> {
        &self.conduits
    }

    /// The faces of other blocks the network's conduits are joined to.
    pub fn ports(&self) -> &HashSet<Port> {
        &self.ports
    }

    /// The chunks its conduits are in, by coordinate.
    pub fn chunks(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.chunks.keys().copied()
    }

    /// How many of its faces lead into chunks that aren't loaded.
    pub fn open_faces(&self) -> u32 {
        self.open
    }

    /// Whether it reaches into a chunk that isn't loaded, so it may be
    /// bigger than it looks. Solvers skip a frozen network: it keeps what
    /// it holds until the chunk loads and it's whole again.
    pub fn is_frozen(&self) -> bool {
        self.open > 0
    }

    /// The sum of its conduits' capacities.
    pub fn capacity(&self) -> f64 {
        self.capacity
    }

    pub fn share(&self) -> Share {
        Share {
            conduits: self.conduits.len() as u32,
            capacity: self.capacity,
        }
    }
}

/// Every network of conduits in the loaded world. Built from the chunks as
/// they load and kept up to date as voxels change; nothing of it is saved.
#[derive(Resource, Debug, Default)]
pub struct Networks {
    nodes: HashMap<IVec3, Node>,
    networks: HashMap<NetworkId, Network>,
    /// The conduits in each chunk, by chunk coordinate.
    by_chunk: HashMap<IVec3, HashSet<IVec3>>,
    next: u64,
    /// Counts the changes made to the networks, so an update that changes
    /// nothing can be told apart (see [`Networks::revision`]).
    revision: u64,
}

impl Networks {
    /// The network the conduit at `pos` is part of.
    pub fn at(&self, pos: IVec3) -> Option<NetworkId> {
        self.nodes.get(&pos).map(|node| node.network)
    }

    pub fn get(&self, id: NetworkId) -> Option<&Network> {
        self.networks.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (NetworkId, &Network)> {
        self.networks.iter().map(|(&id, network)| (id, network))
    }

    pub fn len(&self) -> usize {
        self.networks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    /// The chunks with conduits in them, by coordinate.
    pub fn chunks(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.by_chunk.keys().copied()
    }

    /// The conduits in a chunk, by chunk coordinate.
    pub fn in_chunk(&self, chunk: IVec3) -> impl Iterator<Item = IVec3> + '_ {
        self.by_chunk.get(&chunk).into_iter().flatten().copied()
    }

    /// Whether there's a conduit at `pos`.
    pub fn has(&self, pos: IVec3) -> bool {
        self.nodes.contains_key(&pos)
    }

    /// A number that changes whenever the networks do: a conduit joining,
    /// leaving or changing, or an edge changing. Re-evaluating voxels that
    /// turn out the same leaves it as it was.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Brings the networks up to date with the world at the voxels in
    /// `dirty`, and at whatever else that touches. `world` gives the block
    /// at a position, or `None` where the chunk isn't loaded. Returns how
    /// the networks changed, in order.
    pub fn update(
        &mut self,
        dirty: impl IntoIterator<Item = IVec3>,
        content: &Content,
        world: impl Fn(IVec3) -> Option<BlockState>,
    ) -> Vec<NetworkChanged> {
        let mut pass = Pass {
            networks: self,
            content,
            world: &world,
            queue: VecDeque::new(),
            queued: HashSet::new(),
            fresh: HashSet::new(),
            seeds: HashSet::new(),
            removed: HashMap::new(),
            changes: Vec::new(),
        };
        for pos in dirty {
            pass.push(pos);
        }
        while let Some(pos) = pass.queue.pop_front() {
            pass.queued.remove(&pos);
            pass.evaluate(pos);
        }
        pass.split();
        pass.changes
    }
}

/// One [`Networks::update`].
struct Pass<'a, W> {
    networks: &'a mut Networks,
    content: &'a Content,
    world: &'a W,
    queue: VecDeque<IVec3>,
    queued: HashSet<IVec3>,
    /// Networks made during this pass: no solver has anything for them
    /// yet, so their changes needn't be sent.
    fresh: HashSet<NetworkId>,
    /// Conduits that lost a link, where searches for splits start.
    seeds: HashSet<IVec3>,
    /// The share of the conduits each network lost.
    removed: HashMap<NetworkId, Share>,
    changes: Vec<NetworkChanged>,
}

impl<W: Fn(IVec3) -> Option<BlockState>> Pass<'_, W> {
    fn push(&mut self, pos: IVec3) {
        if self.queued.insert(pos) {
            self.queue.push_back(pos);
        }
    }

    /// The conduit at `pos` in the world, if there's one.
    fn conduit_at(&self, pos: IVec3) -> Option<BlockState> {
        (self.world)(pos).filter(|&state| self.content.conduit(state).is_some())
    }

    /// Whether two states are conduits that join each other: the same
    /// network kind and group, whatever their numbers or the sides they
    /// join. A conduit that changes into one it joins stays on its network,
    /// as the same node.
    fn same(&self, a: BlockState, b: BlockState) -> bool {
        match (self.content.conduit(a), self.content.conduit(b)) {
            (Some(a), Some(b)) => a.joins(b),
            _ => false,
        }
    }

    fn evaluate(&mut self, pos: IVec3) {
        let want = self.conduit_at(pos);
        let have = self.networks.nodes.get(&pos).map(|node| node.state);
        match (have, want) {
            (None, None) => {}
            (Some(have), Some(want)) if self.same(have, want) => {
                if self.content.conduit(have) != self.content.conduit(want) {
                    // Another metal, say: the network keeps what it holds,
                    // with the new capacity and throughput.
                    let more = self.capacity(want) - self.capacity(have);
                    let node = &self.networks.nodes[&pos];
                    let network = self.networks.networks.get_mut(&node.network).unwrap();
                    network.capacity += more;
                }
                // Anything about it may have changed that a solver reads
                // (a stat of its metal), even with the same capacity and
                // throughput.
                if have != want {
                    self.networks.revision += 1;
                }
                self.networks.nodes.get_mut(&pos).unwrap().state = want;
                self.refresh_edges(pos);
            }
            (Some(_), want) => {
                self.remove_node(pos);
                if let Some(want) = want {
                    self.add_node(pos, want);
                }
            }
            (None, Some(want)) => self.add_node(pos, want),
        }
    }

    fn new_network(&mut self, kind: usize) -> NetworkId {
        let id = NetworkId(self.networks.next);
        self.networks.next += 1;
        self.networks.networks.insert(id, Network::new(kind));
        self.fresh.insert(id);
        id
    }

    fn capacity(&self, state: BlockState) -> f64 {
        self.content
            .conduit(state)
            .map_or(0.0, |conduit| conduit.capacity as f64)
    }

    fn add_node(&mut self, pos: IVec3, state: BlockState) {
        self.networks.revision += 1;
        let kind = self.content.conduit(state).unwrap().network;
        let id = self.new_network(kind);
        let capacity = self.capacity(state);
        let network = self.networks.networks.get_mut(&id).unwrap();
        network.conduits.insert(pos);
        network.capacity += capacity;
        let (chunk, _) = ChunkCoord::split(pos);
        *network.chunks.entry(chunk.0).or_default() += 1;
        self.networks
            .by_chunk
            .entry(chunk.0)
            .or_default()
            .insert(pos);
        self.networks.nodes.insert(
            pos,
            Node {
                network: id,
                state,
                edges: [Edge::None; 6],
            },
        );
        self.refresh_edges(pos);
    }

    fn remove_node(&mut self, pos: IVec3) {
        self.networks.revision += 1;
        let node = self.networks.nodes.remove(&pos).unwrap();
        let capacity = self.capacity(node.state);
        let network = self.networks.networks.get_mut(&node.network).unwrap();
        for side in Direction::ALL {
            let next = pos + side.normal();
            match node.edges[side as usize] {
                Edge::None => {}
                Edge::Open => network.open -= 1,
                Edge::Port => {
                    network.ports.remove(&Port {
                        pos: next,
                        face: side.opposite(),
                    });
                }
                Edge::Link => {
                    if let Some(neighbour) = self.networks.nodes.get_mut(&next) {
                        neighbour.edges[side.opposite() as usize] = Edge::None;
                    }
                    self.seeds.insert(next);
                    if self.queued.insert(next) {
                        self.queue.push_back(next);
                    }
                }
            }
        }
        network.conduits.remove(&pos);
        network.capacity -= capacity;
        let (chunk, _) = ChunkCoord::split(pos);
        if let Some(count) = network.chunks.get_mut(&chunk.0) {
            *count -= 1;
            if *count == 0 {
                network.chunks.remove(&chunk.0);
            }
        }
        if let Some(set) = self.networks.by_chunk.get_mut(&chunk.0) {
            set.remove(&pos);
            if set.is_empty() {
                self.networks.by_chunk.remove(&chunk.0);
            }
        }
        self.removed.entry(node.network).or_default().add(Share {
            conduits: 1,
            capacity,
        });
    }

    /// What the conduit at `pos`, in `state`, has on its `side`.
    fn edge(&self, pos: IVec3, state: BlockState, side: Direction) -> Edge {
        if !self.content.joined(state, side) {
            return Edge::None;
        }
        let Some(next) = (self.world)(pos + side.normal()) else {
            return Edge::Open;
        };
        let own = self.content.conduit(state).unwrap();
        let linked = self
            .content
            .conduit(next)
            .is_some_and(|conduit| conduit.joins(own))
            && self.content.joined(next, side.opposite());
        if linked { Edge::Link } else { Edge::Port }
    }

    fn refresh_edges(&mut self, pos: IVec3) {
        let state = self.networks.nodes[&pos].state;
        for side in Direction::ALL {
            let edge = self.edge(pos, state, side);
            self.set_edge(pos, side, edge);
        }
    }

    fn set_edge(&mut self, pos: IVec3, side: Direction, edge: Edge) {
        let node = self.networks.nodes.get_mut(&pos).unwrap();
        let old = std::mem::replace(&mut node.edges[side as usize], edge);
        if old == edge {
            return;
        }
        self.networks.revision += 1;
        let id = node.network;
        let next = pos + side.normal();
        let port = Port {
            pos: next,
            face: side.opposite(),
        };
        let network = self.networks.networks.get_mut(&id).unwrap();
        match old {
            Edge::Open => network.open -= 1,
            Edge::Port => {
                network.ports.remove(&port);
            }
            Edge::Link | Edge::None => {}
        }
        match edge {
            Edge::Open => network.open += 1,
            Edge::Port => {
                network.ports.insert(port);
            }
            Edge::Link | Edge::None => {}
        }
        if old == Edge::Link {
            // The link went, while both ends stayed: the network may have
            // split there.
            self.seeds.insert(pos);
            self.seeds.insert(next);
            self.push(next);
        }
        if edge == Edge::Link {
            // Join the neighbour's network, unless the neighbour is about
            // to change: it joins when it's evaluated.
            let current = self.networks.nodes.get(&next).and_then(|neighbour| {
                let world = (self.world)(next)?;
                self.same(neighbour.state, world)
                    .then_some(neighbour.network)
            });
            if let Some(other) = current {
                self.merge(id, other);
            }
            self.push(next);
        }
    }

    /// Merges two networks, the smaller into the larger.
    fn merge(&mut self, a: NetworkId, b: NetworkId) {
        if a == b {
            return;
        }
        // The larger keeps its id, or on a tie the older.
        let rank = |id: NetworkId| {
            (
                self.networks.networks[&id].conduits.len(),
                std::cmp::Reverse(id),
            )
        };
        let (into, from) = if rank(a) >= rank(b) { (a, b) } else { (b, a) };
        let gone = self.networks.networks.remove(&from).unwrap();
        for pos in &gone.conduits {
            self.networks.nodes.get_mut(pos).unwrap().network = into;
        }
        let network = self.networks.networks.get_mut(&into).unwrap();
        network.conduits.extend(gone.conduits);
        network.ports.extend(gone.ports);
        network.open += gone.open;
        network.capacity += gone.capacity;
        for (chunk, count) in gone.chunks {
            *network.chunks.entry(chunk).or_default() += count;
        }
        if let Some(removed) = self.removed.remove(&from) {
            self.removed.entry(into).or_default().add(removed);
        }
        if !self.fresh.contains(&from) {
            // `into` now holds what `from` had.
            self.fresh.remove(&into);
            self.changes.push(NetworkChanged::Merged { into, from });
        }
    }

    /// Finds the networks that came apart, gives each separate part a
    /// network of its own, and reports every network that lost something.
    fn split(&mut self) {
        let mut seeds: HashMap<NetworkId, Vec<IVec3>> = HashMap::new();
        for pos in std::mem::take(&mut self.seeds) {
            if let Some(node) = self.networks.nodes.get(&pos) {
                seeds.entry(node.network).or_default().push(pos);
            }
        }
        let mut touched: Vec<NetworkId> = seeds
            .keys()
            .chain(self.removed.keys())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        touched.sort();
        for id in touched {
            let mut parts = Vec::new();
            if let Some(mut seeds) = seeds.remove(&id) {
                seeds.sort_by_key(|pos| pos.to_array());
                for part in self.separate_parts(id, &seeds) {
                    parts.push(self.move_out(id, part));
                }
            }
            let removed = self.removed.remove(&id).unwrap_or_default();
            let network = &self.networks.networks[&id];
            let emptied = network.conduits.is_empty();
            if !emptied {
                parts.insert(0, id);
            }
            if emptied {
                self.networks.networks.remove(&id);
            }
            if self.fresh.contains(&id) || (parts.len() == 1 && removed == Share::default()) {
                continue;
            }
            let parts = parts
                .into_iter()
                .map(|part| (part, self.networks.networks[&part].share()))
                .collect();
            self.changes.push(NetworkChanged::Split {
                from: id,
                parts,
                removed,
            });
        }
    }

    /// Moves `conduits` out of network `from` into a new one.
    fn move_out(&mut self, from: NetworkId, conduits: Vec<IVec3>) -> NetworkId {
        let kind = self.networks.networks[&from].kind;
        let id = self.new_network(kind);
        if !self.fresh.contains(&from) {
            // The split message tells the solvers about it.
            self.fresh.remove(&id);
        }
        let mut part = Network::new(kind);
        let old = self.networks.networks.get_mut(&from).unwrap();
        for pos in conduits {
            let node = self.networks.nodes.get_mut(&pos).unwrap();
            node.network = id;
            let capacity = self
                .content
                .conduit(node.state)
                .map_or(0.0, |conduit| conduit.capacity as f64);
            old.conduits.remove(&pos);
            old.capacity -= capacity;
            part.capacity += capacity;
            let (chunk, _) = ChunkCoord::split(pos);
            if let Some(count) = old.chunks.get_mut(&chunk.0) {
                *count -= 1;
                if *count == 0 {
                    old.chunks.remove(&chunk.0);
                }
            }
            *part.chunks.entry(chunk.0).or_default() += 1;
            for side in Direction::ALL {
                match node.edges[side as usize] {
                    Edge::Open => {
                        old.open -= 1;
                        part.open += 1;
                    }
                    Edge::Port => {
                        let port = Port {
                            pos: pos + side.normal(),
                            face: side.opposite(),
                        };
                        old.ports.remove(&port);
                        part.ports.insert(port);
                    }
                    Edge::None | Edge::Link => {}
                }
            }
            part.conduits.insert(pos);
        }
        self.networks.networks.insert(id, part);
        id
    }

    /// The parts of network `id` that none of the searches from the other
    /// `seeds` reach, each whole, leaving out the one part found last
    /// (unsearched), which keeps the network.
    fn separate_parts(&self, id: NetworkId, seeds: &[IVec3]) -> Vec<Vec<IVec3>> {
        let mut search = Search::default();
        for &seed in seeds {
            search.start(seed);
        }
        let mut found = Vec::new();
        while search.active() > 1 {
            for root in search.roots() {
                if search.active() <= 1 {
                    break;
                }
                // It may have merged into another this round.
                if search.find(root) != root {
                    continue;
                }
                let Some(pos) = search.frontier[root].pop_front() else {
                    search.finish(root);
                    found.push(std::mem::take(&mut search.members[root]));
                    continue;
                };
                let node = &self.networks.nodes[&pos];
                for side in Direction::ALL {
                    if node.edges[side as usize] != Edge::Link {
                        continue;
                    }
                    let next = pos + side.normal();
                    let on = self
                        .networks
                        .nodes
                        .get(&next)
                        .is_some_and(|neighbour| neighbour.network == id);
                    if on {
                        let root = search.find(root);
                        search.reach(root, next);
                    }
                }
            }
        }
        found
    }
}

/// Searches from several conduits at once, one step each in turn, joined
/// into one when they meet.
#[derive(Default)]
struct Search {
    /// The search that reached each conduit first.
    owner: HashMap<IVec3, usize>,
    /// Union-find over searches.
    parent: Vec<usize>,
    frontier: Vec<VecDeque<IVec3>>,
    members: Vec<Vec<IVec3>>,
    finished: Vec<bool>,
    active: usize,
}

impl Search {
    fn start(&mut self, seed: IVec3) {
        let index = self.parent.len();
        self.parent.push(index);
        self.frontier.push(VecDeque::new());
        self.members.push(Vec::new());
        self.finished.push(false);
        self.active += 1;
        self.reach(index, seed);
    }

    fn active(&self) -> usize {
        self.active
    }

    fn find(&self, mut search: usize) -> usize {
        while self.parent[search] != search {
            search = self.parent[search];
        }
        search
    }

    /// The searches still going.
    fn roots(&self) -> Vec<usize> {
        (0..self.parent.len())
            .filter(|&i| self.parent[i] == i && !self.finished[i])
            .collect()
    }

    fn finish(&mut self, root: usize) {
        self.finished[root] = true;
        self.active -= 1;
    }

    /// Search `root` reaches `pos`: it's its, or the two searches meet.
    fn reach(&mut self, root: usize, pos: IVec3) {
        match self.owner.get(&pos) {
            None => {
                self.owner.insert(pos, root);
                self.frontier[root].push_back(pos);
                self.members[root].push(pos);
            }
            Some(&other) => {
                let other = self.find(other);
                if other != root {
                    self.join(root, other);
                }
            }
        }
    }

    /// Joins two searches, the smaller into the larger.
    fn join(&mut self, a: usize, b: usize) {
        let (into, from) = if self.members[a].len() >= self.members[b].len() {
            (a, b)
        } else {
            (b, a)
        };
        self.parent[from] = into;
        let frontier = std::mem::take(&mut self.frontier[from]);
        self.frontier[into].extend(frontier);
        let members = std::mem::take(&mut self.members[from]);
        self.members[into].extend(members);
        self.active -= 1;
    }
}
