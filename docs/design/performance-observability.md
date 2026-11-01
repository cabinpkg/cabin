# Performance observability: design and coverage

This is the design and the working coverage record for measuring Cabin's own performance: the
trace capture, the benchmark suite, the profiling workflows, baseline comparison, and CI.  The
contributor-facing guide is [`performance.md`](performance.md).  Update the tables below in the
same change as the work they describe.

## What is measured, and by what

Five kinds of measurement answer different questions and are never substituted for each other.

| Kind | Question | Source | Scope and clock |
| --- | --- | --- | --- |
| End-to-end time | How long does the user wait? | `hyperfine` runs of a release `cabin` binary | Whole process, wall clock, from spawn to exit |
| Internal phases | Where inside Cabin did that time go? | `tracing` spans, captured with `CABIN_TRACE_DIR` | Cabin's thread(s), monotonic clock, inclusive span lifetimes; time spent waiting on I/O or a child process counts toward the span that waits |
| CPU and stacks | What code burns CPU? | Sampling profilers (`samply`, `perf`, Instruments) on a `profiling`-profile binary | Sampled call stacks; CPU time, not waiting |
| Memory | How much memory, and where allocated? | `getrusage` in the timings summary, `hyperfine` peak RSS, heap profilers | Cabin alone, its largest child, or the largest process of the run, stated per number; peaks are never summed |
| Work counters | Did the amount of work change? | Span counts and integer fields (HTTP requests, solve rounds, cache hits); Ninja actions from the build's `.ninja_log` | Deterministic for a fixed workload; independent of machine speed |

Rules that follow from the table:

- Summed span time is not elapsed time.  Spans overlap across threads and nest; the summary's
  `total_us` is a per-name sum of inclusive lifetimes and is labeled as such.  No exclusive-time or
  critical-path figure is derived: Cabin's own work is sequential on one thread, apart from
  short-lived helper threads, and the parallel part (Ninja's actions) is shown as Ninja recorded
  it, not re-derived.
- Waiting is not CPU.  The trace shows wall time; CPU time comes from `getrusage` and the
  profilers.
- A deterministic counter that changes (an extra compile action, an extra metadata request) is a
  regression signal on its own, however noisy the wall clock is.

## Tool choices

| Need | Choice | Why |
| --- | --- | --- |
| Span facade in domain crates | `tracing` 0.1.44 | Zero-cost when no subscriber exists; the Rust ecosystem's standard; Cargo uses it |
| Subscriber composition | `tracing-subscriber` 0.3 registry, `Targets` filter | No regex dependency; `Targets` directives cover per-crate detail |
| Trace file | `tracing-chrome` 0.7.2, Chrome trace-event JSON | Opens in Perfetto and `chrome://tracing`; Cargo's own choice.  Its writer unwraps I/O errors, so Cabin wraps the writer (see below).  It numbers threads per process, so a later in-process capture can draw a helper thread in its caller's lane |
| End-to-end timing | `hyperfine` 2.0 | Warmup, per-run prepare/conclude, raw samples, CPU and peak-RSS per run, versioned JSON |
| In-process benchmarks | Criterion | Statistics, baselines, and change detection for subsystem benches |
| Build-action timeline | Ninja's own `.ninja_log`, delimited to the invocation | Authoritative and already written; `ninjatracing`'s last-build heuristic replays history on a no-op build (see below) |
| CPU profiling | `samply` (Linux, macOS), Instruments (macOS), `perf` (Linux) | Maintained samplers with symbolization; no Cabin code |

## Trace capture design

- **Activation.**  `CABIN_TRACE_DIR=<dir>` enables capture for one invocation;
  `CABIN_TRACE_FILTER` takes `Targets` directives (default `info`).  An environment variable, not a
  flag: capture is a developer diagnostic, must reach a `cabin` started by a script or benchmark
  harness, and must not add a flag to every subcommand's help.  `--profile`, `--target`, and
  `--build-dir` keep their meanings.
- **Ownership.**  Domain crates depend on the `tracing` facade only; `cabin/src/trace.rs` is the one
  place a subscriber is built (`docs/architecture.md`, "Performance tracing").
- **Scoping.**  The subscriber is installed with `dispatcher::with_default` for the duration of
  `cabin::run`, never globally, so repeated and concurrent in-process invocations each capture
  their own command and cannot fail on a second global initialization.  Cabin's own work runs on
  one thread, apart from short-lived helper threads, each handed the capture's dispatcher; its
  other concurrency is child processes (Ninja's actions, test executables), which appear as their
  own timelines rather than as threads.
- **Files.**  Each capture's stem is `<command>-<unix-ms>-<pid>-<seq>`, unique per process and
  per in-process invocation, so concurrent or nested invocations never write the same file.  The
  trace is written as `<stem>.trace.json.partial` and renamed only after the writer thread has
  joined and the closing bracket is on disk.
- **Failure behavior.**  An invalid filter or an unusable directory refuses the command before it
  runs: a capture the user asked for is never silently missing.  A write failure during the run
  is recorded by the wrapping writer (later writes are dropped instead of panicking), the command's
  exit status is unchanged, a warning names the `.partial` file, and the summary says
  `"trace_complete": false`.  An interrupted process (Ctrl-C, `SIGKILL`) leaves only the
  `.partial` file: Cabin installs no signal handler, and adding one would change cancellation
  behavior to benefit a diagnostic.
- **Disabled cost.**  With the variable unset no subscriber, writer thread, or file exists; each
  span site reduces to the `tracing` level check.  Measured in the overhead table below.

### The timings summary

`<stem>.timings.json` is the stable, versioned report.  Its fields, units, and meanings are fixed
within a `format_version`.  Which phases appear, and their names and integer fields, follow the
code's spans and may change between commits; a comparison across revisions treats a phase present
on one side only as a change in instrumentation, not a regression.

### Ninja actions

A persistent `.ninja_log` holds earlier builds' entries, so the timeline must be cut to the current
invocation: Cabin keeps the log's contents from before it starts Ninja, and reads only the entries
appended after them.  A no-op build therefore reports zero actions rather than the previous
build's.  Ninja compacts the log when it loads one past 100 entries with more than three per
output (`BuildLog::Load` and `BuildLog::Recompact` in its `src/build_log.cc`, the same from 1.10
through 1.13): it writes the same header, then each kept output's latest entry exactly as the old
log had it, and the run appends after those.  Every entry a run logs carries a modification time
from that run (Ninja 1.12 logs the command's start; earlier versions the output's, which every
Cabin command rewrites), so none repeats an earlier line; Cabin skips the leading lines found in
the old log and reads the rest, provided none of those is an old line either.  Any other rewrite
reports the action timeline as unavailable, never as zero: a log of another format, which Ninja
upgrades or starts over (Ninja before 1.12 also compacts a newer one under its own header).
Compacting the log with `ninja -t recompact` before the build would keep the prefix, but that tool
also compacts `.ninja_deps` against the current manifest, so a traced build would drop dependency
records before an untraced one would.
Ninja writes an entry only for an action that succeeded, so a failed build lists its completed
actions and says the failed one is absent.  Several outputs of one command share one entry's
start, end, and command hash and count as one action.  Ninja's times are milliseconds from its own
build start, a clock Cabin cannot read; actions are placed at Cabin's spawn instant, and the gap
between Ninja's spawn and its first action (manifest load, startup) is the stated alignment
uncertainty.  `NinjaLog` in `trace.rs` implements this.

## Coverage

Trace coverage per workflow.  `info` spans mark phases; `debug` spans mark per-item steps.

| Workflow / subsystem | Owner | Spans | Status |
| --- | --- | --- | --- |
| Command startup | `cabin` | root `cabin` span (`command`, `startup_us`); summary `startup_us` | done |
| Config discovery | `cabin-config` | `discover_config_files` (debug) | done |
| Workspace and manifest load | `cabin-workspace`, `cabin-manifest`, `cabin` | `load_workspace_and_patches`, `load_workspace`, `discover_workspace_root`, `load_manifest` (debug), `resolve_package_selection` (debug) | done |
| Feature resolution | `cabin-feature` | `resolve_features` (`roots`, `packages`) | done |
| File index | `cabin-index` | `load_file_index` (`roots`, `loaded`), `parse_package_entry` (debug, `bytes`) | done |
| HTTP index | `cabin-index-http` | `open_http_index`, `load_http_index` (`roots`, `fetched`), `http_get` (debug: `package`, `status`, `bytes`) | done |
| Dependency solving and lazy metadata | `cabin-resolver`, `cabin` | `resolve_with_lazy_index` (`rounds`), `resolve` / `resolve_packages` (`roots`, `index_packages`), `held_back_report`, `select_locked_revisions` (debug) | done |
| Lockfile | `cabin-lockfile` | `read_lockfile`, `write_lockfile` (debug) | done |
| Artifact cache, checksum, extraction | `cabin-artifact`, `cabin` | `run_artifact_pipeline`, `build_fetch_plan` (HTTP downloads), `fetch_artifacts` (`packages`), `fetch_one`, `ensure_archive` / `ensure_source` (`hit`), `extract_zip` (debug) | done |
| Package and vendor | `cabin-package`, `cabin-vendor` | `stage_package`, `collect_package_files`, `build_zip`, `write_staged`, `materialize_vendor` | done |
| System dependencies | `cabin-system-deps` | `probe_system_dependency` (debug) | done |
| Toolchain detection and probes | `cabin-toolchain` | `detect_toolchain`, `version_probe` (debug, `tool`) | done |
| Build preparation, planning, usage requirements, link closure | `cabin`, `cabin-build` | `prepare_workspace`, `plan` (`actions`), `collect_interface` / `collect_link_libs` (debug) | done |
| Driver lowering, Ninja and compile-database emission | `cabin-driver`, `cabin-ninja` | `lower` (trace), `write_build_ninja`, `write_compile_commands`, `render_build_ninja` / `render_compile_commands` (debug), `write_if_changed` (debug, `bytes`, `changed`) | done |
| Build execution | `cabin`, Ninja | `run_ninja`; the `ninja` lane and summary `ninja_actions` from `.ninja_log` | done |
| Test planning and execution | `cabin-test`, `cabin` | `plan_tests`, `run_tests` (`executables`); the `tests` lane | done |

Measurement-infrastructure checks:

| Property | Check |
| --- | --- |
| Stdout unchanged by capture | `trace_capture::traced_metadata_keeps_stdout_byte_identical` |
| Failure keeps its diagnostic and finishes the capture | `trace_capture::a_failing_command_keeps_its_error_and_finishes_the_capture` |
| Malformed filter / unusable directory refuse to run | `trace_capture::malformed_filter_refuses_to_run_the_command`, `an_unusable_trace_dir_refuses_to_run_the_command`, unit `an_invalid_filter_is_refused_before_any_file_exists` |
| Concurrent processes never share a file | `trace_capture::concurrent_invocations_write_separate_complete_captures` |
| Repeated in-process invocations | unit `repeated_sessions_in_one_thread_each_capture_their_own_command` |
| Write failure is reported, not a panic or a false "complete" | unit `a_write_failure_leaves_a_partial_trace_and_says_so`, `checked_writer_keeps_the_first_error_and_swallows_the_rest` |

## Benchmark coverage

End-to-end scenarios (`cargo perf list`) and Criterion benches (`benches/domain.rs`) per family.
Each scenario's check is what keeps a number honest; see `performance.md`.

| Family | CLI scenarios | Check | Criterion | Status |
| --- | --- | --- | --- | --- |
| Startup and no-op | `startup/version`, `inspect/metadata-*`, `build/noop-*` | exit 0; no Ninja action | - | done |
| Resolution | `resolve/wide`, `resolve/deep`, `resolve/update-wide` | exit 0, from no lockfile; the lockfile lists every package | `resolver/wide`, `resolver/deep`, `features/resolve-40` | done |
| Index and registry metadata | `index/http-wide`, `index/http-lazy` (loopback sparse HTTP); `index/rtt-*` (the same with a 20 ms round trip) | exit 0, from no lockfile; the lockfile lists every package | `index/load-file-registry` | done; no warm/cold metadata split, because Cabin keeps no metadata cache |
| Workspace and planning | `workspace/metadata`, `workspace/tree`, `workspace/build-noop` | exit 0; no Ninja action | `workspace/load-40`, `manifest/parse-*` | done; no planning bench: in a traced `workspace/build-noop`, `plan` takes 1.8 ms of 79 ms, against 40 ms for toolchain detection and 14 ms for writing `build.ninja` and `compile_commands.json` |
| Clean and incremental builds | `build/clean-*`, `build/touch-source-*`, `build/touch-header-*` | exact compiled-object count from the `.ninja_log` delta | - | done |
| Build-output collection | `gc/new-day-*`, `gc/clean-10k` | exit 0; the planted unused outputs are gone, and live outputs or the usage records survive | - | done |
| Artifacts and file system | `artifact/fetch-cold`, `artifact/fetch-warm`, `package/vendor`, `package/package-*` | exit 0; one cached archive per dependency; every package vendored; the archive and its metadata written | `artifact/extract-zip` | done |
| Tests | `test/run-c`, `test/run-cxx` | exit 0; no Ninja action; the JUnit report lists all 12 tests | - | done |
| Real packages | `ports/*` (`--ports`) | exit 0; every port locked and one cached archive per port; the clean build compiles, the no-op runs no Ninja action | - | done |
| Measurement infrastructure | `trace/noop-cxx-info`, `trace/noop-cxx-debug` | exit 0; no Ninja action | - | done |

C and C++ inputs: the project scenarios come in both languages, the workspace alternates them by
member, the synthetic registry by package, and the ports pair two C libraries with two C++ ones.
The registry consumers and the capture-overhead scenarios are C++ only; the language of the root
package does not change what resolution, fetching, or capture does.

## Measured overhead

`hyperfine` 2.0, 5 warmups and 60 runs per command, interleaved in one session, release builds of
`16e100991` (no instrumentation) and of the change adding trace capture (#2068); macOS 27.2 on
Apple silicon.  Workload: a warm `cabin fetch` of one file-registry dependency (resolution,
lockfile check, artifact cache hit), a command dense with spans.

| Binary and capture | Wall time (mean ± σ) | Trace size |
| --- | --- | --- |
| `16e100991`, no instrumentation | 6.5 ± 0.6 ms | - |
| #2068, capture disabled | 6.4 ± 0.5 ms | - |
| #2068, `CABIN_TRACE_DIR` (`info`) | 7.4 ± 1.1 ms | 3.4 KB, 22 events |
| #2068, `CABIN_TRACE_FILTER=debug` | 7.5 ± 1.1 ms | 7.6 KB, 48 events |

Disabled capture is within noise of the uninstrumented binary.  Enabled capture costs about 1 ms
per invocation: creating the files, starting and joining the writer thread, and the renames.  The
files are made atomic by rename alone, without `fsync`, which cost 14 ms on macOS.

## Investigation: calibrating the comparison

Same machine, one release binary of `main` (5acbd263e) measured under two labels with
`cargo perf run --cabin a=... --cabin b=...` (every scenario but the ports ones, default run
counts): an A/A comparison, in which every reported difference is false.  Two runs, one while
another session compiled alongside (load average 40-65), one quieter (7-25); 93 rows each.

**Question.**  The comparison counted a change past the larger of the metric's minimum effect and
the two sides' relative interquartile range, and claimed that a noisier machine reports fewer,
never false, regressions.  How often does it report a difference between a binary and itself?

Each cell counts the rows reported as `Regressed` or `Improved` with `a`, then `b`, as the
baseline, and after a semicolon the smaller differences reported as `Increased` or `Decreased`.

| Rule | Loaded run | Quieter run |
| --- | --- | --- |
| Previous: past both the minimum effect and the relative interquartile range | 8 / 11 | 3 / 3 |
| Rank test alone, then minimum effect and floor | 13 / 13; 11 / 11 | 2 / 2; 12 / 12 |
| Rank test and past the interquartile range, then minimum effect and floor | 8 / 8; 5 / 5 | 2 / 2; 3 / 3 |

**Why.**  A scenario's runs were two blocks, all of one binary's and then all of the other's (the
harness alternated which went first).  Load that changes between the blocks moves every sample of
one side, which no statistic over the samples can tell from a real change: in the loaded run the
same binary's `build/touch-source-cxx` medians were 983 ms and 580 ms, with 98 of the 100 pairs of
the two sides' samples ordered one way.  Instruction counts drift too: the toolchain probes were polled every
millisecond, so a probe that waited longer retired more instructions, and `gc/clean-10k`'s count,
mostly the kernel's (see "Investigation: build-output collection"), moved 3-7% between the two
sides.

**Change.**  A difference counted as real only when the samples' ranks separated (a Mann-Whitney
test, two-sided 1%) and the medians moved past the larger interquartile range, so the previous
rule's guard against drift stays.  The range is compared in the metric's own units rather than
relative to each side's median, so whether a difference is real no longer depends on which side is
the baseline; the policy's percentages, relative to the baseline, still do.
The policy then counts a real difference only past the minimum effect and an absolute floor above
the quieter run's A/A shifts: 2 ms of wall time (scenarios under 30 ms moved up to 1.05 ms), 1 MiB
of peak RSS (up to 544 KiB), 1 million instructions (up to 0.48 million, but for the load-dependent
3.6 million of `artifact/fetch-cold` and 929 million of `gc/clean-10k`).  A real difference below
the policy is reported as `Increased` or `Decreased` rather than hidden, and a metric only one side
recorded is `Incomparable` rather than dropped.  The false rows that remained were drift, which
no threshold over the samples removes; interleaving the runs (below) reduces it.

**Interleaving.**  Each binary's runs of a scenario were then split into up to 10 blocks, taken in
turns in an order that rotates every round, so drift reaches every side alike.  Calibration: one
release binary of 7325ca99c under two labels, `a` and `b`, beside `r`, the same source with a
3 ms sleep and 3.02 million instructions added at startup (`cargo perf run --cabin a=... --cabin
b=... --cabin r=...`, every scenario but the ports ones, default run counts), compared both ways.
Four runs alternated the block harness and the interleaved one while other sessions loaded the
machine.  Under exchangeable samples the rank test's z has a mean square of 1 and passes the 1%
level in 0.35 of 35 rows; drift between blocks inflates both.  The calibrated harness reversed
the order every round rather than rotating it, which is the same for two binaries but never let
the middle one, `b`, run twice in a row.

| Run | Harness | Load average | A/A rows, counted as above | Mean z², wall / instructions | Rows past 1%, wall / instructions | Median runs |
| --- | --- | --- | --- | --- | --- | ---: |
| 1 | Blocks | 27 -> 48 | 3 / 3; 3 / 3 | 8.1 / 8.8 | 12 / 9 | 26 |
| 2 | Interleaved | 48 -> 29 | 0 / 0; 0 / 0 | 1.2 / 1.9 | 2 / 2 | 17 |
| 3 | Interleaved | 29 -> 114 | 0 / 0; 0 / 0 | 0.8 / 0.9 | 0 / 0 | 18 |
| 4 | Blocks | 114 -> 37 | 0 / 0; 0 / 0 | 5.0 / 4.0 | 3 / 5 | 34 |

Interleaved, the A/A comparisons reported nothing and the rank test came close to its assumption,
but not all the way: in run 2, 2 rows of each metric passed 1%, and `b` retired 12% fewer
instructions than `a` at `artifact/fetch-warm`, a row only the interquartile-range guard kept
`Unchanged`.  A loaded machine can still produce a false difference between interleaved binaries,
only more rarely.  In blocks, the mean square of z ran 4-9 times too large, though those runs also
took more samples, which separates the same drift more readily; again only the interquartile-range
guard kept most of those separations from becoming verdicts.  `gc/clean-10k`'s instructions, which
moved 3-7% before, moved 0.2-0.4% between `a` and `b` interleaved (5.7% and 0.4% in blocks).  Drift
also inflates real differences: at `workspace/build-noop`, `r` retired 17.9 million more
instructions than `a` in run 1, 8.4 million in run 2, and 3.9-4.0 million in runs 3 and 4.  Against
`r`, the instruction rows where the injected count alone passes the policy (27, those under 151
million) were `Regressed` in 23-26 of 27 in every run and both directions; the misses had noise
above the change.  The 3 ms sleep passes the wall-time policy only below 60 ms, and those rows were
`Regressed` in 5 and 2 of 12 and 11 in run 1, 12 and 8 of 15 and 16 in run 4, and 2-6 of 11-13 in
the interleaved runs, which took fewer runs and, in run 3, ran under the heaviest load.  The
interleaved harness sizes a scenario from its check run, which is cold and traced, so it takes fewer
runs than `hyperfine`'s own sizing did; `--runs` buys power where wall time matters.  Each block
costs a `hyperfine` start, about 17 ms.

**Blocks as the unit.**  The interleaved harness still tested the pooled runs as if each were
independent.  Runs within a block share that moment's load: in runs 2 and 3, the medians of one
binary's blocks varied 2.5-2.9 times as much as their runs alone explain (the one-way F statistic,
wall time and instructions), and the two binaries' medians of one round correlated 0.40-0.53.  A
test over pooled runs counts each block's shared load once per run.  A simulation fitted to those
variances (a load each round shares across binaries, one per block, and one per run; 400 trials
each) put the pooled test's rate of calling identical binaries different at 1.8% for 40 runs in 10
blocks and 5.8% for 100, against its nominal 1%.

The test now takes each block's median as one value.  Blocks of one layout pair up round by round,
an exact Wilcoxon signed-rank test of the rounds' differences, and other layouts are compared
unpaired, an exact Mann-Whitney test of the medians; both rank ties by their mean rank.  A row that
does not reach 1% is `Inconclusive` when no arrangement of its blocks could have: below 8 paired
rounds, or 5 blocks a side unpaired (or an uneven split as strong, such as 4 against 6).  In the
simulation, the paired test called identical binaries different in 0.3-2.0% of trials, and found a
shift of one run's standard deviation in 38.5% of trials with 40 runs in 10 blocks and 74.8% in 20,
so blocks are now capped at 20 rather than 10.  The unpaired test over the same 20 blocks found it
in 42.2%.

The interquartile-range guard stays: in the calibration below, the block test without it reported
differences between a binary and itself, and with it none.  For paired blocks the guard now takes
the spread of the rounds' differences, not of either side's runs: a load drifting from round to
round moves both sides alike, so pairing cancels it in the test, and a guard measured on the runs
would still count it as noise.  A uniform 8% regression under a drift of 2% a round reads as
`Regressed` with the rounds' spread and `Unchanged` with the runs'.

Calibration: one release binary of be65ebcf5 under two labels, `a` and `b`, beside `i`, the same
source with 6.0 million instructions added at startup, and `w`, with a 3 ms sleep (`cargo perf run
--cabin a=... --cabin b=... --cabin i=... --cabin w=...`, every scenario but the ports ones, default
run counts), compared with `a`, then `b`, as the baseline.  The cells give each direction; an
injected change is counted only on rows where it passes the policy (6.0 million instructions under
300 million, 3 ms under 60 ms).

| Run | Load average | Median runs | A/A rows reported different | `i` instructions `Regressed` | `w` wall time `Regressed` |
| --- | --- | ---: | --- | --- | --- |
| 1 | 194 -> 45 | 13 | 0 / 0 | 30 of 33 / 31 of 32 | 1 of 12 / 1 of 10 |
| 2 | 45 -> 22 | 17 | 0 / 0 | 31 of 33 / 32 of 33 | 6 of 15 / 5 of 16 |

With the guard on either side's runs, the same data gave 28 of 33, 28 of 32, 30 of 33, and 29 of 33
instruction rows and 0 of 12, 0 of 10, 4 of 15, and 4 of 16 wall rows.  The rounds' spread costs one
row the runs' spread kept: in run 1, under the heaviest load, `gc/clean-10k` retired 2.1% more
instructions in `i` than in `b`, far more than the 6.0 million added, with its rounds moving
together, where the wider spread of its runs, whose kernel work varies with the machine's load (see
"Investigation: build-output collection"), had kept it `Unchanged`.  The wall-time rows of `i` that
passed the policy, such as `startup/version` at +35%, are real: the added instructions take 1-2 ms.
Without a guard, run 2's A/A comparison reported one `Increased` and one `Decreased` row, and in run
1 `i` improved on `a` by 11.6% in `gc/new-day-cxx`'s wall time.  In run 1 the eighth timed run of
`b` in `build/touch-header-c` exited with status 1 under the heaviest load, and the harness keeps no
output of a timed run to say why, so that scenario was `Invalid`.  A 3 ms change to wall time
remains hard to show in 10-17 rounds of a loaded machine; `--runs` buys rounds, at about 17 ms of
`hyperfine` start per block.

## Investigation: no-op build latency

The first question taken through this tooling end to end, on the macOS reference machine (Apple
silicon, macOS 27.2, release builds).

**Question.**  `build/noop-cxx` took 68 ms, yet its trace accounted for well under 1 ms of it: the
root `cabin` span lasted 64.7 ms while `load_workspace_and_patches`, `load_workspace`, and
`resolve_features` together took 0.6 ms.  The time was spent outside the resolution-path spans, in
the build path (toolchain detection, planning, Ninja), which is traced as well now.

**Hypothesis 1: the `xcrun` shims.**  A no-op build still detects the toolchain, running
`--version` on the C++ compiler, the C compiler, and the archiver one after another.  On macOS
`/usr/bin/c++`, `cc`, and `ar` are `xcrun` shims, and `/usr/bin/c++ --version` took 11.4 ms
against 5.0 ms for the `clang++` it resolves to.  A no-op build given the resolved tools directly
used less CPU (user 12.4 ms instead of 18.3 ms) but was no faster (69.1 ms against 66.1 ms, medians
of 40 runs).  Refuted, and less work at the same wall time pointed at waiting.

**Hypothesis 2: the wait between probe checks.**  `run_process_with_timeout` in
`cabin-toolchain/src/detect.rs` checked each probe with `try_wait` and slept up to 10 ms between
checks, so each probe was noticed only on a 10 ms tick after it had exited, and the tick, not the
probe, set the wall time.

**Experiment.**  Two release binaries from one commit, differing only in that interval (10 ms and
1 ms), compared with `cargo perf run --runs 30 --cabin base=... --cabin poll1ms=...`:

| Scenario | 10 ms | 1 ms | Change | Noise | Verdict |
| --- | ---: | ---: | ---: | ---: | --- |
| `inspect/metadata-c` | 51.6 ms | 37.0 ms | -28.3% | 8.4% | Improved |
| `inspect/metadata-cxx` | 52.4 ms | 41.0 ms | -21.8% | 14.7% | Improved |
| `build/noop-c` | 65.9 ms | 50.9 ms | -22.8% | 15.4% | Improved |
| `build/noop-cxx` | 67.8 ms | 54.5 ms | -19.6% | 6.7% | Improved |
| `test/run-c` | 102.6 ms | 75.5 ms | -26.4% | 19.6% | Improved |
| `test/run-cxx` | 90.4 ms | 77.9 ms | -13.8% | 12.4% | Improved |
| `workspace/build-noop` | 84.1 ms | 70.8 ms | -15.7% | 11.3% | Improved |
| `resolve/wide` (control: no detection) | 14.2 ms | 14.4 ms | +1.2% | 4.3% | Unchanged |

Instructions rose 0.2-0.7% with the extra wake-ups, and peak RSS did not move.  `startup/version`,
the other control, first read -18.9% (Improved, noise 13.8%); it was the run's first scenario, and
two reruns with the binaries in swapped order read +7.6% and -0.7%, both Unchanged.  That is why
the harness now alternates which binary runs first and `performance.md` warns about a run's first
scenario.

Hypothesis 1 rerun on both binaries (the `build/noop-cxx` project, medians of 40 runs) shows why
it was refuted: the coarse tick hid the shims' cost.

| Tools | 10 ms interval | 1 ms interval |
| --- | ---: | ---: |
| `xcrun` shims (`/usr/bin/cc`, `c++`, `ar`) | 67.5 ms | 53.3 ms |
| Resolved paths (`--cc`, `--cxx`, `--ar`) | 68.4 ms | 38.0 ms |

**Fix.**  The probe loop slept at most 1 ms between checks (#2070), until it stopped polling (see
"Investigation: toolchain detection").

**Found on the way, not changed:**

- With the fix, the `xcrun` shims are the largest remaining cost of a no-op build on macOS: 15 ms
  of 53 ms, paid on every invocation because detection is not cached (see "Toolchain detection"
  below).
- The lazy HTTP index needs one resolution round per dependency level: `index/http-lazy` takes 8
  rounds for its 8-deep optional chain (see "Registry metadata latency" below).

## Investigation: toolchain detection

Same machine, release builds of `main` (5c5e062f2) and the change, under other load (load average
17-51); medians of 30 runs, phase times from the check run's timings.  The default toolchain is
Apple's `/usr/bin` shims.

**Question.**  Every command that builds runs `--version` on the C++ compiler, the C compiler, and
the archiver, one after another, before it plans anything.  Through an `xcrun` shim each probe
takes 15-20 ms, against 8 ms for the same `clang` run directly: the shim first asks `xcrun` which
tool the selected Xcode or Command Line Tools provides.  Can a build skip or shorten that?

**Caching the result, rejected.**  A cache keyed on the tool's path and file identity is sound only
when that file alone decides what `--version` prints.  Two common setups break that:

- An `xcrun` shim never changes when `xcode-select --switch`, `DEVELOPER_DIR`, `TOOLCHAINS`, or an
  Xcode update changes the compiler behind it.  Keying on that choice means reproducing `xcrun`'s
  selection, and asking `xcrun` (`xcrun -f`, `xcode-select -p`) costs about what the probe does.
- A dynamically linked compiler, such as a distribution's `clang` over `libLLVM` or
  `libclang-cpp`, prints a version that its libraries carry: upgrading them leaves the executable
  untouched.

A stale identity would keep `cabin-toolchain.json` unchanged and skip the rebuild a compiler change
requires, so a fast path that can be stale is not worth its saving.

**Fix.**  The probes run side by side: each mostly waits for its tool to load, so detection now
takes about as long as the slowest probe.

| Scenario | `detect_toolchain` | Wall | CPU (user + system) |
| --- | ---: | ---: | ---: |
| `build/noop-c` | 34.5 -> 14.1 ms | 53.0 -> 32.2 ms | 41.3 -> 43.7 ms |
| `build/noop-cxx` | 34.6 -> 13.9 ms | 57.2 -> 35.4 ms | 45.1 -> 47.0 ms |
| `test/run-c` | 36.2 -> 14.2 ms | 78.4 -> 57.2 ms | 62.4 -> 64.3 ms |
| `test/run-cxx` | 52.2 -> 15.7 ms | 84.6 -> 63.3 ms | 68.1 -> 69.2 ms |
| `workspace/build-noop` | 40.9 -> 21.7 ms | 76.4 -> 54.7 ms | 64.1 -> 65.6 ms |

Instructions are within 1% (the two helper threads), and peak RSS is unchanged.

**Apple SDK discovery.**  Without a sysroot chosen by `--sysroot` or config, a build for an Apple
platform passes the SDK that `SDKROOT` or `xcrun --show-sdk-path` names, but only to Apple Clang,
which detection identifies; so `xcrun` ran after the probes, for about 6 ms (40 runs of
`xcrun --sdk macosx --show-sdk-path`: median 5.8 ms, 5.1-12.2 ms).  It now runs beside them, and
its answer is dropped for any other compiler.  Release builds of 7325ca99c and the change, 60 runs
each, interleaved, under lighter load (load average 7-16):

| Scenario | Wall | Change | Verdict |
| --- | ---: | ---: | --- |
| `inspect/metadata-c` | 28.7 -> 21.0 ms | -26.9% | Improved |
| `inspect/metadata-cxx` | 31.9 -> 23.2 ms | -27.2% | Improved |
| `build/noop-c` | 33.9 -> 26.5 ms | -21.8% | Improved |
| `build/noop-cxx` | 36.5 -> 30.4 ms | -16.8% | Unchanged |
| `test/run-c` | 63.6 -> 57.5 ms | -9.6% | Unchanged |
| `test/run-cxx` | 71.5 -> 71.6 ms | +0.2% | Unchanged |
| `workspace/build-noop` | 54.3 -> 47.9 ms | -11.8% | Improved |
| `startup/version` (control: no detection) | 5.8 -> 5.6 ms | -3.4% | Unchanged |

The `Unchanged` rows moved less than their interquartile range (17-46%).  Instructions rose by up
to 0.7% (`build/noop-cxx`'s 0.5% is `Increased`), and peak RSS moved by at most 0.5%, as much in
the control.  A build with another compiler on macOS now runs `xcrun` for an answer it drops,
beside probes that take longer.  On a Mac without Apple's developer tools, running `xcrun` offers
to install them; such a machine has no SDK to compile against unless one is chosen, and choosing
one (`--sysroot`, the config entry, or `SDKROOT`) skips `xcrun`.

**Waiting for the exit.**  The probe runner noticed a probe's exit by waking every millisecond to
check, so a probe that waited longer retired more instructions.  The thread that reads the probe's
error output now also waits for the exit, and the runner blocks on that thread's message until the
probe's deadline: no wake-ups, and no thread beyond the two output readers.  The deadline still
applies to the exit alone: a probe that exited in time, leaving a process that holds its output
open, is waited for as before.  Release builds of `main` (74e960928) and the change, 60 runs each, interleaved:

| Scenario | Instructions | Change | Verdict |
| --- | ---: | ---: | --- |
| `inspect/metadata-c` | 53.39 -> 52.80 M | -1.1% | Decreased |
| `inspect/metadata-cxx` | 53.33 -> 52.78 M | -1.0% | Decreased |
| `build/noop-c` | 63.53 -> 63.02 M | -0.8% | Unchanged |
| `build/noop-cxx` | 63.68 -> 63.15 M | -0.8% | Decreased |
| `test/run-c` | 94.88 -> 94.18 M | -0.7% | Unchanged |
| `test/run-cxx` | 93.61 -> 92.71 M | -1.0% | Unchanged |
| `startup/version` (control: no detection) | 38.42 -> 38.41 M | -0.0% | Unchanged |

Wall time, CPU time, and peak RSS moved within the noise (other builds shared the machine, and wall
time's noise was 17-65%).  The `Unchanged` rows moved as much as the others, within their
interquartile range.

## Investigation: generated-file writes and manifest size

Same machine, release builds of `main` and the change, three synthetic workspaces of one-header
C++ members (`small`: 7 Ninja edges; `medium`: 212; `large`: 4,052), each under a 120-byte
absolute path; medians of 20 no-op runs, phase times from `CABIN_TRACE_DIR` timings.

**Question.**  A no-op build rewrote `build.ninja` and `compile_commands.json` every time, through
`write_atomic`, which `fsync`s each file and its directory.  What does that cost, and is the
manifest's size worth shrinking?

| Workspace | `build.ninja` | `write_build_ninja` | `write_compile_commands` | No-op wall time |
| --- | ---: | ---: | ---: | ---: |
| `small`, before | 11.7 KB | 4.4 ms | 4.7 ms | 53.0 ms |
| `small`, unchanged files kept | 11.7 KB | 0.1 ms | 0.0 ms | 43.7 ms |
| `medium`, before | 464 KB | 6.2 ms | 5.9 ms | 63.3 ms |
| `medium`, unchanged files kept | 464 KB | 1.1 ms | 0.6 ms | 52.5 ms |
| `large`, before | 11.3 MB | 30.2 ms | 22.9 ms | 209.2 ms |
| `large`, unchanged files kept | 11.3 MB | 24.9 ms | 18.1 ms | 195.7 ms |

**Fix.**  Each file is now compared with its current contents and written only when they differ.
The comparison reads the file on disk a 64 KiB block at a time, after checking its size, so it
never holds a second copy of an 11 MB `build.ninja`.

**Where the rest goes.**  With the files kept, a no-op `large` build spends 22.3 ms rendering
`build.ninja`, 15.9 ms rendering `compile_commands.json`, and 2.6 ms comparing both, 20 MB, with
the files on disk (debug spans, medians of 5 traced runs); Ninja then parses the manifest in 18 ms
and `stat`s for 16 ms.  About 5 ms of the compilation database's time is JSON serialization (from a
temporary span) and most of the rest is `shlex` quoting each argument; it examines every byte, and
a faster quoter would be a replacement for `shlex`.  Neither file depends on the other, so each is
now rendered, compared, and written on a thread of its own.  Alternating 30 no-op runs of each binary, under other load (load average 7-20), with
Cabin's own peak RSS from `getrusage`:

| Workspace | Before | Block-wise comparison | And side-by-side rendering |
| --- | ---: | ---: | ---: |
| `small` | 48.0 ms, 11.9 MiB | 45.7 ms, 12.0 MiB | 44.8 ms, 11.9 MiB |
| `medium` | 52.8 ms, 14.7 MiB | 52.6 ms, 14.1 MiB | 54.2 ms, 14.3 MiB |
| `large` | 204.2 ms, 79.2 MiB | 203.7 ms, 68.3 MiB | 190.3 ms, 69.2 MiB |

At `large`, side-by-side rendering is faster in 25 of 30 paired runs (median -13.4 ms) and peak
RSS stays 10 MiB below the whole-file comparison's; at `small` and `medium` the wall-time
differences are inside the run-to-run spread (interquartile ranges of 10-14% and 4-7%).

**Manifest size, not changed.**  At `large`, Ninja spends 18.5 ms parsing the 11.3 MB manifest
and 15 ms on 8,106 `stat`s (`ninja -d stats`).  Each edge spells its full command, so moving each
target's shared flags into Ninja variables could save at most about half the parse time, under 5%
of the no-op, and would split commands that the driver lowers whole and that
`compile_commands.json` must spell out anyway.

## Investigation: features and packaging revisions in resolution

Same machine, release builds on either side of #2055 (registry features and optional
dependencies) and #2060 (pinned packaging revisions), `main` (a749724e8), and the change;
`cargo perf run --runs 30`, under other load (load average 11-21), so instructions and peak RSS
only.  Instructions are medians in millions, peak RSS medians in MiB:

| Scenario | Before #2055 | After #2055 | After #2060 | `main` | Change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `resolve/wide` instructions | 215.0 | 225.3 | 225.9 | 193.6 | 193.3 |
| `resolve/wide` peak RSS | 17.69 | 18.11 | 19.41 | 17.20 | 16.76 |
| `resolve/deep` peak RSS | 16.58 | 16.80 | 17.81 | 13.20 | 13.03 |
| `index/http-wide` instructions | 180.4 | 191.7 | 192.3 | 193.0 | 193.1 |
| `index/http-wide` peak RSS | 16.16 | 17.11 | 17.91 | 18.18 | 17.59 |

**Question.**  A bisect of the first weekly comparison attributed about 2 MiB of resolution peak
RSS, and part of a 7-14% rise in its wall time on Linux, to these two changes.  Where does the
cost come from, and how much of it do the new semantics need?

**Why.**  #2060 gave each packaging revision an inline `Option<ResolutionMetadata>`, growing
`RevisionMetadata` from 80 to 200 bytes.  Every version keeps its revisions in a `BTreeMap`, whose
first node has room for eleven, so each loaded version's node grew from 1,160 to 2,480 bytes, which
macOS's `malloc` rounds to 1,280 and 2,560: 708 versions at `resolve/wide` account for the whole
906,240-byte rise in peak heap, counted with an allocator that records what `malloc` hands out.
#2055's instructions are the solve's: PubGrub now works over feature nodes, so the provider's
`get_dependencies`, `choose_version`, and `prioritize` callbacks run more often (a sampled profile
of 400 in-process resolves) and its bookkeeping is keyed by the larger node; #2055's 0.25 MB of
extra heap at `resolve/wide` is the root feature requests, the consumer's own entry, and each
package's dependency set, which the solve and the lockfile need.

**Change.**  A revision's own resolution metadata is boxed, so a version's revisions node is
1,248 bytes, again a 1,280-byte allocation on macOS: peak heap falls 640,000 bytes at
`resolve/wide` and 230,400 at `resolve/deep`.  With the file index reading only the documents
resolution needs (below), resolution over a file registry now costs less than before #2055 in
both instructions and memory; over HTTP it keeps #2055's instructions.  Lockfiles, output, and
errors are unchanged on every resolve, update, and `--locked` case of the benchmark registries,
including a respun revision pinned by a lockfile.

**Not changed.**  The rest of #2055's cost is solving with features; testing a version's default
features before its range in the provider saved about 0.7% of `resolve/wide`'s instructions,
which the comparison reports as `Unchanged`.

**`index/http-wide`'s peak RSS.**  The table's 0.27 MiB rise at `index/http-wide` from #2060 to
`main`, while both resolve scenarios fell, is not memory Cabin allocates.  A library injected with
`DYLD_INSERT_LIBRARIES` read the process's peak resident memory at exit, split into file-backed and
anonymous pages (`TASK_VM_INFO`, 30 runs each): the file-backed peak grew from 8.703 to 8.984 MiB,
18 pages of 16 KiB in every run, and anonymous memory did not grow.  Over those commits the
binary's `__TEXT` grew 8.6%, and the file-backed peak moves with changes to Ninja handling that
resolution does not run: #2089 within those commits and #2095 after them moved it by +0.06 and
-0.16 MiB.  So the resolve path most likely touches more pages of a larger binary (the system
libraries did not change, but macOS does not show which image a resident page belongs to without
root), in steps spread over many commits in both directions.  Recovering it would take
whole-binary size or layout work for 1.6% of the peak, under the policy's 1 MiB floor.  The
comparison cannot resolve a step this small on macOS anyway: the allocator's cache of freed large
blocks gives the peak two modes about 0.5 MiB apart, already at #2060 (`MallocLargeCache=0` removes
the upper one), so the mix alone can move a 40-run median by 0.25 MiB or more.  After a749724e8,
anonymous memory rose 0.05-0.14 MiB at each of #2095 and #2099, neither on the resolve path, by a
mechanism not identified.

## Investigation: file-index reads

Same machine, release builds of `main` (2ccb8f623) and the change, under other load (load average
10-39); medians from `hyperfine`.  Peak RSS is the largest process's, which is Cabin's here
because resolution spawns no child; instructions are Cabin's alone.

**Question.**  Resolution over a file index parsed every document under `packages/` before
solving.  How much of a resolve was that, and how does it grow with the registry?

**Experiment.**  One consumer with a fixed 60-package closure, against registries that add
unrelated packages around it (15 runs each); documents parsed counted from `parse_package_entry`
spans:

| Unrelated packages | `main` | Change | Documents parsed |
| ---: | ---: | ---: | ---: |
| 0 | 20.6 ms, 17.9 MiB | 13.5 ms, 12.9 MiB | 188 -> 60 |
| 1,000 | 68.3 ms, 59.1 MiB | 15.6 ms, 13.0 MiB | 1,188 -> 60 |
| 10,000 | 558.4 ms, 429.3 MiB | 12.8 ms, 13.0 MiB | 10,188 -> 60 |

**Fix.**  Resolution reads, by name, only the documents the roots reach through non-optional
edges, with the walk the HTTP index uses, and an optional dependency's document once a feature
enables it.  A damaged document for an unrelated package no longer fails the run; one that
resolution reads still does.  `cargo perf run --runs 30 --cabin main=... --cabin change=...`:

| Scenario | Wall | Peak RSS | Instructions |
| --- | ---: | ---: | ---: |
| `resolve/deep` | -29.4% | -27.8% | -47.0% |
| `resolve/wide` | -9.7% | -11.6% | -14.4% |
| `resolve/update-wide` | -9.7% | -12.4% | -14.7% |
| `artifact/fetch-warm` | -43.4% | -35.0% | -54.2% |
| `index/http-wide`, `index/http-lazy` (controls) | Unchanged | Unchanged | Unchanged |

Every row is past the harness's noise bound.  The file index now needs one more solve round for
each level of optional dependencies features enable, as the HTTP index does: the `index/http-lazy`
registry, whose 8-deep optional chain enables one level at a time, read from disk takes 8 rounds
(1.8 ms) instead of 1 (0.2 ms), and still finishes in 17.5 ms instead of 25.6 ms because it
parses 8 documents instead of 188.

## Investigation: registry metadata latency

Same machine, release builds of `main` (5acbd263e) and the change, against the harness's loopback
registry with each response held for 20 ms, as a nearby registry's round trip would take; medians
of 15 runs.  Wall time is the command's; CPU is user plus system time and instructions are
Cabin's alone, since resolution spawns no child.  Requests are the `fetched` fields of the
`load_http_index` spans, rounds the `rounds` field of `resolve_with_lazy_index`.

**Question.**  The HTTP index fetched one package document at a time over one connection, and a
lazy solve stopped at the first package it lacked.  How much of a resolve against a remote
registry is waiting, and on what?

| Scenario | Requests | Rounds | Wall | CPU | Instructions |
| --- | ---: | ---: | ---: | ---: | ---: |
| `index/rtt-wide`: 100 direct dependencies | 100 | 1 | 2.561 s -> 671.2 ms | 32.0 -> 37.2 ms | +2.9% |
| `index/rtt-deep`: a 60-deep chain | 60 | 1 | 1.540 s -> 1.529 s | 20.9 -> 18.1 ms | Unchanged |
| `index/rtt-lazy`: an 8-deep optional chain | 8 | 8 | 248.8 -> 234.6 ms | 13.1 -> 8.4 ms | Unchanged |
| `index/rtt-lazy-pair`: two halves of that chain | 8 | 7 -> 4 | 236.1 -> 138.2 ms | 9.0 -> 8.3 ms | Unchanged |
| `index/http-wide`: 100 dependencies, no delay | 100 | 1 | 102.4 -> 50.5 ms | 36.7 -> 31.9 ms | +2.2% |

Before the change every request waited its own round trip, so wall time was about 25-30 ms per
request whatever the shape.

**Fix.**  The dependency walk fetches each wave, every package the previous wave's metadata
names, over up to 4 connections at once, and a lazy solve decides the packages it lacks last, so
it reaches every one the round can before it stops and asks for all of them, even one it then
backtracked away from, so the selection stays the one the full index gives.  The waves fetch what
the walk always fetched; a batched round can also fetch a package that only an abandoned branch
reaches.  The cost is the threads and the extra connections: 2-3% more instructions on the wide
walks.

**Not changed.**  A chain's metadata names its next package only once it arrives, so
`index/rtt-deep` and `index/rtt-lazy` still pay one round trip per level.  Removing that needs a
registry that returns a package's dependency closure in one response, a protocol change.

## Investigation: build-output collection

Same machine, release builds of `main` (2e8ea483b) and the change, the change under two labels,
`cargo perf run --filter gc/ --runs 20` with the binaries' runs interleaved block by block, under
other load (load average 46-17).

**Question.**  In the calibration above, `gc/clean-10k`'s instruction count moved 3-7% between
two runs of one binary, though `cabin clean --gc` runs none of the toolchain probes whose polling
made other counts drift.  What does it count?

**Why.**  On macOS the count is Cabin's own in user and kernel mode, and `cabin clean --gc` runs no
probes, so its user-mode work is the same from run to run: what moved was the kernel's.  The
kernel's work dominated the scenario, 1.03 s of system time against 32 ms of user time.  After
removing each output, collection tried to remove its directory and every parent up to the output
directory, which fails while the directory holds anything else, so removing 10,000 outputs from
one directory made a failed `rmdir` call for each but the last.  A script making the same calls on
10,000 files in one directory retired 4.7 billion instructions removing the files alone and 13.1
billion with a failed `rmdir` after each: about 0.84 million per failed `rmdir`, against 0.47
million per removed file.  The cost barely depends on the directory's size: 2,000 failed calls
retired 1.6 billion instructions on a directory of 100 entries and 1.9 billion on one of 10,000.

**Change.**  Collection gathers every directory a removal may have emptied, up to its output
directory, and tries each once every removal is done, deepest first, so failed `rmdir` calls no
longer grow with the number of outputs a directory held.

| `gc/clean-10k` | `main` | Change |
| --- | ---: | ---: |
| Wall time | 1.112 s | 466.7 ms (-58%) |
| System time | 1.033 s | 424.8 ms |
| Instructions | 12,832 M | 4,222 M (-67%) |

Both rows are `Improved`.  `gc/new-day-c` and `gc/new-day-cxx`, which collect one output each, are
`Unchanged`.

**The second mode.**  The count still had two modes about 0.9 billion instructions apart, in both
builds: 4-7 of each label's 20 runs retired that much more kernel work, mostly at different runs for
each label, so interleaving could not line the modes up between the sides.  The mode is the
kernel's, and comes from the fixture rather than from Cabin.  On a release build of `main` at #2171,
an interposer counting each file-system call's instructions (`thread_selfcounts`) put 4.3-4.4
billion of a run's 5.1-5.2 billion in its 10,001 `unlink` calls, about 430,000 each; its 20,004
`lstat` calls took 0.35 billion and everything outside the interposed calls 0.4 billion, the same in
every run.  On macOS, removing or opening a file costs about 0.14 million fewer kernel instructions
while the state its creation or its last open left is current, and removing or renaming any
directory on the machine, on any volume, ends that state for every file.  Creating a directory,
renaming a file, a failed `mkdir` or `rmdir`, `chmod`, `setxattr`, and starting processes do not.  A
C program paid 250,000-300,000 instructions per removal of files it had just created, and
390,000-470,000 when one directory was removed in between, anywhere; `/bin/rm` paid the same, and
HFS+ about as much as APFS, so the work is above the file system.  `--prepare` planted the 10,000
outputs a second or two before the timed run removed them, so a run's removals were cheaper only
when no process on the machine removed or renamed a directory in between.  With every output
reopened just before `cabin clean --gc`, 2 of 8 runs removed 1,480 and 3,221 outputs at the lower
cost; with one `rmdir` after the reopening, none of 8 did.

A real collection removes outputs written long before, which pay the higher cost, so on macOS the
`stale` preparation now removes an empty directory after planting them, and every run starts from
that state whatever else the machine does.  `gc/new-day-c` and `gc/new-day-cxx` plant their one
output the same way.  Under this session's load, other processes removed directories often enough
that the preparation made no difference: planting the outputs with either harness and collecting
them at once, 16 interleaved runs at load average 85-102 removed none at the lower cost, and 38 of
40 interleaved `cargo perf` runs of the changed harness retired 5.0-5.6 billion instructions (two
that took 16-26 s of wall time retired 6.0-6.1 billion).  On a quieter machine the change moves the
scenario's count to its higher mode, so its results are not comparable with ones recorded before.

## Investigation: recording build-output use

Same machine, release builds of `main` (2e8ea483b) and the change, under heavy other load (load
average 40-127).  The load made wall times unusable: an interleaved rerun of `workspace/build-noop`
put the two binaries' medians 5% apart with quartiles 70 ms wide.  The figures below are the
`record_use` span, medians of 30 runs, and instruction counts from `cargo perf run --runs 30`.

**Question.**  Every build records which outputs its plan used, for the collection of unused build
outputs (`profiles.md`, "Unused build outputs").  On a workspace of 200 packages with 19 C sources
each (4,000 products in 200 shards), a no-op build spent 13 ms there and wrote nothing, since every
stamp already held the day.  What was the time spent on?

**Profile.**  Spans inside `record_use` on that workspace: turning 4,000 absolute paths into
build-directory-relative ones and grouping them by output directory took 5 ms, reading the 200
shards 4 ms, parsing them 0.8 ms, comparing stamps 1.1 ms, and naming the shard files 0.5 ms.

**Fix.**  A plan recorded on the current active day is remembered in `.cabin-gc/recorded.json` by a
digest of its products (up to 16 plans a day), and recording it again that day reads the clock and
that file and stops.  Collection spares products used on the current active day, `cabin clean`
removes the file with the catalog, and replacing an unreadable shard forgets every remembered plan.
So the shards already hold what the skipped recording would write.  A plan with another product,
another package selection, or a new active day misses and records in full.  The file is replaced
without a sync, since losing it only costs a recording, so rotating through more plans a day than
it holds stays cheap: 17 plans of 20 products took 0.3 ms a call against `main`'s 0.07 ms, and
16.9 ms when the file was synced.

| `record_use` | `main` | Change |
| --- | ---: | ---: |
| 200-package no-op build | 12.98 ms | 1.05 ms |
| 200-package build, one source touched | 18.49 ms | 1.17 ms |
| `build/noop-cxx` | 0.18 ms | 0.12 ms |
| 200-package build, first of a new active day | 1.66 s | 1.67 s |

Instructions fell 5.7% for `workspace/build-noop`, 1.0-2.1% for `build/noop-c` and `-cxx`, and 2.8%
for `build/touch-source-cxx`.  They rose 4.8% for `gc/new-day-c` and `-cxx`, whose first build of
a day also writes `recorded.json`.

**Concurrency.**  Recording was a read-modify-write of each shard under the build directory's
shared lock, so two commands sharing a build directory (`cabin build` beside `cabin test`, or two
`-p` builds) could each read a shard and write it back, and the later write dropped the other's
stamps: a product used that day could then be collected as old.  Recording now holds an exclusive
lock on `.cabin-gc` for the read-modify-write alone, taken after the check that skips it, so builds
still run side by side and wait on each other only while one records.

**Not changed: the syncs.**  The first build of each active day still rewrites every shard its plan
touches through `atomic-write-file`, which syncs each one: 1.66 s for 200 shards here.  A killed
process loses no finished write, so the syncs matter only across an operating-system crash or a
power loss, and they protect the collection policy, never a build: a collected product is rebuilt
when next needed.

- Without the syncs, such a crash could leave a shard empty, torn, or with its rewrite lost.  One
  that no longer parses authorizes no removal, and the next recording that rewrites it replaces it
  with what that recording writes, so the products only it listed leave the catalog, and are never
  collected, until a later recording lists them again.  One that still parses, with older stamps,
  lets a later collection remove a product used on the current or previous active day.
- Recording finishes before the build uses anything, so the syncs rule out that second case, with
  two gaps.  On macOS, `atomic-write-file` syncs each file with `F_FULLFSYNC` but the directory,
  and with it the rename, with a plain `fsync`, which Apple documents as reaching the drive but not
  necessarily its storage; the next shard's `F_FULLFSYNC` flushes the drive's whole cache, so only
  the last shard's rename stays exposed.  On Windows, `atomic-write-file` renames without
  `MOVEFILE_WRITE_THROUGH` and syncs no directory, so nothing forces any rename to storage.
- `recorded.json` names a plan only after its shards were rewritten, and is removed before an
  unreadable shard is replaced.  Outside those gaps, a copy that is lost, no longer parses, or is
  intact but stale therefore costs at most a recording: each product of a remembered plan is either
  stamped with the day or, when the crash also lost that removal, absent from the catalog, and
  kept.  Unsynced, a copy can also come back torn yet parseable, naming an earlier day's plan under
  the current day, and inside the gaps it can survive a rename it vouches for.  Either way that
  plan's older stamps stay until its first recording on a later active day.

**Deferred: one sync per recording.**  Rewriting the shards in two phases would keep that guarantee,
close the macOS gap, and cost far less there: stage and `fsync` every shard, issue one
`F_BARRIERFSYNC`, rename them all, then sync the directory with `F_FULLFSYNC`, the ordering
`fcntl(2)` documents for this.  A script doing so for 200 small files on APFS took 0.06-0.12 s
against 1.2-1.5 s for `atomic-write-file`'s sequence; the two-phase sequence was not measured
inside Cabin.  On Linux it saves only the directory sync per shard, unmeasured.  On Windows it saves
nothing, and closing the gap there needs the renames written through, also unmeasured.  It would
put per-platform sync code in Cabin in place of `atomic-write-file`, and no test can show what a
power cut leaves behind, so it waits for a maintainer decision.

## Platform verification

| Capability | macOS (arm64) | Linux (x86-64) | Windows (MSVC) |
| --- | --- | --- | --- |
| Trace capture and summary | verified locally | CI test suite | CI test suite |
| `getrusage` memory and CPU in the summary | verified locally | CI test suite (non-zero peak asserted) | unsupported: `process` is `null` (asserted) |
| `cargo perf run`, every scenario | verified locally | Performance workflow: the smoke subset on pull requests, every scenario weekly | Performance workflow: the smoke subset on pull requests, its resets run by `cmd.exe` from a path with a space |
| Criterion benches | verified locally | CI test suite (one iteration each) | CI test suite (one iteration each) |
