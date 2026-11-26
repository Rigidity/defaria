//! Hot reloading scripts in dev builds: every script file also loads
//! through the asset server, which watches it, and when one changes the
//! scripts reload with its new text, as images do.
//!
//! Only files that existed at startup are watched; a new script needs a
//! restart.

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    platform::collections::HashMap,
    prelude::*,
};

use super::{ScriptRuntime, ScriptsReloaded};

/// A script file's text.
#[derive(Asset, TypePath, Debug)]
pub struct ScriptAsset(pub String);

#[derive(Default, TypePath)]
struct ScriptAssetLoader;

impl AssetLoader for ScriptAssetLoader {
    type Asset = ScriptAsset;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> Result<ScriptAsset, std::io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        String::from_utf8(bytes)
            .map(ScriptAsset)
            .map_err(std::io::Error::other)
    }

    fn extensions(&self) -> &[&str] {
        &[super::source::EXTENSION]
    }
}

/// The script files being watched, by asset, with their asset paths.
#[derive(Resource, Default)]
struct WatchedScripts {
    files: HashMap<AssetId<ScriptAsset>, String>,
    handles: Vec<Handle<ScriptAsset>>,
}

impl WatchedScripts {
    fn watch(&mut self, handle: Handle<ScriptAsset>, file: String) {
        self.files.insert(handle.id(), file);
        self.handles.push(handle);
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_asset::<ScriptAsset>()
        .register_asset_loader(ScriptAssetLoader)
        .init_resource::<WatchedScripts>()
        .add_systems(Startup, watch_scripts)
        .add_systems(Update, reload_changed_scripts);
}

fn watch_scripts(
    server: Res<AssetServer>,
    runtime: Res<ScriptRuntime>,
    mut watched: ResMut<WatchedScripts>,
) {
    for source in runtime.sources().sources.values() {
        watched.watch(server.load(source.file.clone()), source.file.clone());
    }
}

/// Reloads every script when a watched one's text changes. The first
/// load of each (`Added`) is the text already loaded, so only `Modified`
/// counts.
fn reload_changed_scripts(
    mut events: MessageReader<AssetEvent<ScriptAsset>>,
    assets: Res<Assets<ScriptAsset>>,
    watched: Res<WatchedScripts>,
    mut runtime: ResMut<ScriptRuntime>,
    mut reloaded: MessageWriter<ScriptsReloaded>,
) {
    let mut sources = None;
    for event in events.read() {
        let AssetEvent::Modified { id } = event else {
            continue;
        };
        let (Some(file), Some(asset)) = (watched.files.get(id), assets.get(*id)) else {
            continue;
        };
        let sources = sources.get_or_insert_with(|| runtime.sources().clone());
        let unchanged = sources
            .sources
            .values()
            .any(|source| &source.file == file && source.text == asset.0);
        if !unchanged {
            info!("{file} changed: reloading the scripts");
            sources.set_text(file, asset.0.clone());
        }
    }
    if let Some(sources) = sources
        && &sources != runtime.sources()
    {
        runtime.reload(Some(sources));
        reloaded.write(ScriptsReloaded {
            generation: runtime.generation(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{Caller, ScriptPlugin, tests::fixture};

    /// Changing a watched script's text reloads the scripts: the new
    /// hook runs, and a caller an earlier error disabled works again.
    #[test]
    fn changing_a_script_reloads_it() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScriptPlugin));
        app.insert_resource(ScriptRuntime::new(fixture(), default()));
        let file = "tests/fixtures/script-mods/scripted/scripts/answer.luau";
        let original = std::fs::read_to_string(file).unwrap();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<ScriptAsset>>()
            .add(ScriptAsset(original));
        app.world_mut()
            .resource_mut::<WatchedScripts>()
            .watch(handle.clone(), "mods/scripted/scripts/answer.luau".into());
        app.update();

        let caller = Caller::Other(1);
        let mut runtime = app.world_mut().resource_mut::<ScriptRuntime>();
        let generation = runtime.generation();
        assert_eq!(
            runtime.call_with::<_, i32>(caller, "scripted:answer", "on_answer", ()),
            Ok(Some(42))
        );
        // Disable the caller.
        assert!(
            runtime
                .call_with::<_, ()>(caller, "scripted:nowhere", "on_answer", ())
                .is_err()
        );
        assert!(runtime.is_disabled(caller));

        app.world_mut()
            .resource_mut::<Assets<ScriptAsset>>()
            .get_mut(&handle)
            .unwrap()
            .0 = "function on_answer() return 7 end".into();
        // Asset events go out at the end of a frame, so they're read on the
        // next.
        app.update();
        app.update();

        let mut runtime = app.world_mut().resource_mut::<ScriptRuntime>();
        assert_eq!(runtime.generation(), generation + 1);
        assert!(!runtime.is_disabled(caller));
        assert_eq!(
            runtime.call_with::<_, i32>(caller, "scripted:answer", "on_answer", ()),
            Ok(Some(7))
        );
        let reloaded = app.world().resource::<Messages<ScriptsReloaded>>();
        assert!(!reloaded.is_empty());
    }
}
