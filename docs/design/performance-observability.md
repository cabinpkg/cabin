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
  critical-path figure is derived: Cabin's own work is sequential on one thread, and the parallel
  part (Ninja's actions) is shown as Ninja recorded it, not re-derived.
- Waiting is not CPU.  The trace shows wall time; CPU time comes from `getrusage` and the
  profilers.
- A deterministic counter that changes (an extra compile action, an extra metadata request) is a
  regression signal on its own, however noisy the wall clock is.

## Tool choices

| Need | Choice | Why |
| --- | --- | --- |
| Span facade in domain crates | `tracing` 0.1.44 | Zero-cost when no subscriber exists; the Rust ecosystem's standard; Cargo uses it |
| Subscriber composition | `tracing-subscriber` 0.3 registry, `Targets` filter | No regex dependency; `Targets` directives cover per-crate detail |
| Trace file | `tracing-chrome` 0.7.2, Chrome trace-event JSON | Opens in Perfetto and `chrome://tracing`; Cargo's own choice.  Its writer unwraps I/O errors, so Cabin wraps the writer (see below) |
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
  one thread today; its concurrency is child processes (Ninja's actions, test executables), which
  appear as their own timelines rather than as threads.
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
build's.  If the prefix changed (Ninja recompacts the log at startup once it holds enough stale
entries) the action timeline is reported as unavailable, never as zero.
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
| Driver lowering, Ninja and compile-database emission | `cabin-driver`, `cabin-ninja` | `lower` (trace), `write_build_ninja`, `write_compile_commands` | done |
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
| Index and registry metadata | `index/http-wide`, `index/http-lazy` (loopback sparse HTTP) | exit 0, from no lockfile; the lockfile lists every package | `index/load-file-registry` | done; no warm/cold metadata split, because Cabin keeps no metadata cache |
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

**Why.**  A scenario's runs are two blocks, all of one binary's and then all of the other's (the
harness alternates which goes first).  Load that changes between the blocks moves every sample of
one side, which no statistic over the samples can tell from a real change: in the loaded run the
same binary's `build/touch-source-cxx` medians were 983 ms and 580 ms, with 98 of the 100 pairs of
the two sides' samples ordered one way.  Instruction counts drift too: the toolchain probes are polled every
millisecond, so a probe that waits longer retires more instructions, and `gc/clean-10k`'s count,
mostly the kernel unlinking 10,000 files, moved 3-7% between the two sides.

**Change.**  A difference counts as real only when the samples' ranks separate (a Mann-Whitney
test, two-sided 1%) and the medians move past the larger interquartile range, so the previous
rule's guard against drift stays.  The range is compared in the metric's own units rather than
relative to each side's median, so whether a difference is real no longer depends on which side is
the baseline; the policy's percentages, relative to the baseline, still do.
The policy then counts a real difference only past the minimum effect and an absolute floor above
the quieter run's A/A shifts: 2 ms of wall time (scenarios under 30 ms moved up to 1.05 ms), 1 MiB
of peak RSS (up to 544 KiB), 1 million instructions (up to 0.48 million, but for the load-dependent
3.6 million of `artifact/fetch-cold` and 929 million of `gc/clean-10k`).  A real difference below
the policy is reported as `Increased` or `Decreased` rather than hidden, and a metric only one side
recorded is `Incomparable` rather than dropped.  The false rows that remain are drift, which no
threshold over the samples removes: `--fail-on-regression` can still fail a run of one binary
against itself.

**Not changed.**  Interleaving the two binaries' runs within a scenario, in short alternating
batches, would let the test's assumption hold under drifting load; it changes how the harness
schedules every run.

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

**Fix.**  The probe loop now sleeps at most 1 ms between checks (#2070).

**Found on the way, not changed:**

- With the fix, the `xcrun` shims are the largest remaining cost of a no-op build on macOS: 15 ms
  of 53 ms, paid on every invocation because detection is not cached.
- The lazy HTTP index needs one resolution round per dependency level: `index/http-lazy` takes 8
  rounds for its 8-deep optional chain.

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
The comparison reads the old file back, so at `large` peak RSS rises from 67 to 78 MiB, the size
of that `build.ninja`.

**Manifest size, not changed.**  At `large`, Ninja spends 18.5 ms parsing the 11.3 MB manifest
and 15 ms on 8,106 `stat`s (`ninja -d stats`).  Each edge spells its full command, so moving each
target's shared flags into Ninja variables could save at most about half the parse time, under 5%
of the no-op, and would split commands that the driver lowers whole and that
`compile_commands.json` must spell out anyway.

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
each optional dependency a feature enables, as the HTTP index does: the `index/http-lazy`
registry, whose 8-deep optional chain enables one at a time, read from disk takes 8 rounds
(1.8 ms) instead of 1 (0.2 ms), and still finishes in 17.5 ms instead of 25.6 ms because it
parses 8 documents instead of 188.

## Platform verification

| Capability | macOS (arm64) | Linux (x86-64) | Windows (MSVC) |
| --- | --- | --- | --- |
| Trace capture and summary | verified locally | CI test suite | CI test suite |
| `getrusage` memory and CPU in the summary | verified locally | CI test suite (non-zero peak asserted) | unsupported: `process` is `null` (asserted) |
| `cargo perf run`, every scenario | verified locally | Performance workflow: the smoke subset on pull requests, every scenario weekly | not verified; `--prepare` uses `cmd.exe` quoting |
| Criterion benches | verified locally | CI test suite (one iteration each) | CI test suite (one iteration each) |
