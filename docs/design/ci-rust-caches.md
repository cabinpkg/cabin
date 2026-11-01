# CI Rust caches

`rust.yml` restores its dependency builds from caches that only `main` writes.  A workflow run
restores caches saved on its own ref or on the default branch, and `rust.yml` runs only for pull
requests and merge groups, so before these caches every merge-queue run and every pull request's
first push compiled its dependencies from nothing.  A trusted workflow on `main` now saves one
cache per build configuration, and every `rust.yml` job restores it read-only.

## Ownership

| Shared key | Producer (`main` only) | Restore-only consumers |
| --- | --- | --- |
| `debug-<image>`, one per `build-and-test` image | `rust-cache.yml` `debug` | `rust.yml` `build-and-test` on that image; on `ubuntu-24.04` also `lint-work` and `compiler-detection`; on `windows-2025-vs2026` also `windows-msvc-autodiscovery` |
| `fuzz-ubuntu-24.04` | `rust-cache.yml` `fuzz` | `rust.yml` `fuzz` |

`registry-cache.yml` seeds `registry.yml`'s `registry-build-work` separately and the older way: it
restores the previous entry and saves on top of it, on `ubuntu-latest`.

Each `rust.yml` consumer sets `cache-shared-key`, `cache-bin: false` and `cache-save-if: false`.
Each `rust-cache.yml` job repeats its consumers' toolchain setup and builds what they build.  The
`debug` job runs no tests; the `fuzz` job runs its consumer's `cargo fuzz-check` whole, tests and
seed replays included, so a check that fails on `main` leaves that key unseeded.
`ring`'s build script tracks the C compiler's environment, so two consumers still rebuild `ring`
and the crates that depend on it: `compiler-detection`, which builds under `CC=gcc-14`, and
`windows-msvc-autodiscovery`, which runs without the MSVC environment the seed is built in.

## Keys and invalidation

`setup-rust-toolchain` wraps `Swatinem/rust-cache`, whose key is
`v0-rust-<shared key>-<OS>-<arch>-<environment hash>-<lock hash>`:

- the environment hash covers `rustc -vV` of every installed toolchain and every set variable
  whose name starts with `CARGO`, `CC`, `CFLAGS`, `CXX`, `CMAKE` or `RUST` when the cache step
  runs.  That is the workflow and job `env` plus what `setup-rust-toolchain` exports, which is why
  `rust-cache.yml` repeats `rust.yml`'s workflow `env` and its `setup-rust-toolchain` inputs;
- the lock hash covers `Cargo.lock`'s registry packages, every member's `Cargo.toml` with
  versions blanked, and every `.cargo/config.toml`, `rust-toolchain` and `rust-toolchain.toml`
  file anywhere under a cached workspace, so bumping the `fuzz` workspace's nightly changes every
  key.

GitHub also matches a cache *version* derived from the cached paths, so `cache-bin` and
`cache-workspaces` must agree between a producer and its consumers or the restore misses on an
identical key.

The key names the OS and architecture but not the runner image, and a build script or proc
macro built against a newer glibc does not run on an older one.  The shared key therefore names
the image, and every job that restores a key runs on the image the key names.  That is why
`lint-work` runs on `ubuntu-24.04` rather than `ubuntu-latest`, which is now `ubuntu-26.04`.

A consumer restores the exact key, or else the newest entry with the same environment hash.
After a dependency change lands, consumers restore the previous entry and rebuild only the
crates that changed until the next seed.  A toolchain change alters the environment hash, so
consumers miss entirely until then.  Source-only changes never invalidate a cache, because
`rust-cache` deletes workspace members' artifacts before saving.  The `fuzz` entry still holds the
`crates/` packages the `fuzz` workspace builds by path, which Cargo rebuilds once they change.

## Warming

`rust-cache.yml` runs every six hours and on `workflow_dispatch`.  Its cache step is
`rust-cache` itself, the version `setup-rust-toolchain` wraps, in `lookup-only` mode: when the
key exists the job stops there, and when it does not the job builds from an empty target
directory and saves.  A producer that restored the previous entry and saved on top of it would
carry every superseded crate version forward, since `rust-cache` prunes by crate name, not
version.

Entries are immutable.  Two producer runs that compute the same key build the same lockfile, and
GitHub refuses the second save with a warning.  GitHub evicts the least recently used entries
once a repository exceeds its cache limit, and pull-request caches from other workflows keep this
one over it: when this design landed, the oldest entry in the repository was half an hour old.
An entry that consumers keep restoring stays recent; one evicted anyway comes back at the next
scheduled run.

The schedule, not a `push: main` trigger, keeps `.github/AGENTS.md`'s `push: main` rule intact.
Dependency and toolchain inputs changed in 40 of the 252 commits to `main` in the 30 days before
this design, and most of those changed few crates, so an entry a few hours old costs consumers
little.  After a toolchain bump, dispatch the producer:

```sh
gh workflow run rust-cache.yml --ref main
```

## Trust boundary

Only these runs write the `main` cache scope that pull requests and merge groups restore:

- GitHub grants write access to the default branch's scope only to trusted triggers, `schedule`
  and `workflow_dispatch` among them, and `rust-cache.yml` runs reviewed `main` code with
  `contents: read` and no secrets.  Its jobs skip on any other ref; a branch's own copy of the
  workflow could drop that check, but a run on that branch writes only that branch's scope, which
  `main`, merge groups and pull requests into `main` never read;
- a `pull_request` run saves only to `refs/pull/<n>/merge`, which no other pull request, merge
  group or `main` run restores, and a `merge_group` run saves only to its own queue ref.
  Consumers set `cache-save-if: false` so they skip those saves, but GitHub's scoping, not that
  input, is the boundary;
- no privileged job restores a key `rust-cache.yml` seeds.  `deploy-registry`, ports-publish's
  `publish` and the release build that `release.yml` runs through `dist.yml` restore their own
  keys, none of which starts with `debug-` or `fuzz-`.  `dist.yml` is not seeded for that
  reason.

No cached path holds a credential: `rust-cache` deletes `~/.cargo/credentials.toml` before saving,
`cache-bin: false` keeps installed executables out, and the producer checks out with
`persist-credentials: false`.

What remains trusted is every run that can write `main`'s scope, not only `rust-cache.yml`'s.
Anyone who can land a commit on `main`, or a locked dependency compiled there, can shape an entry;
a consumer runs that same locked dependency's code in the same build anyway, and only unprivileged
test and lint jobs restore the result.

## Diagnosing a miss

The `setup-rust-toolchain` step's *Cache Configuration* group prints the key, the toolchains and
variables in the environment hash, and the files in the lock hash.  Compare the key with what
`main` holds:

```sh
gh cache list --ref refs/heads/main --key v0-rust-debug-
```

- no entry with the consumer's environment hash: a toolchain or `env` difference, or the
  producer has not run since the change; compare the *Environment considered* lists;
- an entry with the same key that still misses: the cached path lists differ (`cache-bin`,
  `cache-workspaces`), or the producer's `rust-cache` version no longer matches the one
  `setup-rust-toolchain` wraps.

To rebuild an entry, delete it with `gh cache delete <key>` and dispatch the producer.

## Cost and benefit

Before this design, every pull-request run of `rust.yml` saved nine entries of about 2.1 GB in all
to a scope no other run reads, and the repository held 12 to 15 GB of caches against its 10 GB
limit.  `main` now holds six entries of about 1.6 GB in all, and `rust.yml` saves nothing.

A warm restore saved about 270 runner-seconds per `rust.yml` run and about 50 seconds on its
slowest job, `build-and-test` on `windows-11-vs2026-arm`; skipping the saves removes another 5 to
20 seconds per job.  A scheduled run that finds every key costs about six runner-minutes, and a
run that rebuilds every entry about 25.

## Not seeded

- `dist.yml`: `release.yml` calls it to build the attested release binaries, so a seeded entry
  would put a shared artifact into a release; it also ran six times in five days.
- `registry.yml`'s `conformance`, `kani` and `tlc`: a warm cache saves under 20 seconds each, off
  the workflow's critical path.
- `perf.yml`'s `smoke` job: it is not a required check and does not run in the merge queue, so a
  cold build delays no merge.
- `ports-publish.yml`'s `dry-run`: building takes under a tenth of the job.
