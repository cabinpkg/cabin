# Performance observability: design and coverage

This is the design and the working coverage record for measuring Cabin's own performance: the
trace capture, the benchmark suite, the profiling workflows, baseline comparison, and CI.  The
contributor-facing guide is [`performance.md`](performance.md).  Update the tables below in the
same change as the work they describe.

Baseline: `main` at `16e100991` (2026-10-30).

## What is measured, and by what

Five kinds of measurement answer different questions and are never substituted for each other.

| Kind | Question | Source | Scope and clock |
| --- | --- | --- | --- |
| End-to-end time | How long does the user wait? | `hyperfine` runs of a release `cabin` binary | Whole process, wall clock, from spawn to exit |
| Internal phases | Where inside Cabin did that time go? | `tracing` spans, captured with `CABIN_TRACE_DIR` | Cabin's thread(s), monotonic clock, inclusive span lifetimes; time spent waiting on I/O or a child process counts toward the span that waits |
| CPU and stacks | What code burns CPU? | Sampling profilers (`samply`, `perf`, Instruments) on a `profiling`-profile binary | Sampled call stacks; CPU time, not waiting |
| Memory | How much memory, and where allocated? | `getrusage` in the timings summary, `hyperfine` peak RSS, heap profilers | Cabin alone vs. its child processes, stated per number; peaks are never summed |
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
invocation: Cabin records the log's length and a digest of its contents before starting Ninja, and
reads only the entries appended after that point.  A no-op build therefore reports zero actions
rather than the previous build's.  If the prefix changed (Ninja recompacts the log at startup once
it holds enough stale entries) the action timeline is reported as unavailable, never as zero.
Ninja writes an entry only for an action that succeeded, so a failed build lists its completed
actions and says the failed one is absent.  Several outputs of one command share one entry's
start, end, and command hash and count as one action.  Ninja's times are milliseconds from its own
build start, a clock Cabin cannot read; actions are placed at Cabin's spawn instant, and the gap
between Ninja's spawn and its first action (manifest load, startup) is the stated alignment
uncertainty.  Status: designed; lands with the build-path spans after roadmap stage 04's PR B.

## Delivery

| PR | Content | Depends on | Status |
| --- | --- | --- | --- |
| 1 | Capture (`trace.rs`), timings summary, spans in workspace, manifest, config, feature, resolver, index, index-http, artifact, lockfile, package, vendor, system-deps, and the resolve/fetch orchestration; this record and `performance.md` | - | in progress |
| 2 | `xtask-bench`: fixtures, Criterion benches, `hyperfine` CLI scenarios, baselines, comparison reports; profiling profile and recipes | 1 | not started |
| 3 | Build-path spans (toolchain detection, planning, lowering, Ninja emission), the Ninja action timeline, test-run timeline | 1; stage 04 PR B; #2065 | blocked on stage 04 PR B |
| 4 | Performance workflow: smoke subset on pull requests, full comparison on schedule and on demand, artifact retention | 2 | not started |

Stage 04 (build and target contexts) reshapes `cli/build_prep.rs`, `cli/ninja.rs`, the planner,
the driver, and toolchain detection; PR 3 lands on top of it by agreement with that stage's
owner.  Stage 11's JUnit change (#2065) adds per-test durations that the test timeline reuses.

## Coverage

Trace coverage per workflow.  `info` spans mark phases; `debug` spans mark per-item steps.

| Workflow / subsystem | Owner | Spans | Status |
| --- | --- | --- | --- |
| Command startup | `cabin` | root `cabin` span (`command`, `startup_us`); summary `startup_us` | done |
| Config discovery | `cabin-config` | `discover_config_files` (debug) | done |
| Workspace and manifest load | `cabin-workspace`, `cabin-manifest`, `cabin` | `load_workspace_and_patches`, `load_workspace`, `discover_workspace_root`, `load_manifest` (debug), `resolve_package_selection` (debug) | done |
| Feature resolution | `cabin-feature` | `resolve_features` (`roots`, `packages`) | done |
| File index | `cabin-index` | `load_file_index`, `parse_package_entry` (debug, `bytes`) | done |
| HTTP index | `cabin-index-http` | `open_http_index`, `load_http_index` (`roots`, `fetched`), `http_get` (debug: `package`, `status`, `bytes`) | done |
| Dependency solving and lazy metadata | `cabin-resolver`, `cabin` | `resolve_with_lazy_index` (`rounds`), `resolve` / `resolve_packages` (`roots`, `index_packages`), `held_back_report`, `select_locked_revisions` (debug) | done |
| Lockfile | `cabin-lockfile` | `read_lockfile`, `write_lockfile` (debug) | done |
| Artifact cache, checksum, extraction | `cabin-artifact`, `cabin` | `run_artifact_pipeline`, `build_fetch_plan` (HTTP downloads), `fetch_artifacts` (`packages`), `fetch_one`, `ensure_archive` / `ensure_source` (`hit`), `extract_zip` (debug) | done |
| Package and vendor | `cabin-package`, `cabin-vendor` | `stage_package`, `collect_package_files`, `build_zip`, `write_staged`, `materialize_vendor` | done |
| System dependencies | `cabin-system-deps` | `probe_system_dependency` (debug) | done |
| Toolchain detection and probes | `cabin-toolchain` | - | PR 3 |
| Build planning, usage requirements, link closure | `cabin-build` | - | PR 3 |
| Driver lowering, Ninja and compile-database emission | `cabin-driver`, `cabin-ninja` | - | PR 3 |
| Build execution | `cabin`, Ninja | Ninja action timeline | PR 3 |
| Test planning and execution | `cabin-test` | - | PR 3 |

Measurement-infrastructure evidence for PR 1:

| Property | Check |
| --- | --- |
| Stdout unchanged by capture | `trace_capture::traced_metadata_keeps_stdout_byte_identical` |
| Failure keeps its diagnostic and finishes the capture | `trace_capture::a_failing_command_keeps_its_error_and_finishes_the_capture` |
| Malformed filter / unusable directory refuse to run | `trace_capture::malformed_filter_refuses_to_run_the_command`, `an_unusable_trace_dir_refuses_to_run_the_command`, unit `an_invalid_filter_is_refused_before_any_file_exists` |
| Concurrent processes never share a file | `trace_capture::concurrent_invocations_write_separate_complete_captures` |
| Repeated in-process invocations | unit `repeated_sessions_in_one_thread_each_capture_their_own_command` |
| Write failure is reported, not a panic or a false "complete" | unit `a_write_failure_leaves_a_partial_trace_and_says_so`, `checked_writer_keeps_the_first_error_and_swallows_the_rest` |

## Measured overhead

`hyperfine` 2.0, 5 warmups and 60 runs per command, interleaved in one session, release builds of
`main` (`16e100991`, no instrumentation) and this branch; macOS 27.2 on Apple silicon.  Workload:
a warm `cabin fetch` of one file-registry dependency (resolution, lockfile check, artifact cache
hit), the command most covered by PR 1's spans.

| Binary and capture | Wall time (mean ± σ) | Trace size |
| --- | --- | --- |
| `main`, no instrumentation | 6.5 ± 0.6 ms | - |
| branch, capture disabled | 6.4 ± 0.5 ms | - |
| branch, `CABIN_TRACE_DIR` (`info`) | 7.4 ± 1.1 ms | 3.4 KB, 22 events |
| branch, `CABIN_TRACE_FILTER=debug` | 7.5 ± 1.1 ms | 7.6 KB, 48 events |

Disabled capture is within noise of the uninstrumented binary.  Enabled capture costs about 1 ms
per invocation: creating the files, starting and joining the writer thread, and the renames.  An
earlier draft that `fsync`ed both files cost 14 ms on macOS; the files are now written without
`fsync` and made atomic by rename alone.

## Platform verification

| Capability | macOS (arm64) | Linux (x86-64) | Windows (MSVC) |
| --- | --- | --- | --- |
| Trace capture and summary | verified locally | CI test suite | CI test suite |
| `getrusage` memory and CPU in the summary | verified locally | CI test suite (non-zero peak asserted) | unsupported: `process` is `null` (asserted) |
