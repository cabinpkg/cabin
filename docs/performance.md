# Performance

How to see where a Cabin command spends its time, measure it reproducibly, and catch regressions:
trace capture, the `cargo perf` benchmarks, and profiling.  The design, the coverage record, and
the platform verification matrix are in
[`design/performance-observability.md`](design/performance-observability.md).

## Capturing a trace

Set `CABIN_TRACE_DIR` to a directory outside the package for any command:

```console
$ CABIN_TRACE_DIR=/tmp/cabin-traces cabin build
   Compiling hello v0.1.0 (/home/me/hello)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.46s
cabin: wrote trace /tmp/cabin-traces/build-1793401315036-85031-0.trace.json
```

Each invocation writes two files whose names start with the subcommand, the Unix time in
milliseconds, the process id, and a sequence number, so concurrent or repeated invocations never
share a file:

- `<stem>.trace.json`: every span the command recorded, in Chrome trace-event format.  Open it in
  [Perfetto](https://ui.perfetto.dev) (or `chrome://tracing`); Perfetto's UI processes the file
  inside the browser.
- `<stem>.timings.json`: the [timings summary](#timings-summary).

The `cabin: wrote trace` line goes to stderr and is suppressed by `--quiet`; stdout, exit status,
and everything the command writes are the same with and without capture.  Keep the directory
outside the package: `cabin package` and `cabin publish` archive every file under the package
root, a trace directory there included.

`CABIN_TRACE_FILTER` selects detail with `tracing-subscriber`
[`Targets`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/targets/struct.Targets.html)
directives:

| Filter | Records |
| --- | --- |
| `info` (default) | Command phases: workspace load, feature resolution, index loading, solving, artifact fetching, toolchain detection, build planning, writing `build.ninja` and `compile_commands.json`, the Ninja run, and test planning and running |
| `debug` | Also each manifest, index document, lockfile read and write, HTTP request, cache lookup, archive extraction, tool `--version` probe, and each target's usage requirements and link closure |
| `trace` | Also each build action's lowering to a command line |
| `cabin_resolver=debug,info` | `debug` for one crate, `info` elsewhere |

A directive without a bare default level (`cabin_resolver=debug` alone) records only that crate,
which drops the root `cabin` span too.

### Reading a trace

Timestamps are microseconds from the start of the capture on a monotonic clock.  A span's length
is its wall-clock lifetime and includes the spans inside it and any time it spent waiting on the
file system, the network, or a child process: a long span is not necessarily CPU work.  Span
arguments carry counts and sizes (`roots`, `packages`, `bytes`, `rounds`) and cache results
(`hit` = 1 or 0).

Two more processes show work that runs beside Cabin, at every filter level; overlapping bars go on
separate rows:

- `ninja`: one bar per action the command's Ninja run performed, named by its output relative to
  the profile's build directory.  It comes from Ninja's `.ninja_log`, cut to the entries this run
  appended, so a no-op build shows none and a failed build shows the actions that finished.
  Ninja times actions from its own start, so the bars sit early by Ninja's startup time.  When
  Ninja rewrote its log during the run (it compacts the log once it holds many stale entries),
  the lane is empty and the summary's `ninja_actions` is `null`.
- `tests`: one bar per test executable `cabin test` ran, named `<package>:<target>`, ending when
  the runner reported it.

## Timings summary

`<stem>.timings.json` is a versioned summary meant for comparing runs.  Its fields keep their
names, units, and meanings for a given `format_version`; which phases appear and their field
names follow the code and can change between commits.

| Field | Meaning |
| --- | --- |
| `format_version` | Schema version, currently `2`: before it, Linux's `self_peak_rss_bytes` could be the spawning process's peak |
| `cabin_version` | The binary's package version |
| `command` | The subcommand, or `cabin` when none was given |
| `outcome` | `success` when the command exited with status 0, else `failure` |
| `trace_complete` | Whether the trace file was written completely |
| `wall_time_us` | From the start of argument handling to the end of the command; excludes process creation |
| `startup_us` | Argument parsing and terminal setup before the capture started, covered by no span |
| `ninja_actions` | The actions the command's Ninja run performed, or `null` when it ran no Ninja or Ninja rewrote its log |
| `phases[]` | Per span name: `target`, `name`, `count`, `total_us` (sum of inclusive lifetimes, which exceeds elapsed time when occurrences overlap), `max_us`, and `fields` (each integer field summed over occurrences) |
| `process` | `getrusage` at exit, or `null` on Windows: `self_peak_rss_bytes`, `self_user_us`, `self_system_us` for Cabin alone (the peak from `/proc/self/status` `VmHWM` on Linux, where `getrusage` starts at the spawning process's peak); `children_user_us`, `children_system_us` summed over the child processes Cabin waited for; `children_largest_peak_rss_bytes`, the largest single child's peak, which on Linux is never below Cabin's own when that child started - child peaks are not simultaneous, so they are never added |

## When capture fails

- An invalid `CABIN_TRACE_FILTER`, or a `CABIN_TRACE_DIR` that cannot be created or written,
  stops the command before it runs.
- A write error during the command (a full disk) leaves `<stem>.trace.json.partial`, prints a
  warning, and records `"trace_complete": false`; the command's own exit status is unchanged.
- An interrupted command (Ctrl-C, a killed process) leaves only the `.partial` file and no
  summary.  Perfetto can usually still open what was flushed.

## Sharing a trace

A trace records span names, counts, sizes, HTTP status codes, the outputs Ninja built (relative to
the profile's build directory, so they name packages, targets, and source files), the tests run,
and, at `debug`, the names of the packages being loaded or fetched.  It records no absolute paths,
URLs, command lines, environment variables, headers, credentials, or response bodies.  The file
names contain only the subcommand, a timestamp, and a process id.  Package, target, and source file
names can still be private: check a trace of a private project before attaching it to a public
issue.

## Benchmarks

`cargo perf` times Cabin commands end to end with [hyperfine](https://github.com/sharkdp/hyperfine)
2.0 or newer (`cargo install hyperfine --locked`); Ninja and a C/C++ compiler are needed as for any
build.  Run it from the repository root:

```console
$ cargo perf list                     # every scenario; [smoke] marks the quick subset
$ cargo perf run                      # build this checkout (release profile), run the suite
$ cargo perf run --smoke              # the quick subset
$ cargo perf run --filter build/ --runs 20
$ cargo perf run --base origin/main   # also build origin/main and compare the two
$ cargo perf run --cabin a=/path/to/cabin --cabin b=/path/to/other/cabin
```

A run writes into `target/perf/runs/<unix time>/` (or `--out`):

- `<label>.json` for each binary: the raw per-run samples (wall, user, and system time, peak
  resident memory, and retired instructions, each with the scope below), the check result, the
  `phases` and `process` blocks of the check run's timings summary, and where the numbers came
  from: `build` (the commit the binary was built from, whether that checkout had uncommitted
  changes, the profile, and `rustc -V`; all `null` for a `--cabin` binary, which the harness did
  not build and does not guess about) and `harness` (the commit and cleanliness of the checkout
  `cargo perf` was built from, which defines the scenarios).  `format_version` is `2`.
- `report.md`: per scenario, the median, the spread, the check run's figures for Cabin alone and
  its largest child, and the three costliest phases.
- `compare.md` and `compare.json` when two binaries ran: each side's label, version, build, and
  harness, and a row per scenario and metric.  `compare` refuses a results file of another
  `format_version`; measure that binary again.

What each number covers depends on the platform, and `report.md` and `compare.md` state it:

| Number | Linux | macOS | Windows |
| --- | --- | --- | --- |
| User and system time | Cabin and every process it waited for, summed | Cabin and every process it waited for, summed | Every process in Cabin's job, summed |
| Peak RSS | The largest single process, never a sum, and never below hyperfine's own peak so far, which Linux carries into a started process | The largest single process, never a sum | Not measured |
| Instructions | Cabin and its children, summed, user mode only | Cabin alone, user and kernel mode | Not measured |
| Check run: Cabin alone | Cabin's own peak RSS and CPU time, from one traced run | Same | Not measured |
| Check run: largest child | The largest child's peak RSS, never below Cabin's own when that child started | The largest child's peak RSS | Not measured |

A metric that is not measured has no samples, never zeros.  A Linux counter that was never
scheduled reads zero instructions, so a zero instruction count in any run drops instructions for
the scenario rather than entering the median.  Zero CPU time is kept: Windows charges CPU time in
clock ticks, so a short run can read zero, and a failed Windows job query, which hyperfine also
writes as zero, cannot be told apart from it.

### Scenarios

| Family | Scenarios | Measures |
| --- | --- | --- |
| Startup | `startup/version` | Process start and argument parsing |
| Inspection | `inspect/metadata-c`, `inspect/metadata-cxx` | Config, workspace, and feature loading without a build |
| Builds | `build/clean-*`, `build/noop-*`, `build/touch-source-*`, `build/touch-header-*` | A full build of a 24-source library and an executable; a no-op; a one-source and a shared-header rebuild.  Each in C and C++ |
| Tests | `test/run-c`, `test/run-cxx` | Running 12 test executables with nothing to rebuild |
| Packaging | `package/package-c`, `package/package-cxx`, `package/vendor` | Archive staging, and vendoring 20 registry packages |
| Workspaces | `workspace/metadata`, `workspace/tree`, `workspace/build-noop` | A 40-member workspace of alternating C and C++ packages, each linking the previous one |
| Build-output collection | `gc/new-day-c`, `gc/new-day-cxx`, `gc/clean-10k` | The first no-op build of an active day, which rewrites the plan's usage records and collects one unused output, in C and C++; `cabin clean --gc` removing 10,000 unused outputs |
| Resolution | `resolve/wide`, `resolve/deep`, `resolve/update-wide` | Solving 100 direct dependencies whose newest versions are all unsatisfiable, a 60-deep chain, and writing the lockfile |
| Registry metadata | `index/http-wide`, `index/http-lazy` | The sparse HTTP index over loopback, and an 8-deep optional-feature chain that needs one resolution round per level |
| Artifacts | `artifact/fetch-cold`, `artifact/fetch-warm` | Downloading, verifying, and extracting 20 archives; re-validating a warm cache |
| Measurement | `trace/noop-cxx-info`, `trace/noop-cxx-debug` | Capture overhead: compare with `build/noop-cxx` |
| Real packages (`--ports`) | `ports/resolve`, `ports/fetch-cold`, `ports/build-clean`, `ports/build-noop` | zlib, libpng, fmt, and spdlog from `ports/`, staged by the port publisher's own pipeline.  The first run downloads their pinned upstream archives |

The inputs are generated into `target/perf/fixtures/` and reused until the harness is rebuilt; the
ports are staged into each run's own directory.  The synthetic registry is published through the
same code path as `cabin publish --registry-dir`, so its index documents and archives are real.
Every scenario and binary works on a private copy.

### What makes a number trustworthy

- **Checked before timed.**  Each scenario first runs once, untimed and traced, and must exit 0
  and do what it claims: a no-op build runs no Ninja action, a one-source touch compiles exactly
  one object, a header touch compiles exactly its includers, a clean build compiles every source,
  a test run rebuilds nothing and its JUnit report lists every test, a resolution locks every
  package, a fetch leaves one archive per dependency in the cache, packaging and vendoring
  write every expected file, and a collection removes the unused outputs planted for it and
  nothing it must keep.  A scenario that fails its check is reported as `FAILED` with the
  reason and no numbers, and `cargo perf run` exits 1, unless only the `--base` revision failed
  it: an older Cabin without a flag the scenario uses leaves that scenario uncompared.  A
  binary from before trace capture is checked and timed all the same, without phases, and one
  from before timings `format_version` 2 without the check run's Cabin-alone and largest-child
  figures.
- **State reset outside the timing.**  hyperfine's `--prepare` removes the build directory,
  cache, or output, or touches a source, before every run.
- **Isolated.**  Every invocation gets a private `CABIN_CONFIG_HOME` and `CABIN_CACHE_HOME` and
  no other inherited `CABIN_*` variable, so no configuration file, build directory, registry
  token, or capture setting of yours reaches it; no `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, or
  `LDFLAGS`; and `--no-compiler-wrapper` for builds, so a ccache hit never passes for a fast
  build.  Your configuration and caches are neither read nor touched.  `CC`, `CXX`, and `AR`
  still choose the toolchain.
- **Recorded.**  Results name the hyperfine and Ninja versions, the OS, architecture, CPU count,
  fixture version, each binary's build, and the harness's own checkout.  A comparison names both
  sides' builds and says so when they ran on different hosts or fixtures, or were measured by
  different harness checkouts, whose scenarios may differ.
- **Interleaved.**  With two binaries, each scenario runs for both back to back, so drift in the
  machine's state affects both alike, and which binary goes first alternates per scenario.  The
  first scenario of a run is the noisiest, measured while the machine is still settling; before
  trusting a change there, rerun that scenario alone with the `--cabin` order swapped.
- **Your machine stays as it is.**  The harness changes no system setting.  Close heavy
  applications and keep a laptop on power; retired instructions move far less than wall time and
  are the metric to trust first on a noisy host.  On macOS they cover Cabin's own process only, so
  there a build's or a test run's instructions leave out Ninja, the compilers, and the tests.

### Baselines and regressions

```console
$ cargo perf run --save-baseline before    # also saved as target/perf/baselines/before-candidate.json
$ git switch my-change
$ cargo perf run --out target/perf/after
$ cargo perf compare before-candidate target/perf/after/candidate.json --out target/perf/after
```

`compare` takes two results files, or names saved with `--save-baseline`.  For every scenario and
metric (wall time, peak memory, instructions) it compares the medians.  A change counts only when
it exceeds both the metric's minimum effect (5% for wall time and peak memory, 2% for
instructions) and the noise, the larger relative interquartile range of the two sides.  Each row
is `Regressed`, `Improved`, or `Unchanged`; `Invalid` when the candidate failed its check or lacks
the scenario; or `NoBaseline` when only the baseline did.  `--fail-on-regression` (on `run` or
`compare`) exits 1 on any `Regressed` or `Invalid` row.

### Micro-benchmarks

`cargo bench -p xtask-bench` runs [Criterion](https://docs.rs/criterion) benchmarks on the same
generated inputs: manifest parsing (C and C++), loading the 40-member workspace, feature
resolution, loading the file index, solving the wide and deep shapes, and safe zip extraction.
Criterion compares against its own saved baselines:
`cargo bench -p xtask-bench -- --save-baseline main`, then `-- --baseline main` after a change.
`cargo nextest run --all-targets` (part of `cargo ci`) runs each benchmark once as a test.

### In CI

The Performance workflow is not a required check.  On a pull request that touches the Rust code it
runs `cargo perf run --smoke` on Linux, so a smoke scenario that fails its check shows up before
merge; a shared runner's numbers flag only gross changes.  Every Monday on `main`, and when
dispatched by hand on any branch, it runs every scenario, ports included, with
`--fail-on-regression` against the dispatched `base` revision or, by default, the same branch as
it was eight days earlier.  Consecutive weekly runs therefore overlap by a day, and a regression
accepted on purpose stops failing once it is older than the base.  Each run copies its reports
(`report.md`, and `compare.md` for a full run) into its summary and keeps them with the results
JSON as an artifact, for 14 days from a pull request and 90 from a full run.  Nothing is uploaded
anywhere else, and the workflow holds no secrets.

## Profiling

Capture a trace first: it says which phase to look at.  Then profile a binary with line tables:

```console
$ cargo build --profile profiling -p cabinpkg
$ cargo perf exec build/noop-cxx --cabin target/profiling/cabin --repeat 200
```

`cargo perf exec` prepares a scenario as `run` does, runs its command `--repeat` times untimed,
and prints the directory it ran in.  Repeat a short command enough to give a sampling profiler
samples.  Wrap it in the profiler:

- [samply](https://github.com/mstange/samply) (macOS, Linux; follows child processes):
  `samply record cargo perf exec build/noop-cxx --cabin target/profiling/cabin --repeat 200`.
- `perf` (Linux): `perf record -g --call-graph dwarf -- cargo perf exec ...`, then `perf report`.
- Instruments (macOS), which records one process: from the directory `cargo perf exec` printed,
  `xcrun xctrace record --template 'Time Profiler' --launch -- <repo>/target/profiling/cabin build
  --no-compiler-wrapper`.  This run uses your own environment, not the harness's private one.

Memory:

- Peak resident memory, Cabin's and its largest child's apart, is in the timings summary's
  `process`; `cargo perf` reports the peak of the largest process per run.
- Allocations: `heaptrack <repo>/target/profiling/cabin build --no-compiler-wrapper` (Linux) or
  Instruments' `Allocations` template (macOS), from the same directory and in your own
  environment.

A compiler's time is not Cabin's: build execution spans wait on Ninja.  To see what a compile
costs, ask the compiler through the standard flag variables, for example `CXXFLAGS=-ftime-trace`
with Clang (one `.json` per object beside it, viewable in Perfetto) or `CFLAGS=-ftime-report` with
GCC.  Changing the flags rebuilds everything once.
