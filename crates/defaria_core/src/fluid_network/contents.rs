//! What a fluid network holds: [`FluidNet`], one shared tank, kept per
//! network in a [`NetworkMap`](crate::network::NetworkMap), and
//! [`FluidAmount`], an amount of one fluid.

use bevy::prelude::*;

use crate::network::{NetworkState, Share, divide, divide_by_share};

/// Some amount of one fluid, in [`FULL_FLUID`](crate::content::FULL_FLUID)
/// units (thousandths of a block, as the world counts fluid). The fluid is
/// its block's id, as a machine's [`Tank`](crate::machine::Tank) names it.
/// What anything holding "some of one fluid" (a pipe, a bucket) keeps.
#[derive(Reflect, Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FluidAmount {
    pub fluid: String,
    pub amount: u64,
}

/// What a fluid network holds, as one shared tank: its volume is the sum
/// of its pipes' capacities.
///
/// A network holds one fluid. It holds more than one only after networks
/// holding different fluids were joined; then it takes nothing in, only
/// gives out (the first fluid first) until one is left (see
/// `docs/decisions.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FluidNet {
    /// Each fluid it holds, none of them empty, in the order they came.
    pub held: Vec<FluidAmount>,
}

impl FluidNet {
    /// How much it holds, of every fluid.
    pub fn total(&self) -> u64 {
        self.held.iter().map(|held| held.amount).sum()
    }

    /// How much of `fluid` it holds.
    pub fn amount(&self, fluid: &str) -> u64 {
        self.held
            .iter()
            .find(|held| held.fluid == fluid)
            .map_or(0, |held| held.amount)
    }

    /// The fluid it holds, if it holds just one.
    pub fn fluid(&self) -> Option<&str> {
        match self.held.as_slice() {
            [one] => Some(&one.fluid),
            _ => None,
        }
    }

    /// Whether it holds more than one fluid.
    pub fn is_mixed(&self) -> bool {
        self.held.len() > 1
    }

    /// Whether it takes `fluid` in: it's empty or holds only that.
    pub fn takes(&self, fluid: &str) -> bool {
        match self.held.as_slice() {
            [] => true,
            [one] => one.fluid == fluid,
            _ => false,
        }
    }

    /// Puts `amount` of `fluid` in, whatever it holds. Room and which
    /// fluids it takes are the caller's to check.
    pub fn add(&mut self, fluid: &str, amount: u64) {
        if amount == 0 {
            return;
        }
        match self.held.iter_mut().find(|held| held.fluid == fluid) {
            Some(held) => held.amount += amount,
            None => self.held.push(FluidAmount {
                fluid: fluid.to_string(),
                amount,
            }),
        }
    }

    /// Takes up to `amount` of `fluid` out, and returns how much came out.
    pub fn take(&mut self, fluid: &str, amount: u64) -> u64 {
        let Some(index) = self.held.iter().position(|held| held.fluid == fluid) else {
            return 0;
        };
        let held = &mut self.held[index];
        let taken = amount.min(held.amount);
        held.amount -= taken;
        if held.amount == 0 {
            self.held.remove(index);
        }
        taken
    }

    /// Takes the shares of some of its pipes out of each fluid, by volume:
    /// `weights` are their capacities and `rest` that of the network's
    /// other pipes, which keep theirs. Divided by [`divide`], so every
    /// pipe's share adds up to what it holds exactly.
    pub fn take_shares(&mut self, weights: &[f64], rest: f64) -> Vec<Vec<FluidAmount>> {
        let mut shares = vec![Vec::new(); weights.len()];
        let mut all = weights.to_vec();
        all.push(rest.max(0.0));
        for held in std::mem::take(&mut self.held) {
            let parts = divide(held.amount, &all);
            for (share, &amount) in shares.iter_mut().zip(&parts) {
                if amount > 0 {
                    share.push(FluidAmount {
                        fluid: held.fluid.clone(),
                        amount,
                    });
                }
            }
            let left = parts[weights.len()];
            if left > 0 {
                self.held.push(FluidAmount {
                    fluid: held.fluid,
                    amount: left,
                });
            }
        }
        shares
    }
}

impl NetworkState for FluidNet {
    fn merge(&mut self, other: Self) {
        for held in other.held {
            self.add(&held.fluid, held.amount);
        }
    }

    /// Each part gets its share of each fluid by volume. The conduits it
    /// lost get nothing: what a broken pipe held was spilled, and what an
    /// unloading chunk's pipes held was saved in them, before the split.
    fn split(self, parts: &[Share], _removed: Share) -> Vec<Self> {
        let mut states = vec![Self::default(); parts.len()];
        for held in self.held {
            // Split into nothing, what it held is counted in
            // `FluidNetworkStats::lost` (`pipes::count_ended_networks`).
            let (amounts, _) = divide_by_share(held.amount, parts, Share::default());
            for (state, amount) in states.iter_mut().zip(amounts) {
                state.add(&held.fluid, amount);
            }
        }
        states
    }
}
