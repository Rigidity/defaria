---
name: review-loop
description: Take a piece of defaria work (a feature, gameplay change, bug fix, rendering or performance work, or a follow-up to an open PR) all the way to a reviewed PR that passes the same checks as CI, running rounds of independent code review and headless playtesting and fixing what they find, without the user having to ask for each pass. Use this whenever the user asks to implement something "with reviews", to "drive this to completion", "review until clean", "keep reviewing and fixing", "take this through review", or hands over a task in this repo and expects a finished, reviewed PR rather than a first draft, even if they don't say "review loop". Not for a one-off review of an existing diff with no fix loop; use code-review for that.
---

# Review loop

The user's usual workflow is: an agent implements something, then the user asks for a review pass, the agent fixes the findings, and this repeats until the work is clean. This skill runs that loop for them, for a Bevy 0.20 game.

A game needs two kinds of review. Code review catches logic, ECS and architecture problems. A playtest catches what code review can't: geometry that renders inside out, a camera that moves the wrong way, a feature that's correct but feels wrong, a frame-time regression. The repo's quality bar (`AGENTS.md`, "Verifying changes") is unit tests for everything that can be checked in code, and the headless runner (`cargo headless <script>`) for everything that has to be seen or measured. Every round checks both.

The loop is: **scope and write the brief → implement → verify yourself → review → verify findings and fix → repeat until a stop condition → final checks → commit, PR and report.**

## Ground rules for this repo

These apply to you and to every reviewer:

- **Never open the game window.** No `cargo run`, `cargo play`, `cargo shot` or `--screenshot`. Use `cargo headless <script>` to see the game. The user plays the windowed game themselves.
- **Keep Bevy builds shared.** Don't run `cargo clean`, don't add `--release` or `--no-default-features` while iterating (each builds a second copy of Bevy), and don't change Bevy's version or features, `rust-toolchain.toml` or `[profile.*]` unless that's the task.
- **One build at a time.** Every worktree of this project shares one target directory (set in the `.cargo/config.toml` above the worktrees). Parallel cargo commands wait on each other. If cargo prints `Blocking waiting for file lock`, wait; don't kill anything.
- **Screenshots go to the scratchpad**, not the repo: scenario `screenshot` paths are relative to the working directory, so use absolute scratchpad paths.

## 1. Scope the work and write the state file

Read the source first: `gh issue view <n> --comments` or `gh pr view <n> --comments` if there is one, the user's request, and the code it touches. Read `AGENTS.md`; it's short and it's the contract. When the work is an issue, mark it started on the roadmap: `scripts/roadmap.sh status <n> doing`.

**Several independent features in one request.** Don't put them all on one branch. Give each its own branch, state file and loop, and stack the branches with `gh stack`. If the request names a list and it's unclear which items to do, ask: that's the one question worth asking up front.

**Open choices.** The user prefers that you make decisions and bring them up at the end, rather than stopping to ask. When the request leaves open anything on the reviewer brief's "for the user" list (controls and keybindings, feel and tuning values, visual style, data, mod and save formats, new dependencies or Bevy features, scope), make the call: pick what fits the request and the existing game, and the more conservative option when it's close. Record it under Decisions as provisional, and keep going. Ask up front only when no reasonable choice exists.

**Follow-ups to an open PR.** Do what the user asked; if they didn't say, continue on their own `rigidity/` branches and stack on anyone else's. Base is set so reviewers see only the new work.

- *Continuing on the PR's branch* (`gh pr checkout <n>`): keep its branch name, set Base to the commit the follow-up starts from, and push to that PR in section 8.
- *Stacking on top*: if the PR is already in a stack, `gh stack checkout <n>`; otherwise `gh pr checkout <n>` first, then adopt it with `gh stack init <pr-branch>`. Then `gh stack add rigidity/<slug>`. Base is the parent branch.

The state file is the memory of the loop. Reviewers start with fresh context, and after a few rounds yours is long, so without it later rounds re-raise settled points or undo earlier decisions. Write it to the session scratchpad (not the repo), at `<scratchpad>/review-loop/<branch-slug>.md`. Every review round reads it, and every fix round updates it.

```markdown
# <short title>

## Brief (shared with reviewers)
- Goal: <what this change must achieve, in the user's terms: what the player sees and does>
- Serves: <issue/PR link, or "user request">
- Base: <origin/main, the parent branch in a gh stack, or the commit a PR follow-up starts from>
- Branch: <rigidity/...>
- Decisions: <each decision and why, noting who made it: the user, the issue, or the implementer. Mark choices that are the user's to confirm as "provisional, for the user: X, because Y; the alternative is Z".>
- Out of scope: <things deliberately left alone, and why>
- Settled: <points the user or the issue decided; not open for review>
- Previously rejected: <findings rejected in earlier rounds: the claim and the evidence>

## Playtest (shared with reviewers)
- Scenarios: <scenario files that exercise this change: reusable ones in scenarios/, one-offs in the scratchpad>
- What to look for: <what the screenshots and inspect output should show if the change works>
- Last run: <round, and what you saw>

## Decisions for the user
- <each provisional decision: the options, what you picked and why, and what switching would take>

## Review log (implementer only)
### Round 1
- [confirmed, major] <finding> -> fixed in <commit>, regression test <where>
- [rejected] <finding> -> <evidence>
- [deferred] <finding> -> <why, and where it's recorded>
```

## 2. Implement

Work on a `rigidity/` branch (lowercase; never `Rigidity/`) created from a freshly fetched `origin/main`, unless you're continuing someone else's PR branch. For a bug fix, write the failing test first, or a scenario that shows the bug if it can only be seen.

Follow `AGENTS.md`: one plugin per feature in its own module under `src/`, registered in `GamePlugin` (`src/lib.rs`, the only line you touch there); `Reflect` with `#[reflect(Component)]` on gameplay components; small single-purpose systems; messages and observers over systems that reach into many unrelated queries.

Every change needs both kinds of check:

- **Tests** for everything deterministic: logic, math, parsing, state changes, input handling. Unit tests build an `App` with `MinimalPlugins` and only the systems under test, step time with `TimeUpdateStrategy::ManualDuration` (under 250 ms; the first update has zero delta), and assert on the world. For behaviour that crosses plugins, write an end-to-end test that runs a script through the real game with no GPU (`headless::default_plugins(false)` + `GamePlugin` + `HeadlessPlugin`; see `script_drives_the_fly_camera` in `src/headless.rs`). CI has no GPU, so tests always use `rendering: false`.
- **A scenario** for everything visible: write a script that exercises the change, run it with `cargo headless <script>`, and open every screenshot with the Read tool. Look at it as a player would. Keep scenarios that will stay useful in `scenarios/`, and make sure the existing ones still run and still look right.

**The headless runner is yours to extend.** When a check needs something it can't do (frame-time measurement, UI interaction, a new input type, a new kind of output), add it to `src/headless.rs` in the same change or a commit before it: document the command in the module docs and `AGENTS.md`, and test the parser and behaviour. Don't work around a missing capability with the window.

Before the first round, build everything reviewers will use, so they don't all start by rebuilding: `cargo build && cargo test --no-run`. Run your scenarios, fill in the Playtest section, and commit, so each round's fixes are a separate diff.

## 3. Review rounds

Each round, launch the reviewers **in parallel** with the Agent tool (`general-purpose`, `run_in_background: false`), one per focus:

1. **Correctness**: gameplay logic, ECS behaviour (system order, queries, deferred commands, messages, change detection), math and coordinate conventions, panics, frame-rate independence, and work that runs every frame.
2. **Tests**: whether the unit and end-to-end tests would catch a regression in each behaviour the change adds, and whether they're deterministic and run without a GPU.
3. **Simplification and architecture**: duplicated logic, Bevy built-ins or repo helpers that should be used, plugin and module layout, and structure that doesn't match the rest of the game.
4. **Playtest**: runs the game headless with the change, writes its own scenarios to probe it, looks at every screenshot, and checks visuals, feel, edge-case input and performance.

Adjust the set to the change:

- **Small diff** (roughly under 100 changed lines): one code reviewer covering focuses 1–3, plus the playtest reviewer if the change is visible.
- **Nothing visible** (a parser, a data format, the runner's own internals, docs): skip the playtest reviewer, and say so in the report.
- **Performance work**: tell the playtest reviewer the baseline to compare against, and that measurement is its main job this round.

Build each prompt from [references/reviewer-brief.md](references/reviewer-brief.md), which holds the template, the checklist for each focus, the severities, and which choices are for the user. Give reviewers the Brief and Playtest sections, the base ref, the scratch directory and the focus, and nothing else: not your summary of the change, your reasoning about why it's correct, or the review log. They read the diff (`git diff <base>...HEAD`) and form their own view, and that independence is what makes a round worth running. The playtest reviewer gets your scenario list so it can start there, but it must also write its own. From round 2, reviewers still review the whole diff against the base, since fixes break things elsewhere; name the areas that changed since the last round so they look closely there.

The built-in `code-review` skill is an alternative for the correctness focus on a large diff (`/code-review high`), but it doesn't see the Brief, so check its findings against the goal and the Settled points yourself.

## 4. Verify, then fix

Reviewers are sometimes confidently wrong. Before acting on a finding, check it:

- **A claimed bug**: reproduce it, with a failing unit test or a scenario that shows it (`cargo headless <scratch script>`, then look at the screenshot or `inspect` output). A finding you can't reproduce is rejected unless you can see from the code that it's right.
- **A visual or feel finding**: run the reviewer's scenario, or yours, and look at the screenshots yourself. Screenshots taken on the first few frames can be incomplete; check that the reviewer waited a few frames before capturing.
- **A performance finding**: re-measure, on the base and on the branch, with the same scenario, long enough (hundreds of frames) that startup doesn't dominate.
- **A claimed missing test**: check whether an existing test already covers it.
- **A simplification**: check that the suggested helper or Bevy API does what the reviewer thinks in Bevy 0.20. Read its source in `~/.cargo/registry/src/*/bevy_*-0.20.*/` rather than trusting memory of older Bevy versions; many APIs were renamed between 0.14 and 0.20.
- **A finding marked "for the user"**: it's confirmed if the choice really is open: neither the request nor Settled answers it. Handle it as in section 6.

Mark each finding in the review log as **confirmed**, **rejected** (with the evidence), or **deferred** (real but out of scope, with where it's recorded). Add rejected ones to the Brief's Previously rejected list and deferred ones to Out of scope, so later rounds don't raise them again. Fix every confirmed blocker and major finding, and minor ones when they're cheap. Each confirmed correctness finding gets a regression test, or a scenario in `scenarios/` when it can only be seen. Nits are optional.

Then run the quick checks (`cargo test`, or `cargo test <module>::` while iterating; `cargo clippy --all-targets -- -D warnings`), rerun the scenarios and look at the screenshots, update the Brief and Playtest sections, and commit (section 8).

## 5. When to stop

- **Clean**: a round with no confirmed findings at major or above. Stop there; never start a round just for minor findings or nits.
- **No progress**: the rounds are going in circles. Stop after fixing a round's findings if any of these holds:
  - a confirmed finding is the same defect, in the same code, as one that an earlier round fixed. A different bug in the same area doesn't count;
  - for two rounds in a row, every confirmed major-or-above finding is in code that the previous round's fixes wrote;
  - from round 4 on, the count of confirmed major-or-above findings went up in each of the last two rounds (for example 1, 2, 3). A count that holds steady doesn't count: rounds that each find and fix different real issues are making progress.

  Say in the report which signal held. A loop that circles usually has a design problem that another round won't fix, and the user should look.
- **Out of rounds**: stop after round 10 (or the limit the user gave) even if findings remain, and list them in the report.

Before stopping for no progress or out of rounds, if the last round's fixes were more than trivial, run one more round with only the correctness reviewer, plus the playtest reviewer if the fixes changed anything visible, to check the fixes; fix what they confirm and report it, without another round.

Whatever the stop, still run the final checks and open or update the PR, so the user has the work in hand.

## 6. Decisions during the loop

These don't end the loop or interrupt the user. When a confirmed finding raises a choice that's the user's to confirm (the list is in the reviewer brief), keep the current choice or switch, whichever the evidence favours. Record it under Decisions as provisional, so later reviewers don't raise it again, and add it to Decisions for the user. A reviewer's new argument against a provisional decision goes into that entry, so the user sees both sides. The PR and the report carry every provisional decision, and the user picks at the end. For feel and tuning, the user decides by playing: say in the report exactly what to try.

## 7. Final checks

These are the checks in `.github/workflows/ci.yml`, plus a final playtest. Fix anything they report:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --locked --no-default-features -- -D warnings   # CI runs it; it's a check, not a second full build
cargo test --locked
```

- **Scenarios:** don't run all of `scenarios/` locally. CI runs every one on each push, unrendered, in about a minute and a half, and `cargo test` already checks that each one parses and fills with real blocks. Render only the scenarios whose output the change affects (the new and changed ones, and those that show what the change touches), open their screenshots and confirm they still show what they should. After pushing, check the CI run (`gh pr checks <n>`) and fix any scenario it fails.
- CI installs Bevy's Linux dependencies and runs on Ubuntu with no GPU. Tests that pass locally but need a GPU, a window, an audio device or macOS-only behaviour will fail there; the end-to-end tests must use `rendering: false`.
- If you changed `Cargo.toml` or `Cargo.lock`, make sure `--locked` passes, since CI uses it.
- If these checks led to changes beyond formatting, say so in the report, since no reviewer saw them.

## 8. Commits, PR and report

**Commits.** One focused commit per logical change. Messages follow the git log: a plain sentence as the subject that says what changed ("Add a headless scenario runner"), or "Address review findings in <area>" with a `-` bulleted body when one round fixes several small things. No prefix like `feat:`. No AI attribution of any kind: no `Co-Authored-By`, no "Generated with" line.

**PR.** Push and open a ready-for-review PR with `gh pr create --base <branch>`, where the branch is Base without the `origin/` prefix, or push to the existing PR when continuing on its branch. The title follows the commit style. The body says what changed and why, links what it serves (`Closes #<n>` for an issue it finishes, so the roadmap project marks it Done; see "Issues and the roadmap" in `AGENTS.md`), lists the provisional decisions for the user, and has a **How to try it** section: the keys to press and what the player should see in the windowed game, plus the scenario that shows it headless. If the change is visible, it also has a **Screenshots** section, made from the final playtest's screenshots with `scripts/pr-screenshots.sh` as `AGENTS.md` describes ("Screenshots in pull requests"). When a later round changes what's visible, refresh them. Follow the user's global instructions for the risk assessment at the end. For stacked work, use `gh stack` rather than setting PR bases by hand: `gh stack submit --auto --open`, then set each title and body with `gh pr edit <n> --title ... --body ...`.

**Report.** End with this, in the reply to the user:

```markdown
## <title> — <PR link>

<two or three sentences: what was done, and what the user should look at first>

### Try it
<in the windowed game: what to press and what you should see. Mention anything about feel the user should judge, since headless runs can't.>

### Review rounds
- Round 1: <reviewers run>, <n> findings, <n> confirmed. Caught: <the substantive ones, one line each>
- Round 2: ...
- Stopped because: <clean / no progress (which signal) / out of rounds>

### Rejected findings
- <finding> — <why it was rejected>

### Left for you
- <each provisional decision: what you picked and why, the alternatives, and what switching would take>
- <confirmed findings left unfixed, if the loop stopped for no progress or ran out of rounds>

### Checks
<each final check and whether it passed; the scenarios rendered locally and what the screenshots showed; the CI run's result, including its scenario shards>
```

Show the most telling screenshot from the final playtest in the report with Markdown image syntax and its absolute path.

Keep the state file and the scenario screenshots until the user has seen the report; they're the place to look if the user asks why something was decided.
