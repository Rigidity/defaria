//! What solvers keep per network, combined and divided as networks merge
//! and split, so nothing a network holds is made or lost by its shape
//! changing.
//!
//! A solver implements [`NetworkState`] for what it keeps (stored energy,
//! a tank of fluid, items in transit) and registers it with
//! [`NetworkAppExt::add_network_state`]. Its states live in a
//! [`NetworkMap`], kept in step with [`NetworkChanged`] right after the
//! networks update, before anything in `FixedUpdate` reads them. A network
//! the map has nothing for has the default state.

use bevy::{platform::collections::HashMap, prelude::*};

use super::{NetworkChanged, NetworkId, NetworkSystems, Share};

/// What a solver keeps for each network.
pub trait NetworkState: Default + Send + Sync + 'static {
    /// The network `other` was joined this one.
    fn merge(&mut self, other: Self);

    /// The network lost conduits or links, and is now one network per
    /// share in `parts`: return one state for each, in order. `removed` is
    /// the share of the conduits it lost; what isn't handed to a part went
    /// with them (see [`divide_by_share`]).
    fn split(self, parts: &[Share], removed: Share) -> Vec<Self>;
}

/// Each network's `T`, by network.
#[derive(Resource, Debug)]
pub struct NetworkMap<T>(HashMap<NetworkId, T>);

impl<T> Default for NetworkMap<T> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<T: NetworkState> NetworkMap<T> {
    pub fn get(&self, id: NetworkId) -> Option<&T> {
        self.0.get(&id)
    }

    /// The network's state, made if it has none yet.
    pub fn get_mut(&mut self, id: NetworkId) -> &mut T {
        self.0.entry(id).or_default()
    }

    pub fn iter(&self) -> impl Iterator<Item = (NetworkId, &T)> {
        self.0.iter().map(|(&id, state)| (id, state))
    }

    /// Applies one change.
    pub fn apply(&mut self, change: &NetworkChanged) {
        match change {
            NetworkChanged::Merged { into, from } => {
                if let Some(state) = self.0.remove(from) {
                    self.get_mut(*into).merge(state);
                }
            }
            NetworkChanged::Split {
                from,
                parts,
                removed,
            } => {
                let state = self.0.remove(from).unwrap_or_default();
                let shares: Vec<Share> = parts.iter().map(|&(_, share)| share).collect();
                let states = state.split(&shares, *removed);
                debug_assert_eq!(states.len(), parts.len());
                for (&(id, _), state) in parts.iter().zip(states) {
                    self.0.insert(id, state);
                }
            }
        }
    }
}

/// Keeps a [`NetworkMap`] in step with the networks.
fn follow<T: NetworkState>(
    mut changes: MessageReader<NetworkChanged>,
    mut map: ResMut<NetworkMap<T>>,
) {
    for change in changes.read() {
        map.apply(change);
    }
}

pub trait NetworkAppExt {
    /// Keeps a `T` for every network, in a [`NetworkMap<T>`].
    fn add_network_state<T: NetworkState>(&mut self) -> &mut Self;
}

impl NetworkAppExt for App {
    fn add_network_state<T: NetworkState>(&mut self) -> &mut Self {
        self.init_resource::<NetworkMap<T>>()
            .add_systems(Update, follow::<T>.after(NetworkSystems))
    }
}

/// Divides `total` between `parts` and `removed` by their shares, exactly:
/// the parts' amounts, in order, and what goes with the removed conduits,
/// adding up to `total`. Shares count by capacity, or by conduits where
/// none has any.
pub fn divide_by_share(total: u64, parts: &[Share], removed: Share) -> (Vec<u64>, u64) {
    let all = parts.iter().chain([&removed]);
    let by_capacity = all.clone().any(|share| share.capacity > 0.0);
    let weights: Vec<f64> = all
        .map(|share| {
            if by_capacity {
                share.capacity
            } else {
                share.conduits as f64
            }
        })
        .collect();
    let mut amounts = divide(total, &weights);
    let removed = amounts.pop().unwrap_or_default();
    (amounts, removed)
}

/// Divides `total` in proportion to `weights`, in whole units that add up
/// to it exactly: each rounded down, then one more to each of those with
/// the largest remainders. All the weights 0 (or none) puts it all in the
/// first.
pub fn divide(total: u64, weights: &[f64]) -> Vec<u64> {
    let sum: f64 = weights.iter().map(|w| w.max(0.0)).sum();
    if weights.is_empty() {
        return Vec::new();
    }
    if sum <= 0.0 {
        let mut out = vec![0; weights.len()];
        out[0] = total;
        return out;
    }
    let exact: Vec<f64> = weights
        .iter()
        .map(|w| total as f64 * w.max(0.0) / sum)
        .collect();
    let mut out: Vec<u64> = exact.iter().map(|e| e.floor() as u64).collect();
    let given: u64 = out.iter().sum();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| {
        let fraction = |i: usize| exact[i] - exact[i].floor();
        fraction(b).total_cmp(&fraction(a)).then(a.cmp(&b))
    });
    // Floating point can leave the floors a unit or so off either way.
    let mut left = total as i64 - given as i64;
    let mut i = 0;
    while left > 0 {
        out[order[i % order.len()]] += 1;
        left -= 1;
        i += 1;
    }
    while left < 0 {
        let j = order[order.len() - 1 - (i % order.len())];
        if out[j] > 0 {
            out[j] -= 1;
            left += 1;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dividing_keeps_every_unit() {
        assert_eq!(divide(10, &[1.0, 1.0]), [5, 5]);
        assert_eq!(divide(10, &[1.0, 1.0, 1.0]), [4, 3, 3]);
        assert_eq!(divide(7, &[0.0, 0.0]), [7, 0]);
        assert_eq!(divide(7, &[0.0, 2.0]), [0, 7]);
        assert_eq!(divide(0, &[1.0, 3.0]), [0, 0]);
        assert!(divide(5, &[]).is_empty());
        for total in [1, 999, 1000, 123_456_789] {
            for weights in [
                vec![0.5, 0.25, 0.25],
                vec![1.0 / 3.0; 7],
                vec![1e-9, 1.0, 1e9],
            ] {
                let parts = divide(total, &weights);
                assert_eq!(parts.iter().sum::<u64>(), total, "{total} {weights:?}");
            }
        }
    }

    #[test]
    fn shares_count_capacity_or_else_conduits() {
        let share = |conduits, capacity| Share { conduits, capacity };
        let (parts, removed) =
            divide_by_share(1000, &[share(3, 1.5), share(1, 0.5)], share(2, 0.0));
        assert_eq!((parts, removed), (vec![750, 250], 0));
        // Wires hold nothing, so they share by count.
        let (parts, removed) = divide_by_share(100, &[share(3, 0.0)], share(1, 0.0));
        assert_eq!((parts, removed), (vec![75], 25));
    }
}
