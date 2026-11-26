//! Moving items between two places: two inventories, or an inventory and
//! the creative catalog, which gives out and takes in any number of
//! anything. Plain functions, so the rules are tested without a screen.

use super::{Inventory, ItemStack};
use crate::content::Content;

/// One side of a move: an inventory, or the catalog (`None`).
pub type Side<'a> = Option<&'a mut Inventory>;

/// Moves up to `count` of `kind`'s item (with its data) from `from` to
/// `to`: no more than `from` has, nor than `to`'s weight limit lets in.
/// Returns how many moved.
pub fn transfer(content: &Content, from: Side, to: Side, kind: &ItemStack, count: u32) -> u32 {
    let available = from.as_ref().map_or(u32::MAX, |from| from.count(kind.item));
    let room = to
        .as_ref()
        .map_or(u32::MAX, |to| to.room_for(content, kind.item));
    let moved = count.min(available).min(room);
    if moved == 0 {
        return 0;
    }
    if let Some(from) = from {
        from.remove(kind.item, moved);
    }
    if let Some(to) = to {
        let stack = ItemStack {
            count: moved,
            ..kind.clone()
        };
        // Within the room checked above, so it all fits.
        to.add(content, stack);
    }
    moved
}

/// How many a move of a `count`-strong entry takes for "half": rounded up,
/// so one of one still moves.
pub fn half(count: u32) -> u32 {
    count.div_ceil(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{content::base, inventory::model::item_weight};

    fn stone() -> ItemStack {
        ItemStack::one(base().default_item("defaria:stone").unwrap())
    }

    #[test]
    fn moving_between_inventories() {
        let content = base();
        let mut a = Inventory::new();
        let mut b = Inventory::new();
        a.add(
            content,
            ItemStack {
                count: 10,
                ..stone()
            },
        );
        assert_eq!(
            transfer(content, Some(&mut a), Some(&mut b), &stone(), 4),
            4
        );
        assert_eq!((a.count(stone().item), b.count(stone().item)), (6, 4));
        // No more than there is.
        assert_eq!(
            transfer(content, Some(&mut a), Some(&mut b), &stone(), 99),
            6
        );
        assert!(a.is_empty());
        assert_eq!(
            transfer(content, Some(&mut a), Some(&mut b), &stone(), 1),
            0
        );
    }

    #[test]
    fn the_catalog_gives_and_takes_anything() {
        let content = base();
        let mut a = Inventory::new();
        assert_eq!(transfer(content, None, Some(&mut a), &stone(), 100), 100);
        assert_eq!(transfer(content, Some(&mut a), None, &stone(), 30), 30);
        assert_eq!(a.count(stone().item), 70);
    }

    #[test]
    fn the_weight_limit_holds() {
        let content = base();
        let each = item_weight(content, stone().item);
        let mut a = Inventory::new().with_max_weight(each * 5.0);
        assert_eq!(transfer(content, None, Some(&mut a), &stone(), 100), 5);
        let mut b = Inventory::new();
        b.add(
            content,
            ItemStack {
                count: 9,
                ..stone()
            },
        );
        assert_eq!(
            transfer(content, Some(&mut b), Some(&mut a), &stone(), 9),
            0
        );
        assert_eq!(b.count(stone().item), 9);
    }

    #[test]
    fn half_rounds_up() {
        assert_eq!([half(1), half(2), half(7)], [1, 1, 4]);
    }
}
