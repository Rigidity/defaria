//! The `systems` command: time per system, to break a slow frame down by
//! what it ran.
//!
//! When a script uses `systems`, every system in the app's schedules and
//! the render world's is wrapped, before any of them first runs, in an
//! adapter that adds the time it took to a clock of its own (Bevy's
//! `trace` feature would give the same detail, but it's a Bevy feature
//! change and a full rebuild). Each clock keeps wall-clock time and the
//! CPU time of the thread the system ran on, which a busy machine (other
//! builds, other games) barely changes, where it can stretch wall-clock
//! time many times over. At the end of each frame the clocks are read and
//! reset. A `systems` report covers the frames since the last one (or the
//! start): for the busiest of them, the one whose systems used the most
//! CPU time, the systems that used the most in it, with their mean and
//! worst over all the frames, and its wall-clock time.
//!
//! What this leaves out: work a system hands to other threads (a
//! `par_iter`'s, the async pools') counts only in wall-clock time, and
//! commands (spawning, despawning, inserting) are applied between systems,
//! so they count in neither. A system that runs other systems (the render
//! world's `render_system` runs the render graph's) counts only its own
//! time, not theirs. Systems run in parallel, so their times don't
//! add up to the frame's. The systems that only run other schedules (the
//! main schedule's, the fixed loop's and the render world's) aren't timed,
//! since their time is everything they ran. The render world runs
//! alongside the next frame's main schedules, so its systems' times land on
//! the frame they finished in. Off Linux and macOS the CPU clock is the
//! wall clock.

use std::{
    any::TypeId,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bevy::{
    ecs::{
        schedule::{ApplyDeferred, SystemWithAccess},
        system::{Adapt, AdapterSystem, RunSystemError, SystemIn},
    },
    prelude::*,
    render::RenderApp,
};

/// A system's wall-clock and CPU nanoseconds since the clocks were last
/// read.
#[derive(Default)]
struct Clock {
    wall: AtomicU64,
    cpu: AtomicU64,
}

/// One clock per system name, shared by every copy of it (a system added
/// to more than one schedule, or run more than once a frame).
#[derive(Resource, Default)]
pub(crate) struct SystemClocks {
    clocks: Vec<(String, Arc<Clock>)>,
}

impl SystemClocks {
    fn clock(&mut self, name: String) -> Arc<Clock> {
        if let Some((_, clock)) = self.clocks.iter().find(|(n, _)| *n == name) {
            return clock.clone();
        }
        let clock = Arc::new(Clock::default());
        self.clocks.push((name, clock.clone()));
        clock
    }

    /// What each system took since the last read, by index, starting each
    /// over.
    fn take(&self) -> Vec<Spent> {
        self.clocks
            .iter()
            .map(|(_, clock)| Spent {
                wall: clock.wall.swap(0, Ordering::Relaxed),
                cpu: clock.cpu.swap(0, Ordering::Relaxed),
            })
            .collect()
    }
}

/// A system's time in a frame, in nanoseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Spent {
    pub wall: u64,
    pub cpu: u64,
}

/// The CPU time this thread has used, in nanoseconds.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn thread_cpu() -> u64 {
    use rustix::time::{ClockId, clock_gettime};
    let time = clock_gettime(ClockId::ThreadCPUTime);
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

/// Without a CPU clock, the wall clock.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn thread_cpu() -> u64 {
    use std::sync::LazyLock;
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_nanos() as u64
}

thread_local! {
    /// The timed systems running on this thread, innermost last, each with
    /// the time of the timed systems it has run so far: a system that runs
    /// a schedule (the render world's `render_system` runs the render
    /// graph's) would otherwise count their time as its own too.
    static NESTED: std::cell::RefCell<Vec<Spent>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Times the system it wraps into a clock: its own time, less any timed
/// systems it ran.
struct Timed(Arc<Clock>);

impl Adapt<SystemWithAccess> for Timed {
    type In = ();
    type Out = ();

    fn adapt(
        &mut self,
        (): (),
        run_system: impl FnOnce(SystemIn<'_, SystemWithAccess>) -> Result<(), RunSystemError>,
    ) -> Result<(), RunSystemError> {
        NESTED.with_borrow_mut(|stack| stack.push(Spent::default()));
        let (start, cpu) = (Instant::now(), thread_cpu());
        let result = run_system(());
        let took = Spent {
            wall: start.elapsed().as_nanos() as u64,
            cpu: thread_cpu().saturating_sub(cpu),
        };
        // Its own time: less what the systems it ran took, which counts
        // in theirs; and all of it is what its parent ran, if it has one.
        let inner = NESTED.with_borrow_mut(|stack| {
            let inner = stack.pop().unwrap_or_default();
            if let Some(parent) = stack.last_mut() {
                parent.wall += took.wall;
                parent.cpu += took.cpu;
            }
            inner
        });
        let clock = &self.0;
        clock
            .wall
            .fetch_add(took.wall.saturating_sub(inner.wall), Ordering::Relaxed);
        clock
            .cpu
            .fetch_add(took.cpu.saturating_sub(inner.cpu), Ordering::Relaxed);
        result
    }
}

/// Wraps every system in `world`'s schedules that haven't run yet in a
/// [`Timed`] adapter, named with `prefix`.
fn wrap_schedules(world: &mut World, clocks: &mut SystemClocks, prefix: &str) {
    let Some(mut schedules) = world.get_resource_mut::<Schedules>() else {
        return;
    };
    for (_, schedule) in schedules.iter_mut() {
        // A schedule that has run has moved its systems out of its graph,
        // already initialized; only wrap ones that haven't.
        if schedule.systems().is_ok() {
            continue;
        }
        let systems = &mut schedule.graph_mut().systems;
        let keys: Vec<_> = systems.iter().map(|(key, ..)| key).collect();
        for key in keys {
            let Some(slot) = systems.get_mut(key) else {
                continue;
            };
            // An `ApplyDeferred` is found by its type, to sync commands at
            // it, so it can't be wrapped.
            if runs_schedules(&slot.name().to_string())
                || slot.system_type() == TypeId::of::<ApplyDeferred>()
            {
                continue;
            }
            let placeholder: Box<dyn System<In = (), Out = ()>> =
                Box::new(IntoSystem::into_system(|| {}));
            let inner = std::mem::replace(slot, SystemWithAccess::new(placeholder));
            let name = inner.name();
            let clock = clocks.clock(format!("{prefix}{}", short_name(&name.to_string())));
            *slot = SystemWithAccess::new(Box::new(AdapterSystem::new(Timed(clock), inner, name)));
        }
    }
}

/// Whether a system is one that runs other schedules (the main schedule's,
/// the fixed loop's and the render world's), whose time is all of theirs: left untimed, so
/// the report shows what they ran instead.
fn runs_schedules(name: &str) -> bool {
    [
        "::run_main",
        "::run_fixed_main",
        "::run_fixed_main_schedule",
        "::run_render_schedule",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

/// A system's name without the game's crate path: `terrain::remesh…`
/// rather than `defaria::terrain::remesh…`.
fn short_name(name: &str) -> &str {
    name.strip_prefix("defaria::").unwrap_or(name)
}

/// Wraps every system of the app and its render world, so the `systems`
/// command can report them. Call once every plugin has added its systems,
/// before the app's first update, and before pipelined rendering moves
/// the render world to its own thread (in its plugin's `cleanup`).
pub(crate) fn time_systems(app: &mut App) {
    let mut clocks = SystemClocks::default();
    wrap_schedules(app.world_mut(), &mut clocks, "");
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        wrap_schedules(render.world_mut(), &mut clocks, "render: ");
    }
    app.insert_resource(clocks).init_resource::<SystemTimes>();
}

/// What the systems took over the frames since the last report.
#[derive(Resource, Default, Debug)]
pub(crate) struct SystemTimes {
    frames: u32,
    /// Each clock's total CPU time, by its index in [`SystemClocks`].
    total: Vec<u64>,
    /// Each clock's most CPU time in one frame.
    worst: Vec<u64>,
    /// The busiest frame: its number, wall-clock time and each clock's.
    busiest: Option<(u32, Duration, Vec<Spent>)>,
}

impl SystemTimes {
    /// Adds a frame: its number, how long it took and each clock's time.
    pub(crate) fn add(&mut self, frame: u32, took: Duration, spent: Vec<Spent>) {
        self.frames += 1;
        self.total.resize(spent.len(), 0);
        self.worst.resize(spent.len(), 0);
        for (index, spent) in spent.iter().enumerate() {
            self.total[index] += spent.cpu;
            self.worst[index] = self.worst[index].max(spent.cpu);
        }
        let cpu = |spent: &[Spent]| spent.iter().map(|s| s.cpu).sum::<u64>();
        if self
            .busiest
            .as_ref()
            .is_none_or(|(_, _, busiest)| cpu(&spent) > cpu(busiest))
        {
            self.busiest = Some((frame, took, spent));
        }
    }

    /// The report: the busiest frame, and the `count` systems that used the
    /// most CPU time in it, each with its CPU time there, its mean and
    /// worst over every frame, and its wall-clock time there. Starts over.
    pub(crate) fn report(&mut self, names: &[&str], count: usize) -> Vec<String> {
        let times = std::mem::take(self);
        let Some((frame, took, spent)) = times.busiest else {
            return vec!["systems: no frames yet".into()];
        };
        let ms = |nanos: u64| nanos as f64 / 1e6;
        let mut order: Vec<usize> = (0..spent.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(spent[index].cpu));
        let mut lines = vec![format!(
            "systems over {} frames; the busiest, frame {frame}, used {:.2} ms of CPU in \
             its systems and took {:.2} ms (CPU in it / mean / worst, wall in it, in ms):",
            times.frames,
            ms(spent.iter().map(|s| s.cpu).sum()),
            took.as_secs_f64() * 1000.0
        )];
        for index in order.into_iter().take(count) {
            lines.push(format!(
                "  {:7.2} {:7.2} {:7.2} {:7.2}  {}",
                ms(spent[index].cpu),
                ms(times.total[index]) / times.frames as f64,
                ms(times.worst[index]),
                ms(spent[index].wall),
                names.get(index).copied().unwrap_or("?"),
            ));
        }
        lines
    }
}

/// Reads the clocks into [`SystemTimes`] at the end of a frame that took
/// `took`.
pub(crate) fn record(world: &mut World, frame: u32, took: Duration) {
    let Some(clocks) = world.get_resource::<SystemClocks>() else {
        return;
    };
    let spent = clocks.take();
    world.resource_mut::<SystemTimes>().add(frame, took, spent);
}

/// The `systems` report, for the `count` slowest systems.
pub(crate) fn report(world: &mut World, count: usize) -> Vec<String> {
    let Some(clocks) = world.get_resource::<SystemClocks>() else {
        return vec!["systems: not timed".into()];
    };
    let names: Vec<String> = clocks.clocks.iter().map(|(n, _)| n.clone()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    world.resource_mut::<SystemTimes>().report(&names, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;

    #[derive(Resource, Default)]
    struct Ran(u32);

    /// Wrapped systems still run, and their clocks see their time: a
    /// system that sleeps takes wall-clock time but hardly any CPU, one
    /// that spins takes both.
    #[test]
    fn wrapped_systems_run_and_are_timed() {
        fn sleepy(mut ran: ResMut<Ran>) {
            ran.0 += 1;
            std::thread::sleep(Duration::from_millis(3));
        }
        fn busy() {
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(3) {
                std::hint::black_box(0);
            }
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                16,
            )))
            .init_resource::<Ran>()
            .add_systems(Update, (sleepy, busy).chain());
        time_systems(&mut app);
        for frame in 0..3 {
            app.update();
            record(app.world_mut(), frame, Duration::from_millis(10));
        }
        assert_eq!(app.world().resource::<Ran>().0, 3);
        let lines = report(app.world_mut(), usize::MAX);
        assert!(
            lines[0].starts_with("systems over 3 frames; the busiest"),
            "{lines:?}"
        );
        let columns = |name: &str| -> Vec<f64> {
            let line = lines.iter().find(|l| l.ends_with(name)).expect(name);
            line.split_whitespace()
                .take(4)
                .map(|n| n.parse().unwrap())
                .collect()
        };
        // The busy one first, with its CPU time; the sleepy one with its
        // wall-clock time but little CPU.
        assert!(lines[1].ends_with("::busy"), "{lines:?}");
        assert!(columns("::busy")[0] >= 2.5, "{lines:?}");
        let sleepy = columns("::sleepy");
        assert!(sleepy[3] >= 3.0 && sleepy[0] < 1.0, "{lines:?}");
        // It started over.
        assert_eq!(report(app.world_mut(), 2), ["systems: no frames yet"]);
    }

    #[derive(bevy::ecs::schedule::ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
    struct Inner;

    /// A system that runs a schedule counts only its own time; the busy
    /// system in that schedule counts its own.
    #[test]
    fn systems_that_run_systems_count_only_their_own_time() {
        fn busy() {
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(3) {
                std::hint::black_box(0);
            }
        }
        fn outer(world: &mut World) {
            world.run_schedule(Inner);
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Inner, busy)
            .add_systems(Update, outer);
        time_systems(&mut app);
        app.update();
        record(app.world_mut(), 0, Duration::from_millis(10));
        let lines = report(app.world_mut(), usize::MAX);
        let cpu = |name: &str| -> f64 {
            let line = lines.iter().find(|l| l.ends_with(name)).expect(name);
            line.split_whitespace().next().unwrap().parse().unwrap()
        };
        assert!(cpu("::busy") >= 2.5, "{lines:?}");
        assert!(cpu("::outer") < 1.0, "{lines:?}");
    }

    #[test]
    fn the_report_keeps_the_busiest_frame() {
        let spent = |a: u64, b: u64| {
            vec![
                Spent {
                    wall: a * 2,
                    cpu: a,
                },
                Spent {
                    wall: b * 2,
                    cpu: b,
                },
            ]
        };
        let mut times = SystemTimes::default();
        times.add(1, Duration::from_millis(30), spent(1_000_000, 0));
        times.add(2, Duration::from_millis(20), spent(2_000_000, 8_000_000));
        times.add(3, Duration::from_millis(9), spent(4_000_000, 0));
        let lines = times.report(&["a", "b"], 5);
        assert_eq!(
            lines,
            [
                "systems over 3 frames; the busiest, frame 2, used 10.00 ms of CPU in its \
                 systems and took 20.00 ms (CPU in it / mean / worst, wall in it, in ms):",
                "     8.00    2.67    8.00   16.00  b",
                "     2.00    2.33    4.00    4.00  a",
            ]
        );
    }
}
