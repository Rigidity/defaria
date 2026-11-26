//! Moving items between an inventory (the player's) and a machine's
//! [`Slots`]. Plain functions, so the rules are tested without a screen:
//! nothing is made or lost, the machine's slots hold no more than they
//! hold, and the inventory's weight limit holds.

use super::{SlotGroup, Slots};
use crate::{
    content::{Content, Machine},
    inventory::{Inventory, ItemStack},
};

/// The slot groups a player's items go into: a recipe machine's input
/// groups, or every group of any other machine. Indices into [`Slots`].
pub fn input_groups(machine: &Machine) -> Vec<usize> {
    match &machine.recipes {
        Some(recipes) => recipes.input_slots.clone(),
        None => (0..machine.slots.len()).collect(),
    }
}

/// The slot groups a player's items go into, in order, when they're put
/// on group `first`: that one, then the rest of the [`input_groups`]. Any
/// group but a recipe machine's outputs can be filled this way, so a
/// group the recipes don't name (one a script reads) can be filled by
/// hand.
pub fn input_groups_from(machine: &Machine, first: Option<usize>) -> Vec<usize> {
    let mut groups = input_groups(machine);
    let output = |group: usize| {
        machine.recipes.as_ref().is_some_and(|recipes| {
            recipes.output_slots.contains(&group) && !recipes.input_slots.contains(&group)
        })
    };
    if let Some(first) = first.filter(|&group| group < machine.slots.len() && !output(group)) {
        groups.retain(|&group| group != first);
        groups.insert(0, first);
    }
    groups
}

/// Moves up to `count` of `kind`'s item with its data from `inventory`
/// into `machine`'s [`input_groups`] in `slots`, in order, onto slots of
/// the same kind first within each. No more than the inventory has nor
/// than the groups have room for, and only into groups that take it
/// ([`Content::slot_group_takes`]: a recipe machine's take only what one
/// of its recipes takes). Returns how many moved.
pub fn into_machine(
    content: &Content,
    inventory: &mut Inventory,
    slots: &mut Slots,
    machine: &Machine,
    kind: &ItemStack,
    count: u32,
) -> u32 {
    into_machine_from(content, inventory, slots, machine, kind, count, None)
}

/// [`into_machine`], into group `first` first ([`input_groups_from`]).
pub fn into_machine_from(
    content: &Content,
    inventory: &mut Inventory,
    slots: &mut Slots,
    machine: &Machine,
    kind: &ItemStack,
    count: u32,
    first: Option<usize>,
) -> u32 {
    let mut left = count.min(inventory.count_kind(kind));
    let mut moved = 0;
    for index in input_groups_from(machine, first) {
        if !content.slot_group_takes(machine, index, kind.item) {
            continue;
        }
        let Some(group) = slots.0.get_mut(index) else {
            continue;
        };
        let room = left.min(group.room_for(content, kind));
        if room == 0 {
            continue;
        }
        let stack = ItemStack {
            count: room,
            ..kind.clone()
        };
        let went = room - group.insert(content, stack).map_or(0, |back| back.count);
        moved += went;
        left -= went;
    }
    // Out of the inventory only what went in.
    inventory.remove_kind(kind, moved);
    moved
}

/// Moves up to `count` out of slot `index` of `group` in `slots` into
/// `inventory`: no more than the slot holds, nor than the inventory's
/// weight limit lets in. Returns how many moved.
pub fn out_of_machine(
    content: &Content,
    inventory: &mut Inventory,
    slots: &mut Slots,
    group: usize,
    index: usize,
    count: u32,
) -> u32 {
    let Some(group) = slots.0.get_mut(group) else {
        return 0;
    };
    let Some(Some(there)) = group.slots.get(index) else {
        return 0;
    };
    let room = inventory.room_for(content, there.item);
    let Some(taken) = group.take(index, count.min(room)) else {
        return 0;
    };
    let wanted = taken.count;
    // What the inventory still refuses (a count at its largest) goes back.
    let refused = inventory.add(content, taken);
    let back = refused.as_ref().map_or(0, |stack| stack.count);
    if let Some(stack) = refused {
        put_back(group, index, stack);
    }
    wanted - back
}

/// Puts `stack`, just taken out of slot `index`, back in it.
fn put_back(group: &mut SlotGroup, index: usize, stack: ItemStack) {
    match &mut group.slots[index] {
        Some(there) => there.count += stack.count,
        empty => *empty = Some(stack),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        block_entity::BlockEntityPart,
        content::{base, block},
        inventory::model::item_weight,
        machine::SLOT_SIZE,
        testing::test_content,
    };

    fn item(id: &str) -> ItemStack {
        ItemStack::one(base().default_item(id).unwrap())
    }

    fn ore() -> ItemStack {
        let stone = base().item("defaria:stone").unwrap();
        ItemStack::one(stone.state(&[("ore", "defaria:iron")]).unwrap())
    }

    /// An electric furnace's slots: `input` and `output`, one slot each.
    fn furnace() -> (&'static Machine, Slots) {
        let state = block("electric_furnace");
        let machine = base().machine(state).unwrap();
        let slots = Slots::new(state, base()).unwrap();
        (machine, slots)
    }

    fn with(stack: &ItemStack, count: u32) -> Inventory {
        let mut inventory = Inventory::new();
        inventory.add(
            base(),
            ItemStack {
                count,
                ..stack.clone()
            },
        );
        inventory
    }

    /// Everything of `kind` in the inventory and the slots.
    fn total(inventory: &Inventory, slots: &Slots, kind: &ItemStack) -> u64 {
        let in_slots: u64 = slots
            .0
            .iter()
            .flat_map(|group| group.slots.iter().flatten())
            .filter(|stack| stack.same_kind(kind))
            .map(|stack| stack.count as u64)
            .sum();
        inventory.count_kind(kind) as u64 + in_slots
    }

    #[test]
    fn a_furnace_takes_items_into_its_input_only() {
        let (machine, mut slots) = furnace();
        let groups = input_groups(machine);
        assert_eq!(groups, [machine.slot_group("input").unwrap()]);
        let generator = base().machine(block("combustion_generator")).unwrap();
        assert_eq!(
            input_groups(generator),
            [generator.slot_group("fuel").unwrap()]
        );

        let ore = ore();
        let mut inventory = with(&ore, 100);
        assert_eq!(
            into_machine(base(), &mut inventory, &mut slots, machine, &ore, 1),
            1
        );
        // No more than the slot holds; the rest stays with the player.
        assert_eq!(
            into_machine(base(), &mut inventory, &mut slots, machine, &ore, 500),
            SLOT_SIZE - 1
        );
        assert_eq!(inventory.count_kind(&ore), 100 - SLOT_SIZE);
        assert_eq!(total(&inventory, &slots, &ore), 100);
        let output = machine.slot_group("output").unwrap();
        assert!(slots.0[output].slots[0].is_none());

        // A full slot, or another kind in it, takes nothing.
        let sand = ItemStack::one(
            base()
                .item("defaria:soil")
                .unwrap()
                .state(&[("soil", "defaria:sand")])
                .unwrap(),
        );
        let mut sand_inventory = with(&sand, 5);
        assert_eq!(
            into_machine(base(), &mut sand_inventory, &mut slots, machine, &sand, 5),
            0
        );
        assert_eq!(sand_inventory.count_kind(&sand), 5);
        // Not more than the player has.
        let (_, mut empty) = furnace();
        assert_eq!(
            into_machine(base(), &mut sand_inventory, &mut empty, machine, &sand, 9),
            5
        );
        assert!(sand_inventory.is_empty());

        // Nor does it take what no smelting recipe takes; another machine
        // takes anything.
        let glass = item("defaria:glass");
        let mut glass_inventory = with(&glass, 5);
        let (_, mut empty) = furnace();
        assert_eq!(
            into_machine(base(), &mut glass_inventory, &mut empty, machine, &glass, 5),
            0
        );
        assert_eq!(glass_inventory.count_kind(&glass), 5);
        let state = block("crate");
        let crate_machine = base().machine(state).unwrap();
        let mut crate_slots = Slots::new(state, base()).unwrap();
        assert_eq!(
            into_machine(
                base(),
                &mut glass_inventory,
                &mut crate_slots,
                crate_machine,
                &glass,
                5
            ),
            5
        );
    }

    #[test]
    fn taking_out_moves_what_the_slot_holds() {
        let (machine, mut slots) = furnace();
        let output = machine.slot_group("output").unwrap();
        let ore = ore();
        slots.0[output].insert(
            base(),
            ItemStack {
                count: 10,
                ..ore.clone()
            },
        );
        let mut inventory = Inventory::new();
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, output, 0, 3),
            3
        );
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, output, 0, 99),
            7
        );
        assert_eq!(inventory.count_kind(&ore), 10);
        assert!(slots.0[output].slots[0].is_none());
        // An empty slot, or one that isn't there, gives nothing.
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, output, 0, 1),
            0
        );
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, output, 5, 1),
            0
        );
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, 9, 0, 1),
            0
        );
    }

    /// Put on a group of their choice, items go there first, then into
    /// the input groups as usual: so a recipe machine's group that no
    /// recipe names (a script's) can be filled by hand. Its outputs
    /// can't be chosen.
    #[test]
    fn items_put_on_a_group_go_there_first() {
        let content = test_content(&[
            (
                "blocks/bench.json5",
                r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, machine: { slots: { work: 1, catalyst: 1, done: 1 }, recipes: { type: "polish", inputs: ["work"], outputs: ["done"] } } }"#,
            ),
            (
                "recipes/polish.json5",
                r#"{ type: "polish", inputs: [{ item: "defaria:planks" }], outputs: [{ item: "defaria:glass" }], time: 1 }"#,
            ),
        ]);
        let state = content.default_state("test:bench").unwrap();
        let machine = content.machine(state).unwrap();
        let group = |name| machine.slot_group(name).unwrap();
        let (work, catalyst, done) = (group("work"), group("catalyst"), group("done"));
        assert_eq!(input_groups_from(machine, None), [work]);
        assert_eq!(input_groups_from(machine, Some(catalyst)), [catalyst, work]);
        assert_eq!(input_groups_from(machine, Some(done)), [work]);
        assert_eq!(input_groups_from(machine, Some(99)), [work]);

        let one = |id| ItemStack::one(content.default_item(id).unwrap());
        let (glass, planks) = (one("defaria:glass"), one("defaria:planks"));
        let mut slots = Slots::new(state, &content).unwrap();
        let mut inventory = Inventory::new();
        inventory.add(&content, ItemStack::new(glass.item, 5));
        inventory.add(&content, ItemStack::new(planks.item, 100));
        let mut put = |kind: &ItemStack, count, first| {
            into_machine_from(
                &content,
                &mut inventory,
                &mut slots,
                machine,
                kind,
                count,
                first,
            )
        };
        // Glass no recipe takes: only where it's put.
        assert_eq!(put(&glass, 5, None), 0);
        assert_eq!(put(&glass, 5, Some(done)), 0);
        assert_eq!(put(&glass, 3, Some(catalyst)), 3);
        // Planks put on the outputs go to the work slot; on the catalyst's
        // (taken by glass), past it to the work slot too.
        assert_eq!(put(&planks, 10, Some(done)), 10);
        assert_eq!(put(&planks, 10, Some(catalyst)), 10);
        let held = |group: usize| slots.0[group].slots[0].clone().map(|stack| stack.count);
        assert_eq!(
            (held(work), held(catalyst), held(done)),
            (Some(20), Some(3), None)
        );
    }

    /// A player near their weight limit takes only what they can carry,
    /// and the rest stays in the machine.
    #[test]
    fn the_players_weight_limit_holds() {
        let (machine, mut slots) = furnace();
        let output = machine.slot_group("output").unwrap();
        let stone = item("defaria:stone");
        let each = item_weight(base(), stone.item);
        slots.0[output].insert(
            base(),
            ItemStack {
                count: 20,
                ..stone.clone()
            },
        );
        let mut inventory = Inventory::new().with_max_weight(each * 5.0);
        inventory.add(
            base(),
            ItemStack {
                count: 2,
                ..stone.clone()
            },
        );
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, output, 0, 20),
            3
        );
        assert_eq!(inventory.count_kind(&stone), 5);
        assert_eq!(slots.0[output].slots[0].as_ref().unwrap().count, 17);
        assert_eq!(total(&inventory, &slots, &stone), 22);
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, output, 0, 1),
            0
        );
        assert_eq!(slots.0[output].slots[0].as_ref().unwrap().count, 17);
    }

    /// A count the inventory can't take any higher stays in the slot.
    #[test]
    fn nothing_is_lost_at_the_largest_count() {
        let (machine, mut slots) = furnace();
        let output = machine.slot_group("output").unwrap();
        let stone = item("defaria:stone");
        slots.0[output].insert(
            base(),
            ItemStack {
                count: 10,
                ..stone.clone()
            },
        );
        let mut inventory = with(&stone, u32::MAX - 4);
        assert_eq!(
            out_of_machine(base(), &mut inventory, &mut slots, output, 0, 10),
            4
        );
        assert_eq!(inventory.count_kind(&stone), u32::MAX);
        assert_eq!(slots.0[output].slots[0].as_ref().unwrap().count, 6);
    }
}
