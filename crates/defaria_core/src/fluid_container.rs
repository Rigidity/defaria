//! Fluid containers: items that hold some of one fluid, starting with the
//! bucket. An item is a container if its file has `fluid_container` (see
//! `docs/modding.md`): its [`capacity`]. What one holds is its
//! [`ItemData::fluid`]. Taking and pouring with one is the server's
//! (`defaria_server::fluid_container`), where the module docs say how.

use crate::{content::Content, content::ItemState, inventory::model::ItemData};

/// How much fluid one `item` holds, if it's a fluid container.
pub fn capacity(content: &Content, item: ItemState) -> Option<u64> {
    content.item_of(item)?.fluid_capacity
}

/// The data of a saved `item` holding `amount` of `fluid`, if it can: it
/// has to be a container with room for it, and `fluid` a fluid. Loading
/// checks it, since mods can change between saving and loading.
pub fn saved_data(
    content: &Content,
    item: ItemState,
    fluid: &str,
    amount: u64,
) -> Result<ItemData, String> {
    let Some(capacity) = capacity(content, item) else {
        return Err("it isn't a fluid container any more".into());
    };
    if !content
        .default_state(fluid)
        .is_some_and(|state| content.state(state).is_fluid())
    {
        return Err(format!("`{fluid}` isn't a fluid any more"));
    }
    if amount > capacity {
        return Err(format!(
            "{amount} of `{fluid}` is more than it holds ({capacity})"
        ));
    }
    Ok(ItemData::holding(fluid, amount))
}
