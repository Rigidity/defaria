//! Calling the scripts of blocks and items as things happen to them:
//!
//! - `on_used(block, player)`: a player used the block (right click on
//!   it). Using a block that defines it doesn't place a block; sneaking
//!   with an item in hand places instead.
//! - `on_placed(block)`: the block was put in its voxel (by anyone: the
//!   player, a script, a scenario's `fill`).
//! - `on_broken(block, old)`: it was replaced by another block; `block`
//!   is the voxel now and `old` what was there (`id`, `name`, `state`).
//! - `on_neighbour_changed(block, face)`: the block on that face of it
//!   changed, or its properties did.
//! - `on_random_tick(block)`: now and then, at random (see
//!   [`RandomTicks`](super::random_tick::RandomTicks)).
//! - Items' `on_use(item, player)`: a player used the item in hand (right
//!   click) and it doesn't place a block. Returning `true` uses one up, in
//!   survival.
//!
//! Hooks run in `FixedUpdate`, with the simulation, from the edits and
//! uses of the frames before, through [`ScriptWorld`]: their changes are
//! ordinary edits, so they relight, remesh and save like any other, and
//! the hooks they cause run on the next tick. A block's hooks are called
//! with [`Caller::Block`], so an error disables that voxel's script, until
//! the scripts reload or another block is placed there; an item's with
//! [`Caller::Item`], disabling that item's.

use std::time::{Duration, Instant};

use bevy::{platform::collections::HashSet, prelude::*};
use mlua::Lua;

use super::{
    api::{PlayerView, ScriptWorld, handle, item_table, player_table},
    random_tick,
};
use crate::fluid_network::FluidSystems;
use defaria_core::{
    content::{BlockState, Content, Direction, ItemState},
    game_mode::{GameMode, use_up},
    inventory::Inventory,
    script::{Caller, ScriptRuntime, ScriptsReloaded},
    world::{Chunk, ChunkCoord, ChunkMap, PropertiesUpdated, VoxelChanged},
};

pub const ON_USED: &str = "on_used";
pub const ON_PLACED: &str = "on_placed";
pub const ON_BROKEN: &str = "on_broken";
pub const ON_NEIGHBOUR_CHANGED: &str = "on_neighbour_changed";
pub const ON_RANDOM_TICK: &str = "on_random_tick";
pub const ON_USE: &str = "on_use";
pub const ON_TICK: &str = "on_tick";
pub const ON_RECIPE_FINISHED: &str = "on_recipe_finished";

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<ScriptIndex>()
        .init_resource::<PendingHooks>()
        .init_resource::<HookSettings>()
        .add_message::<BlockUsed>()
        .add_message::<ItemUsed>()
        .add_systems(Update, rebuild_index.run_if(on_message::<ScriptsReloaded>))
        .configure_sets(
            FixedUpdate,
            (HookSystems::Collect, HookSystems::Run)
                .chain()
                // After the systems that edit voxels on their own, so each
                // run queues the same hooks on the same tick.
                .after(crate::fluid::run_flow)
                .after(FluidSystems),
        )
        .add_systems(
            FixedUpdate,
            (
                (collect_edits, collect_uses)
                    .chain()
                    .in_set(HookSystems::Collect),
                (run_block_hooks, run_item_hooks)
                    .chain()
                    .in_set(HookSystems::Run),
            ),
        );
    random_tick::plugin(app);
}

/// Where hooks are queued and run, in `FixedUpdate`: after fluid flow and
/// the fluid solver, the other systems there that edit voxels. What's
/// queued is in a fixed order (edits, then uses, then random ticks), so
/// hooks, and the random numbers they draw, repeat from run to run.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookSystems {
    /// Edits and uses since the last tick become hooks to call.
    Collect,
    /// The hooks are called.
    Run,
}

/// A player used the block at `pos` (whose script defines `on_used`).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BlockUsed {
    pub pos: IVec3,
    pub player: Option<PlayerView>,
}

/// Player `player` used `item` (whose script defines `on_use`).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ItemUsed {
    pub player: Entity,
    pub item: ItemState,
    pub view: PlayerView,
}

/// Limits on block hooks, per tick.
#[derive(Resource, Reflect, Debug, Clone, PartialEq)]
#[reflect(Resource)]
pub struct HookSettings {
    /// The most block hooks called in one tick. The rest wait for the next.
    pub max_per_tick: usize,
    /// The steps block hooks may use between them in one tick, about 3 ms
    /// of work: once they're spent, the rest wait for the next tick. On a
    /// network tick machines' `on_tick`s get as much again, and their
    /// `on_recipe_finished`s as much again (see `script::machine`). (Each
    /// call still gets its own `ScriptSettings::step_budget`, so a tick
    /// may go over by one call's.)
    pub steps_per_tick: u64,
    /// The wall-clock time block hooks may take between them in one
    /// tick, a safety net beside the steps (library functions do a lot
    /// of work in one step, and logging costs no steps): once it's
    /// passed, the rest wait for the next tick. The first hook of a tick
    /// always runs, so hooks keep moving however small it is. Machines'
    /// `on_tick`s and `on_recipe_finished`s each get as much again on a
    /// network tick. Unlike the steps
    /// it isn't deterministic.
    pub time_per_tick: Duration,
}

impl Default for HookSettings {
    fn default() -> Self {
        Self {
            max_per_tick: 1024,
            steps_per_tick: 250_000,
            time_per_tick: Duration::from_millis(4),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl HookSettings {
    /// The defaults with no time per tick, for tests: hooks wait only
    /// for the ticks' counts and steps, the same every run however busy
    /// the machine is.
    pub fn untimed() -> Self {
        Self {
            time_per_tick: Duration::MAX,
            ..default()
        }
    }
}

/// A block's script and which hooks it defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHooks {
    pub script: String,
    pub used: bool,
    pub placed: bool,
    pub broken: bool,
    pub neighbour_changed: bool,
    pub random_tick: bool,
    /// Machines' hooks (see `script::machine`).
    pub tick: bool,
    pub recipe_finished: bool,
}

impl BlockHooks {
    fn has(&self, hook: &str) -> bool {
        match hook {
            ON_USED => self.used,
            ON_PLACED => self.placed,
            ON_BROKEN => self.broken,
            ON_NEIGHBOUR_CHANGED => self.neighbour_changed,
            ON_RANDOM_TICK => self.random_tick,
            ON_TICK => self.tick,
            ON_RECIPE_FINISHED => self.recipe_finished,
            _ => false,
        }
    }
}

/// Which blocks and items have scripts, and which hooks those define, so
/// asking costs a lookup and a block without a hook costs nothing for it.
/// Rebuilt when the scripts reload.
#[derive(Resource, Debug)]
pub struct ScriptIndex {
    /// By index into [`Content::blocks`].
    blocks: Vec<Option<BlockHooks>>,
    /// By index into [`Content::items`]: the script, if it defines `on_use`.
    items: Vec<Option<String>>,
    /// Whether any block's script defines `on_random_tick`, so worlds
    /// with none skip looking for them.
    any_random_tick: bool,
    /// Whether any block's script defines `on_tick`, likewise.
    any_tick: bool,
}

impl FromWorld for ScriptIndex {
    fn from_world(world: &mut World) -> Self {
        match (
            world.get_resource::<Content>(),
            world.get_resource::<ScriptRuntime>(),
        ) {
            (Some(content), Some(runtime)) => Self::new(content, runtime),
            _ => Self {
                blocks: Vec::new(),
                items: Vec::new(),
                any_random_tick: false,
                any_tick: false,
            },
        }
    }
}

impl ScriptIndex {
    /// Indexes `content`'s scripted blocks and items, logging those whose
    /// script isn't there or didn't load.
    pub fn new(content: &Content, runtime: &ScriptRuntime) -> Self {
        let check = |kind: &str, id: &str, script: &str| {
            let loaded = runtime.is_loaded(script);
            if !loaded && runtime.sources().get(script).is_none() {
                error!("{kind} `{id}`'s script `{script}` doesn't exist");
            }
            loaded
        };
        let blocks: Vec<_> = content
            .blocks
            .iter()
            .map(|block| {
                let script = block.script.as_ref()?;
                check("block", &block.id, script).then(|| BlockHooks {
                    script: script.clone(),
                    used: runtime.has_hook(script, ON_USED),
                    placed: runtime.has_hook(script, ON_PLACED),
                    broken: runtime.has_hook(script, ON_BROKEN),
                    neighbour_changed: runtime.has_hook(script, ON_NEIGHBOUR_CHANGED),
                    random_tick: runtime.has_hook(script, ON_RANDOM_TICK),
                    tick: runtime.has_hook(script, ON_TICK),
                    recipe_finished: runtime.has_hook(script, ON_RECIPE_FINISHED),
                })
            })
            .collect();
        let items = content
            .items
            .iter()
            .map(|item| {
                let script = item.script.as_ref()?;
                (check("item", &item.id, script) && runtime.has_hook(script, ON_USE))
                    .then(|| script.clone())
            })
            .collect();
        let any_random_tick = blocks.iter().flatten().any(|hooks| hooks.random_tick);
        let any_tick = blocks.iter().flatten().any(|hooks| hooks.tick);
        Self {
            blocks,
            items,
            any_random_tick,
            any_tick,
        }
    }

    /// Whether any block's script defines `on_random_tick`.
    pub fn any_random_tick(&self) -> bool {
        self.any_random_tick
    }

    /// Whether any block's script defines `on_tick`.
    pub fn any_tick(&self) -> bool {
        self.any_tick
    }

    /// The hooks of `state`'s block, if it's scripted.
    pub fn block(&self, content: &Content, state: BlockState) -> Option<&BlockHooks> {
        self.blocks.get(content.state(state).block)?.as_ref()
    }

    /// `state`'s block's script, if it defines `hook`.
    pub fn block_hook(&self, content: &Content, state: BlockState, hook: &str) -> Option<&str> {
        self.block(content, state)
            .filter(|hooks| hooks.has(hook))
            .map(|hooks| hooks.script.as_str())
    }

    /// Whether using `state` calls its script's `on_used`.
    pub fn is_usable(&self, content: &Content, state: BlockState) -> bool {
        self.block_hook(content, state, ON_USED).is_some()
    }

    /// Whether `state`'s script has an `on_random_tick`.
    pub fn ticks_randomly(&self, content: &Content, state: BlockState) -> bool {
        self.block_hook(content, state, ON_RANDOM_TICK).is_some()
    }

    /// `item`'s script, if it defines `on_use`.
    pub fn item_use(&self, content: &Content, item: ItemState) -> Option<&str> {
        let index = content.item_state(item)?.item;
        self.items.get(index)?.as_deref()
    }
}

fn rebuild_index(
    // Left out in apps with scripts but no content, such as reloading's
    // tests: there's nothing to index.
    content: Option<Res<Content>>,
    runtime: Res<ScriptRuntime>,
    mut index: ResMut<ScriptIndex>,
) {
    if let Some(content) = content {
        *index = ScriptIndex::new(&content, &runtime);
    }
}

/// A block hook waiting to be called.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Hook {
    Used {
        pos: IVec3,
        player: Option<PlayerView>,
    },
    Placed(IVec3),
    Broken {
        pos: IVec3,
        before: BlockState,
    },
    NeighbourChanged {
        pos: IVec3,
        face: Direction,
    },
    RandomTick(IVec3),
}

/// The most block hooks that wait at once. Past that, new ones are
/// dropped (with a warning), so scripts that set off ever more hooks
/// can't use up memory.
const MAX_WAITING: usize = 1 << 16;

/// Remembers that a capped queue is dropping what's pushed onto it, so
/// it warns once: not again until it has drained below half its cap,
/// rather than every time a place frees up while it's held full.
#[derive(Debug, Default)]
pub(super) struct DropWarning {
    dropping: bool,
}

impl DropWarning {
    /// Whether a queue holding `len`, at most `cap`, takes another. The
    /// first one refused warns that `what` are dropped.
    pub(super) fn admits(&mut self, len: usize, cap: usize, what: &str) -> bool {
        if len < cap / 2 {
            self.dropping = false;
        }
        if len < cap {
            return true;
        }
        if !self.dropping {
            warn!("more than {cap} {what} are waiting; new ones are dropped");
            self.dropping = true;
        }
        false
    }

    /// Whether it has warned and the queue hasn't drained since.
    #[cfg(test)]
    pub(super) fn dropping(&self) -> bool {
        self.dropping
    }
}

/// The block hooks to call, oldest first.
#[derive(Resource, Debug, Default)]
pub(super) struct PendingHooks {
    hooks: Vec<Hook>,
    /// The neighbour-changed (by face) and random-tick hooks waiting: one
    /// that's already waiting isn't queued again, since when it runs it
    /// sees the block as it is then. So blocks that keep setting each
    /// other off queue at most one hook per face.
    waiting: HashSet<(IVec3, Option<Direction>)>,
    /// Whether hooks are being dropped, so the warning is given once.
    dropping: DropWarning,
}

impl PendingHooks {
    /// The key of the hooks that aren't queued twice.
    fn key(hook: &Hook) -> Option<(IVec3, Option<Direction>)> {
        match *hook {
            Hook::NeighbourChanged { pos, face } => Some((pos, Some(face))),
            Hook::RandomTick(pos) => Some((pos, None)),
            _ => None,
        }
    }

    /// Queues `hook`, unless the same one is already waiting or the
    /// queue is full.
    pub(super) fn push(&mut self, hook: Hook) {
        let key = Self::key(&hook);
        if key.is_some_and(|key| self.waiting.contains(&key)) {
            return;
        }
        if !self
            .dropping
            .admits(self.hooks.len(), MAX_WAITING, "block hooks")
        {
            return;
        }
        if let Some(key) = key {
            self.waiting.insert(key);
        }
        self.hooks.push(hook);
    }

    /// How many hooks are waiting.
    pub(super) fn len(&self) -> usize {
        self.hooks.len()
    }
}

/// Queues the hooks of the blocks placed, broken and next to a change.
fn collect_edits(
    mut changes: MessageReader<VoxelChanged>,
    mut updates: MessageReader<PropertiesUpdated>,
    content: Res<Content>,
    index: Res<ScriptIndex>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
    mut runtime: ResMut<ScriptRuntime>,
    mut pending: ResMut<PendingHooks>,
    mut ticking: ResMut<random_tick::TickingChunks>,
) {
    let mut changed = Vec::new();
    for change in changes.read() {
        let (before, after) = (content.state(change.before), content.state(change.after));
        if before.block != after.block {
            // A new block in the voxel: whatever disabled the old one's
            // script there is gone.
            runtime.enable(Caller::Block(change.pos));
            if index
                .block_hook(&content, change.before, ON_BROKEN)
                .is_some()
            {
                pending.push(Hook::Broken {
                    pos: change.pos,
                    before: change.before,
                });
            }
            if index
                .block_hook(&content, change.after, ON_PLACED)
                .is_some()
            {
                pending.push(Hook::Placed(change.pos));
            }
        }
        if index.ticks_randomly(&content, change.after) {
            ticking.add(change.pos);
        }
        changed.push(change.pos);
    }
    for update in updates.read() {
        if index.ticks_randomly(&content, update.after) {
            ticking.add(update.pos);
        }
        changed.push(update.pos);
    }
    for pos in changed {
        for side in Direction::ALL {
            let next = pos + side.normal();
            let face = side.opposite();
            let (coord, local) = ChunkCoord::split(next);
            let Some(state) = map
                .get(coord)
                .and_then(|chunk| chunks.get(chunk).ok())
                .map(|chunk| chunk.at(local))
            else {
                continue;
            };
            if index
                .block_hook(&content, state, ON_NEIGHBOUR_CHANGED)
                .is_some()
            {
                pending.push(Hook::NeighbourChanged { pos: next, face });
            }
        }
    }
}

pub(super) fn collect_uses(mut uses: MessageReader<BlockUsed>, mut pending: ResMut<PendingHooks>) {
    for used in uses.read() {
        pending.push(Hook::Used {
            pos: used.pos,
            player: used.player,
        });
    }
}

/// Calls the waiting block hooks, oldest first, up to
/// [`HookSettings::max_per_tick`] of them and until they've used
/// [`HookSettings::steps_per_tick`] or [`HookSettings::time_per_tick`].
/// The rest wait for the next tick.
fn run_block_hooks(
    mut pending: ResMut<PendingHooks>,
    settings: Res<HookSettings>,
    mut runtime: ResMut<ScriptRuntime>,
    index: Res<ScriptIndex>,
    mut world: ScriptWorld,
) {
    let start = runtime.steps_spent();
    let started = Instant::now();
    let spent = |runtime: &ScriptRuntime| runtime.steps_spent().saturating_sub(start);
    let pending = &mut *pending;
    let mut count = 0;
    let mut later = Vec::new();
    while count < pending.hooks.len().min(settings.max_per_tick)
        && spent(&runtime) < settings.steps_per_tick
        && (count == 0 || started.elapsed() < settings.time_per_tick)
    {
        let hook = pending.hooks[count];
        count += 1;
        if run_block_hook(&mut world, &mut runtime, &index, hook) {
            if let Some(key) = PendingHooks::key(&hook) {
                pending.waiting.remove(&key);
            }
        } else {
            later.push(hook);
        }
    }
    pending.hooks.drain(..count);
    pending.hooks.extend(later);
}

/// Calls one block hook, if the block there (or, for `on_broken`, the one
/// that was) still defines it. `false` if it has to wait: the block's
/// block entity, with its `data`, isn't there yet.
pub(super) fn run_block_hook(
    world: &mut ScriptWorld,
    runtime: &mut ScriptRuntime,
    index: &ScriptIndex,
    hook: Hook,
) -> bool {
    let (pos, name, state) = match hook {
        Hook::Used { pos, .. } => (pos, ON_USED, None),
        Hook::Placed(pos) => (pos, ON_PLACED, None),
        Hook::Broken { pos, before } => (pos, ON_BROKEN, Some(before)),
        Hook::NeighbourChanged { pos, .. } => (pos, ON_NEIGHBOUR_CHANGED, None),
        Hook::RandomTick(pos) => (pos, ON_RANDOM_TICK, None),
    };
    let Some(state) = state.or_else(|| world.get(pos)) else {
        return true;
    };
    let Some(script) = index.block_hook(&world.content, state, name) else {
        return true;
    };
    if name != ON_BROKEN
        && world.content.block_of(state).has_block_entity()
        && !world.has_block_entity(pos)
    {
        return false;
    }
    // A machine's hooks get its machine handle; a broken one's, the
    // block that's there now.
    let machine = name != ON_BROKEN && world.content.machine(state).is_some();
    let block = |lua: &Lua| handle(lua, pos, machine);
    let caller = Caller::Block(pos);
    // Errors are logged, and disable the caller, in the runtime.
    let _ = match hook {
        Hook::Used { player, .. } => {
            world.call::<_, ()>(runtime, caller, script, name, Some(pos), |lua, _| {
                let player = player.map(|p| player_table(lua, &p)).transpose()?;
                Ok((block(lua)?, player))
            })
        }
        Hook::Broken { before, .. } => {
            world.call::<_, ()>(runtime, caller, script, name, Some(pos), |lua, content| {
                Ok((block(lua)?, state_info(lua, content, before)?))
            })
        }
        Hook::NeighbourChanged { face, .. } => {
            world.call::<_, ()>(runtime, caller, script, name, Some(pos), |lua, _| {
                Ok((block(lua)?, face.name()))
            })
        }
        Hook::Placed(_) | Hook::RandomTick(_) => {
            world.call::<_, ()>(runtime, caller, script, name, Some(pos), |lua, _| {
                block(lua)
            })
        }
    };
    true
}

/// A block state as a table: `id`, `name` and `state`, its property values.
fn state_info(lua: &Lua, content: &Content, state: BlockState) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    let info = content.state(state);
    table.raw_set("id", content.block_of(state).id.as_str())?;
    table.raw_set("name", info.name.as_str())?;
    table.raw_set(
        "state",
        defaria_core::script::api::state_table(lua, &info.values)?,
    )?;
    Ok(table)
}

/// Calls the `on_use` of the items used, and uses one up (in survival)
/// for each that returns `true`.
fn run_item_hooks(
    mut uses: MessageReader<ItemUsed>,
    mut runtime: ResMut<ScriptRuntime>,
    index: Res<ScriptIndex>,
    mut players: Query<(Option<&mut Inventory>, Option<&GameMode>)>,
    mut world: ScriptWorld,
) {
    for used in uses.read() {
        let Some(script) = index.item_use(&world.content, used.item) else {
            continue;
        };
        let Some(kind) = world.content.item_state(used.item).map(|info| info.item) else {
            continue;
        };
        let origin = used.view.target.map(|(pos, _)| pos);
        let result = world.call::<_, Option<bool>>(
            &mut runtime,
            Caller::Item(kind),
            script,
            ON_USE,
            origin,
            |lua, content| {
                Ok((
                    item_table(lua, content, used.item)?,
                    player_table(lua, &used.view)?,
                ))
            },
        );
        if let Ok(Some(Some(true))) = result
            && let Ok((inventory, mode)) = players.get_mut(used.player)
        {
            let mode = mode.copied().unwrap_or_default();
            use_up(mode, inventory.map(Mut::into_inner), used.item);
        }
    }
}

#[cfg(test)]
mod tests;
