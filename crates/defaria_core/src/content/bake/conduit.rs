//! Network kinds, and the conduits that make networks of them.

use super::{Def, fill::fill, mix::Mix};
use crate::content::{
    Conduit, Content, NetworkKind, Property, qualify,
    schema::{Attribute, ConduitDef, NetworkDef},
};

/// Adds the network kinds, in load order.
pub(super) fn add_networks(defs: &[Def<NetworkDef>], content: &mut Content) {
    for def in defs {
        content
            .network_index
            .insert(def.id.clone(), content.networks.len());
        content.networks.push(NetworkKind {
            id: def.id.clone(),
            name: def.def.name.clone(),
            solver: def.def.solver,
        });
    }
}

/// A block's `conduit`, checked against its properties, to work out per
/// state.
pub(super) struct ConduitMix {
    network: usize,
    group: Option<String>,
    capacity: Mix,
    throughput: Mix,
}

impl ConduitMix {
    pub(super) fn new(
        def: &ConduitDef,
        namespace: &str,
        properties: &[Property],
        content: &Content,
    ) -> Result<Self, String> {
        let id = qualify(&def.network, namespace).map_err(|error| format!("conduit: {error}"))?;
        let network = content
            .network(&id)
            .ok_or_else(|| format!("conduit: no network kind `{id}`"))?;
        let mix = |field: &str, def: &Attribute| {
            Mix::new(
                &format!("conduit.{field}"),
                field,
                def,
                properties,
                None,
                content,
            )
        };
        Ok(ConduitMix {
            network,
            group: def.group.clone(),
            capacity: mix("capacity", &def.capacity)?,
            throughput: mix("throughput", &def.throughput)?,
        })
    }

    /// The conduit of the state with these values.
    pub(super) fn value(
        &self,
        properties: &[Property],
        index: &[usize],
        values: &[(String, String)],
        content: &Content,
    ) -> Result<Conduit, String> {
        let group = self
            .group
            .as_deref()
            .map(|group| fill(group, values))
            .transpose()
            .map_err(|error| format!("conduit group: {error}"))?;
        if group.as_deref() == Some("") {
            return Err("conduit group is empty".into());
        }
        Ok(Conduit {
            network: self.network,
            group,
            capacity: self.capacity.value(properties, index, content),
            throughput: self.throughput.value(properties, index, content),
        })
    }
}
