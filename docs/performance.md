# Performance

How to see where a Cabin command spends its time.  This page covers trace capture; the design,
the coverage record, and the platform verification matrix are in
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
| `info` (default) | Command phases: workspace load, feature resolution, index loading, solving, and artifact fetching |
| `debug` | Also each manifest, index document, lockfile read and write, HTTP request, cache lookup, and archive extraction |
| `cabin_resolver=debug,info` | `debug` for one crate, `info` elsewhere |

A directive without a bare default level (`cabin_resolver=debug` alone) records only that crate,
which drops the root `cabin` span too.

### Reading a trace

Timestamps are microseconds from the start of the capture on a monotonic clock.  A span's length
is its wall-clock lifetime and includes the spans inside it and any time it spent waiting on the
file system, the network, or a child process: a long span is not necessarily CPU work.  Span
arguments carry counts and sizes (`roots`, `packages`, `bytes`, `rounds`) and cache results
(`hit` = 1 or 0).

## Timings summary

`<stem>.timings.json` is a versioned summary meant for comparing runs.  Its fields keep their
names, units, and meanings for a given `format_version`; which phases appear and their field
names follow the code and can change between commits.

| Field | Meaning |
| --- | --- |
| `format_version` | Schema version, currently `1` |
| `cabin_version` | The binary's package version |
| `command` | The subcommand, or `cabin` when none was given |
| `outcome` | `success` when the command exited with status 0, else `failure` |
| `trace_complete` | Whether the trace file was written completely |
| `wall_time_us` | From the start of argument handling to the end of the command; excludes process creation |
| `startup_us` | Argument parsing and terminal setup before the capture started, covered by no span |
| `phases[]` | Per span name: `target`, `name`, `count`, `total_us` (sum of inclusive lifetimes, which exceeds elapsed time when occurrences overlap), `max_us`, and `fields` (each integer field summed over occurrences) |
| `process` | `getrusage` at exit, or `null` on Windows: `self_peak_rss_bytes`, `self_user_us`, `self_system_us` for Cabin alone; `children_user_us`, `children_system_us` summed over the child processes Cabin waited for; `children_largest_peak_rss_bytes`, the largest single child's peak - child peaks are not simultaneous, so they are never added |

## When capture fails

- An invalid `CABIN_TRACE_FILTER`, or a `CABIN_TRACE_DIR` that cannot be created or written,
  stops the command before it runs.
- A write error during the command (a full disk) leaves `<stem>.trace.json.partial`, prints a
  warning, and records `"trace_complete": false`; the command's own exit status is unchanged.
- An interrupted command (Ctrl-C, a killed process) leaves only the `.partial` file and no
  summary.  Perfetto can usually still open what was flushed.

## Sharing a trace

A trace records span names, counts, sizes, HTTP status codes, and, at `debug`, the names of the
packages being loaded or fetched.  It records no file paths,
URLs, command lines, environment variables, headers, credentials, or response bodies.  The file
names contain only the subcommand, a timestamp, and a process id.  Package names can still be
private: check a `debug` trace of a private project before attaching it to a public issue.
