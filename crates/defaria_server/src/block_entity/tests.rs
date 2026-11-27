use std::{collections::BTreeMap, path::Path, sync::OnceLock};

use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::{
    save::chunk::{StateNames, decode, encode},
    testing::{HOME, move_loader, world_app},
};
use defaria_core::{
    testing::{temp_folder, test_content},
    world::Voxels,
};

/// A test part, saved as an int, that only `test:counter` blocks have:
/// what a feature plugin adds.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
struct Ticks(i64);

impl BlockEntityPart for Ticks {
    const KEY: &'static str = "ticks";
    const VERSION: u32 = 2;

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        (content.block_of(state).id == "test:counter").then_some(Self(0))
    }

    fn save(&self, _: &Content) -> Value {
        Value::Int(self.0)
    }

    fn load(value: &Value, version: u32, _: BlockState, _: &Content) -> Option<Self> {
        (version == 2).then(|| value.as_int().map(Self))?
    }
}

/// The base game, and mod `test` with blocks that have block entities:
/// `counter` (with a `lit` property, and data to start with) and `crate`.
fn content() -> &'static Content {
    static CONTENT: OnceLock<Content> = OnceLock::new();
    CONTENT.get_or_init(|| {
        test_content(&[
            (
                "blocks/counter.json5",
                r#"{
                    properties: { lit: { values: ["false", "true"] } },
                    model: "defaria:cube",
                    textures: { all: "defaria:lamp" },
                    block_entity: { data: { count: 0, name: "counter" } },
                }"#,
            ),
            (
                "blocks/crate.json5",
                r#"{
                    model: "defaria:cube",
                    textures: { all: "defaria:glass" },
                    block_entity: {},
                }"#,
            ),
        ])
    })
}

fn state(id: &str, values: &[(&str, &str)]) -> BlockState {
    content().block_state(id, values).unwrap()
}

fn counter() -> BlockState {
    state("test:counter", &[])
}

fn lit_counter() -> BlockState {
    state("test:counter", &[("lit", "true")])
}

fn crate_block() -> BlockState {
    state("test:crate", &[])
}

/// The terrain, light, streaming and block entities with no rendering,
/// loading a small world around `at`, saved to `folder` if there is one.
fn app(folder: Option<&Path>, at: Vec3) -> App {
    world_app(content().clone())
        .plugins(BlockEntityPlugin)
        .with(|app| {
            app.register_block_entity_part::<Ticks>();
        })
        .folder(folder)
        .home(at)
        .build()
}
const AWAY: Vec3 = Vec3::new(8.0, 5000.0, 8.0);
const POS: IVec3 = IVec3::new(3, 50, -5);

fn set(app: &mut App, pos: IVec3, state: BlockState) {
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| assert!(voxels.set(pos, state)))
        .unwrap();
    app.update();
}

/// The block entity at `pos`.
fn at(app: &App, pos: IVec3) -> Option<Entity> {
    app.world().resource::<BlockEntities>().get(pos)
}

fn part<P: Component + Clone>(app: &App, pos: IVec3) -> Option<P> {
    app.world().get::<P>(at(app, pos)?).cloned()
}

fn set_part<P: Component>(app: &mut App, pos: IVec3, value: P) {
    let entity = at(app, pos).unwrap();
    app.world_mut().entity_mut(entity).insert(value);
    app.update();
}

fn data(count: i64) -> BlockData {
    BlockData(BTreeMap::from([
        ("count".into(), Value::Int(count)),
        ("name".into(), Value::Text("counter".into())),
    ]))
}

/// Placing a block with a block entity makes one under its chunk, with
/// its block's defaults and the parts features add for it.
#[test]
fn placing_creates_a_block_entity() {
    let mut app = app(None, HOME);
    assert_eq!(at(&app, POS), None);
    set(&mut app, POS, counter());
    let entity = at(&app, POS).unwrap();
    assert_eq!(
        app.world().get::<BlockEntity>(entity),
        Some(&BlockEntity { pos: POS })
    );
    assert_eq!(part::<BlockData>(&app, POS), Some(data(0)));
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(0)));
    let chunk = app
        .world()
        .resource::<ChunkMap>()
        .get(ChunkCoord::split(POS).0)
        .unwrap();
    assert_eq!(app.world().get::<ChildOf>(entity).unwrap().parent(), chunk);

    // Another block's has only what it asks for.
    let other = POS + IVec3::X;
    set(&mut app, other, crate_block());
    assert!(at(&app, other).is_some());
    assert_eq!(part::<BlockData>(&app, other), None);
    assert_eq!(part::<Ticks>(&app, other), None);
}

/// Changing only the block's state keeps its block entity; breaking or
/// replacing the block removes it.
#[test]
fn state_changes_keep_it_and_breaking_removes_it() {
    let mut app = app(None, HOME);
    set(&mut app, POS, counter());
    set_part(&mut app, POS, Ticks(7));
    let entity = at(&app, POS).unwrap();

    set(&mut app, POS, lit_counter());
    assert_eq!(at(&app, POS), Some(entity), "kept across a state change");
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(7)));

    set(&mut app, POS, crate_block());
    assert_ne!(at(&app, POS), Some(entity), "replaced");
    assert!(app.world().get_entity(entity).is_err());
    assert_eq!(part::<Ticks>(&app, POS), None);

    set(&mut app, POS, BlockState::AIR);
    assert_eq!(at(&app, POS), None, "broken");
    let mut all = app.world_mut().query::<&BlockEntity>();
    assert_eq!(all.iter(app.world()).count(), 0);
}

/// Breaking and placing the same block in one frame gives a new block
/// entity, and leaves nothing behind.
#[test]
fn several_changes_in_a_frame() {
    let mut app = app(None, HOME);
    set(&mut app, POS, counter());
    set_part(&mut app, POS, Ticks(5));
    app.world_mut()
        .run_system_once(|mut voxels: Voxels| {
            voxels.set(POS, BlockState::AIR);
            voxels.set(POS, counter());
            voxels.set(POS, BlockState::AIR);
            voxels.set(POS, counter());
        })
        .unwrap();
    app.update();
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(0)), "a new one");
    let mut all = app.world_mut().query::<&BlockEntity>();
    assert_eq!(all.iter(app.world()).count(), 1);
}

/// A block entity's parts survive its chunk unloading and loading again,
/// including changes made to them after the block was placed in a chunk
/// that was already saved.
#[test]
fn block_entities_survive_unloading() {
    let mut app = app(None, HOME);
    set(&mut app, POS, counter());
    // Away and back, so the chunk comes from the save.
    move_loader(&mut app, AWAY);
    assert_eq!(at(&app, POS), None, "unloaded with its chunk");
    assert!(app.world().resource::<BlockEntities>().is_empty());
    move_loader(&mut app, HOME);
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(0)));

    // Only a part changes: that alone marks the chunk for saving.
    set_part(&mut app, POS, Ticks(42));
    set_part(&mut app, POS, data(3));
    move_loader(&mut app, AWAY);
    move_loader(&mut app, HOME);
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(42)));
    assert_eq!(part::<BlockData>(&app, POS), Some(data(3)));
}

/// Set by a test: the next frame, before `Update`, sets the counter at
/// [`POS`] to this, as `FixedUpdate` would.
#[derive(Resource, Default)]
struct ChangeOnNextFrame(Option<i64>);

fn change_on_next_frame(
    mut change: ResMut<ChangeOnNextFrame>,
    index: Res<BlockEntities>,
    mut ticks: Query<&mut Ticks>,
) {
    if let Some(value) = change.0
        && let Some(mut part) = index.get(POS).and_then(|entity| ticks.get_mut(entity).ok())
    {
        part.0 = value;
        change.0 = None;
    }
}

/// A part changed on the frame after its chunk spawned, before `Update`
/// noted anything, still marks the chunk for saving.
#[test]
fn a_change_right_after_loading_is_saved() {
    let mut app = app(None, HOME);
    set(&mut app, POS, counter());
    move_loader(&mut app, AWAY);
    app.init_resource::<ChangeOnNextFrame>()
        .add_systems(First, change_on_next_frame);
    app.world_mut().resource_mut::<ChangeOnNextFrame>().0 = Some(11);
    move_loader(&mut app, HOME);
    // Settling can end on the frame the chunk spawns; the change is made on
    // the next.
    for _ in 0..10 {
        if app.world().resource::<ChangeOnNextFrame>().0.is_none() {
            break;
        }
        app.update();
    }
    assert_eq!(app.world().resource::<ChangeOnNextFrame>().0, None);
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(11)));
    move_loader(&mut app, AWAY);
    move_loader(&mut app, HOME);
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(11)));
}

/// The [`BlockEntityChanged`] messages sent, collected by a test.
#[derive(Resource, Default)]
struct Changes(Vec<IVec3>);

fn collect_changes(mut changes: MessageReader<BlockEntityChanged>, mut seen: ResMut<Changes>) {
    seen.0.extend(changes.read().map(|change| change.pos));
}

/// Loading a block entity back isn't a change: its chunk isn't saved
/// again unless something changes.
#[test]
fn reloading_is_not_a_change() {
    let mut app = app(None, HOME);
    set(&mut app, POS, counter());
    set_part(&mut app, POS, Ticks(5));
    app.init_resource::<Changes>()
        .add_systems(Last, collect_changes);
    move_loader(&mut app, AWAY);
    // Only what happens from here on (messages last until a fixed step
    // has run, which a fast machine may not have yet).
    app.world_mut().resource_mut::<Changes>().0.clear();
    move_loader(&mut app, HOME);
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(5)));
    assert_eq!(app.world().resource::<Changes>().0, Vec::<IVec3>::new());
    set_part(&mut app, POS, Ticks(6));
    assert_eq!(app.world().resource::<Changes>().0, vec![POS]);
}

/// A world on disk keeps its block entities through a restart.
#[test]
fn block_entities_survive_a_restart() {
    let folder = temp_folder("block-entities");
    {
        let mut app = app(Some(&folder), HOME);
        set(&mut app, POS, lit_counter());
        set_part(&mut app, POS, Ticks(9));
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
    let app = app(Some(&folder), HOME);
    assert_eq!(part::<Ticks>(&app, POS), Some(Ticks(9)));
    assert_eq!(part::<BlockData>(&app, POS), Some(data(0)));
    let entity = at(&app, POS).unwrap();
    assert_eq!(
        app.world().get::<BlockEntity>(entity),
        Some(&BlockEntity { pos: POS })
    );
}

/// A saved chunk in memory, as the store would read it back.
fn saved(chunk: &Chunk, names: &StateNames) -> Chunk {
    decode(&encode(chunk, names), names).unwrap()
}

fn counter_data(ticks: i64) -> BlockEntityData {
    BlockEntityData {
        kind: "test:counter".into(),
        parts: BTreeMap::from([(
            "ticks".into(),
            SavedPart {
                version: 2,
                value: Value::Int(ticks),
            },
        )]),
    }
}

/// Block entities round trip through the chunk format.
#[test]
fn block_entities_round_trip_through_the_save() {
    let names = StateNames::new(content());
    let mut chunk = Chunk::default();
    let pos = IVec3::new(1, 2, 3);
    chunk.set_at(pos, counter());
    chunk.set_block_entity_at(pos, counter_data(12));
    chunk.set_at(IVec3::ZERO, crate_block());
    chunk.set_block_entity_at(
        IVec3::ZERO,
        BlockEntityData {
            kind: "test:crate".into(),
            parts: default(),
        },
    );
    assert_eq!(saved(&chunk, &names), chunk);
}

/// A block entity whose kind no mod defines any more is dropped on load,
/// as is one whose voxel isn't its block.
#[test]
fn unknown_kinds_are_dropped() {
    let names = StateNames::new(content());
    let pos = IVec3::new(4, 5, 6);
    let mut chunk = Chunk::default();
    chunk.set_at(pos, counter());
    chunk.set_block_entity_at(pos, counter_data(1));
    let blob = encode(&chunk, &names);

    // Without mod `test`: the block is air, and its data's gone.
    let without = StateNames::new(defaria_core::content::base());
    let loaded = decode(&blob, &without).unwrap();
    assert_eq!(loaded.at(pos), BlockState::AIR);
    assert_eq!(loaded.block_entities().count(), 0);

    // Data for a voxel of another block.
    let mut wrong = Chunk::default();
    wrong.set_at(pos, crate_block());
    wrong.set_block_entity_at(pos, counter_data(1));
    assert_eq!(saved(&wrong, &names).block_entities().count(), 0);
}

/// A chunk loaded with a block whose block entity wasn't saved (say, a mod
/// gave the block one since) gets a new one; one saved for a voxel whose
/// block has none any more is dropped; parts no feature adds are dropped;
/// a part saved at a version it can't read starts over.
#[test]
fn loading_matches_block_entities_to_blocks() {
    let folder = temp_folder("block-entities-mismatch");
    let names = StateNames::new(content());
    let coord = ChunkCoord::split(POS).0;
    let local = ChunkCoord::split(POS).1;
    let (fresh, stale, old) = (POS, POS + IVec3::X, POS + IVec3::Z);
    {
        // A chunk saved by some other build of the game.
        let mut chunk = Chunk::default();
        chunk.set_at(local, counter());
        chunk.set_block_entity_at(
            local + IVec3::X,
            BlockEntityData {
                kind: "air".into(),
                parts: default(),
            },
        );
        chunk.set_at(local + IVec3::Z, counter());
        let mut old_data = counter_data(3);
        old_data.parts.get_mut("ticks").unwrap().version = 1;
        old_data.parts.insert(
            "gone".into(),
            SavedPart {
                version: 1,
                value: Value::Bool(true),
            },
        );
        chunk.set_block_entity_at(local + IVec3::Z, old_data);
        let store = crate::save::WorldStore::new(Some(&folder), content());
        store.save(coord.0, saved(&chunk, &names));
        store.flush();
    }
    let app = app(Some(&folder), HOME);
    assert_eq!(part::<Ticks>(&app, fresh), Some(Ticks(0)), "made new");
    assert_eq!(part::<BlockData>(&app, fresh), Some(data(0)));
    assert_eq!(at(&app, stale), None, "dropped");
    assert_eq!(part::<Ticks>(&app, old), Some(Ticks(0)), "started over");
}

/// What loading drops (a block entity whose block has none, one saved for
/// another block, a part no feature adds, a part its block doesn't have
/// and that can't be read) is dropped from the save too: the chunk is
/// saved again without it.
#[test]
fn what_loading_drops_leaves_the_save() {
    let folder = temp_folder("block-entities-dropped");
    let names = StateNames::new(content());
    let (coord, local) = ChunkCoord::split(POS);
    let store = || crate::save::WorldStore::new(Some(&folder), content());
    {
        let mut chunk = Chunk::default();
        chunk.set_block_entity_at(
            local,
            BlockEntityData {
                kind: "air".into(),
                parts: default(),
            },
        );
        // A crate never has ticks, and can't read these.
        chunk.set_at(local + IVec3::X, crate_block());
        let mut unreadable = counter_data(3);
        unreadable.kind = "test:crate".into();
        unreadable.parts.get_mut("ticks").unwrap().version = 1;
        chunk.set_block_entity_at(local + IVec3::X, unreadable);
        // A counter with a part no feature adds.
        chunk.set_at(local + IVec3::Z, counter());
        let mut gone = counter_data(3);
        // With all it does have, so loading it changes nothing else.
        gone.parts.insert(
            "data".into(),
            SavedPart {
                version: 1,
                value: Value::Table(data(0).0),
            },
        );
        gone.parts.insert(
            "gone".into(),
            SavedPart {
                version: 1,
                value: Value::Bool(true),
            },
        );
        chunk.set_block_entity_at(local + IVec3::Z, gone);
        let store = store();
        store.save(coord.0, saved(&chunk, &names));
        store.flush();
    }
    {
        let mut app = app(Some(&folder), HOME);
        assert_eq!(part::<Ticks>(&app, POS + IVec3::Z), Some(Ticks(3)));
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
    let chunk = store().load(coord.0).unwrap();
    // The air's block entity is gone, the crate keeps no unreadable part
    // (whether an empty one is written at all is the saver's business),
    // and the counter keeps all but the part no feature adds.
    let left: Vec<_> = chunk
        .block_entities()
        .map(|(at, data)| (Chunk::position_of(at), data.clone()))
        .collect();
    assert!(left.iter().all(|(at, _)| *at != local), "{left:?}");
    let crate_parts = left.iter().find(|(at, _)| *at == local + IVec3::X);
    assert!(
        crate_parts.is_none_or(|(_, data)| data.parts.is_empty()),
        "{left:?}"
    );
    let (_, data) = left
        .iter()
        .find(|(at, _)| *at == local + IVec3::Z)
        .expect("the counter's block entity is saved");
    assert!(data.parts.contains_key("ticks"));
    assert!(!data.parts.contains_key("gone"), "{data:?}");
}

/// Block entities show up in `inspect` and BRP: they're reflected.
#[test]
fn block_entities_are_reflected() {
    let mut app = app(None, HOME);
    set(&mut app, POS, counter());
    let registry = app.world().resource::<AppTypeRegistry>().read();
    for name in ["BlockEntity", "BlockData"] {
        let registration = registry.get_with_short_type_path(name).unwrap();
        assert!(registration.data::<ReflectComponent>().is_some(), "{name}");
    }
}
