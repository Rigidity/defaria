//! Render pipeline status: whether any failed to compile or are still
//! compiling, shared from the render world so the run can fail on a
//! broken shader.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use bevy::{
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        render_resource::{CachedPipelineState, PipelineCache},
    },
    shader::ShaderCacheError,
};

/// How many render pipelines have failed and how many are still pending,
/// written in the render world and read in the main world when the script
/// ends. Both counts live in one atomic so a reader never mixes two frames.
#[derive(Resource, Clone, Default)]
pub(super) struct PipelineStatus(Arc<AtomicU64>);

impl PipelineStatus {
    fn store(&self, failed: u32, pending: u32) {
        self.0.store(
            (u64::from(failed) << 32) | u64::from(pending),
            Ordering::Relaxed,
        );
    }

    /// `(failed, pending)`.
    pub(super) fn load(&self) -> (u32, u32) {
        let packed = self.0.load(Ordering::Relaxed);
        ((packed >> 32) as u32, packed as u32)
    }
}

/// Shares one [`PipelineStatus`] between the main world and the render
/// world, and keeps it up to date from the render world.
pub(super) fn share_pipeline_status(app: &mut App) {
    let status = PipelineStatus::default();
    app.insert_resource(status.clone());
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(status).add_systems(
            Render,
            // After `RenderSystems::Render`, where the pipeline queue is
            // processed, so this frame's results are in.
            record_pipeline_status.in_set(RenderSystems::Cleanup),
        );
    }
}

fn record_pipeline_status(cache: Res<PipelineCache>, status: Res<PipelineStatus>) {
    let (mut failed, mut pending) = (0, 0);
    for pipeline in cache.pipelines() {
        match pipeline_outcome(&pipeline.state) {
            PipelineOutcome::Ready => {}
            PipelineOutcome::Failed => failed += 1,
            PipelineOutcome::Pending => pending += 1,
        }
    }
    status.store(failed, pending);
}

#[derive(Debug, PartialEq)]
enum PipelineOutcome {
    Ready,
    /// Failed in a way Bevy won't retry, such as a shader that doesn't
    /// compile.
    Failed,
    /// Queued, compiling, or waiting to retry a shader or import that hasn't
    /// loaded yet.
    Pending,
}

fn pipeline_outcome(state: &CachedPipelineState) -> PipelineOutcome {
    match state {
        CachedPipelineState::Ok(_) => PipelineOutcome::Ready,
        CachedPipelineState::Queued | CachedPipelineState::Creating(_) => PipelineOutcome::Pending,
        CachedPipelineState::Err(error) => match error {
            ShaderCacheError::ShaderNotLoaded(_)
            | ShaderCacheError::ShaderImportNotYetAvailable => PipelineOutcome::Pending,
            ShaderCacheError::ProcessShaderError(_) | ShaderCacheError::CreateShaderModule(_) => {
                PipelineOutcome::Failed
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::{END_TIMEOUT_FRAMES, HeadlessPlugin, Script, default_plugins};

    /// A headless app with no GPU running `script`, with `status` standing
    /// in for what the render world would report.
    fn app_with_pipelines(script: &str, status: PipelineStatus) -> App {
        let mut app = App::new();
        app.add_plugins((
            default_plugins(false).disable::<bevy::log::LogPlugin>(),
            HeadlessPlugin {
                script: Script::parse(script).unwrap(),
                rendering: false,
            },
        ))
        .insert_resource(status);
        app.finish();
        app.cleanup();
        app
    }

    fn status(failed: u32, pending: u32) -> PipelineStatus {
        let status = PipelineStatus::default();
        status.store(failed, pending);
        status
    }

    fn run_until_exit(app: &mut App, frames: usize) -> Option<AppExit> {
        (0..frames).find_map(|_| {
            app.update();
            app.should_exit()
        })
    }

    /// A pipeline that failed to compile (say, a broken shader) fails the
    /// run. Simulated, since tests have no GPU to compile shaders on.
    #[test]
    fn failed_pipelines_fail_the_run() {
        let mut app = app_with_pipelines("wait 1", status(1, 0));
        assert_eq!(run_until_exit(&mut app, 10), Some(AppExit::error()));
    }

    /// The run doesn't end while pipelines are still compiling, so a short
    /// script can't finish before a broken shader is found.
    #[test]
    fn exit_waits_for_pipelines_to_finish_compiling() {
        let status = status(0, 1);
        let mut app = app_with_pipelines("wait 1", status.clone());
        assert_eq!(run_until_exit(&mut app, 20), None);
        status.store(1, 0);
        assert_eq!(run_until_exit(&mut app, 5), Some(AppExit::error()));
    }

    /// A pipeline that never finishes compiling (a shader that never loads)
    /// ends the run with an error instead of hanging it.
    #[test]
    fn gives_up_on_pipelines_that_never_finish() {
        let mut app = app_with_pipelines("wait 1", status(0, 1));
        let frames = END_TIMEOUT_FRAMES as usize + 10;
        assert_eq!(run_until_exit(&mut app, frames), Some(AppExit::error()));
    }

    #[test]
    fn pipeline_status_round_trips_both_counts() {
        let status = status(3, 70_000);
        assert_eq!(status.load(), (3, 70_000));
    }

    /// Shader errors Bevy retries are pending, not failed, and ones it won't
    /// retry are failures.
    #[test]
    fn sorts_pipeline_states_into_outcomes() {
        use CachedPipelineState::{Err, Queued};
        let cases = [
            (Queued, PipelineOutcome::Pending),
            (
                Err(ShaderCacheError::ShaderNotLoaded(default())),
                PipelineOutcome::Pending,
            ),
            (
                Err(ShaderCacheError::ShaderImportNotYetAvailable),
                PipelineOutcome::Pending,
            ),
            (
                Err(ShaderCacheError::ProcessShaderError(String::new())),
                PipelineOutcome::Failed,
            ),
            (
                Err(ShaderCacheError::CreateShaderModule(String::new())),
                PipelineOutcome::Failed,
            ),
        ];
        for (state, outcome) in cases {
            assert_eq!(pipeline_outcome(&state), outcome, "{state:?}");
        }
    }

    /// The main world reads the very counters the render world writes, and
    /// the render world updates them every frame.
    #[test]
    fn pipeline_status_is_shared_with_the_render_world() {
        let mut app = App::new();
        app.insert_sub_app(RenderApp, SubApp::new());
        share_pipeline_status(&mut app);
        let main = app.world().resource::<PipelineStatus>().clone();
        let render_app = app.sub_app(RenderApp);
        let render = render_app.world().resource::<PipelineStatus>();
        assert!(Arc::ptr_eq(&main.0, &render.0));
        let schedule = render_app
            .world()
            .resource::<Schedules>()
            .get(Render)
            .expect("no Render schedule");
        assert!(
            schedule
                .graph()
                .systems
                .iter()
                .any(|(_, system, _)| system.name().to_string().contains("record_pipeline_status")),
            "record_pipeline_status isn't in the Render schedule"
        );
    }
}
