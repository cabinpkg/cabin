# CI Rust caches

`rust.yml` restores its dependency builds from caches that only `main` writes.  A workflow run
restores caches saved on its own ref or on the default branch, and `rust.yml` runs only for pull
requests and merge groups, so before these caches every merge-queue run and every pull request's
first push compiled its dependencies from nothing.  A trusted workflow on `main` now saves one
cache per build configuration, and every `rust.yml` job restores it read-only.

## Ownership

| Shared key | Producer (`main` only) | Restore-only consumers |
| --- | --- | --- |
| `debug-<image>`, one per `build-and-test` image | `rust-cache.yml` `debug` | `rust.yml` `build-and-test` on that image; on `ubuntu-24.04` also `lint-work` and `compiler-detection`; on `windows-2025-vs2026` also `windows-msvc-autodiscovery` and `windows-mingw` |
| `fuzz-ubuntu-24.04` | `rust-cache.yml` `fuzz` | `rust.yml` `fuzz` |

`registry-cache.yml` seeds `registry.yml`'s `registry-build-work` separately and the older way: it
restores the previous entry and saves on top of it, under `registry-build-work-ubuntu-26.04`.

Each `rust.yml` consumer sets `cache-shared-key`, `cache-bin: false` and `cache-save-if: false`.
Each `rust-cache.yml` job repeats its consumers' toolchain setup and builds what they build.  The
`debug` job runs no tests; the `fuzz` job runs its consumer's `cargo fuzz-check` whole, tests and
seed replays included, so a check that fails on `main` leaves that key unseeded.
`ring`'s build script tracks the C compiler's environment, so three consumers still rebuild
`ring` and the crates that depend on it: `compiler-detection`, which builds under `CC=gcc-14`, and
`windows-msvc-autodiscovery` and `windows-mingw`, which run without the MSVC environment the seed
is built in.

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
once a repository exceeds its 10 GB cache limit.  Pull-request saves kept this repository over it
until the jobs in [Jobs without caches](#jobs-without-caches) stopped making them: the least
recently used entry had been used within the hour, and the corpus `fuzz.yml` keeps on `main`
between daily runs was evicted before the next run.  A restore and a lookup both count as an
access, so an entry that consumers keep restoring stays recent; one evicted anyway comes back at
the next scheduled run.

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
- no privileged job restores a key `rust-cache.yml` seeds.  `deploy-registry` and
  ports-publish's `publish` restore their own keys, neither of which starts with `debug-` or
  `fuzz-`, and the release build that `release.yml` runs through `dist.yml` restores no cache.

No cached path holds a credential: `rust-cache` deletes `~/.cargo/credentials.toml` before saving,
`cache-bin: false` keeps installed executables out, and the producer checks out with
`persist-credentials: false`.

What remains trusted is every run that can write `main`'s scope, not only `rust-cache.yml`'s.
Each builds only code from `main`'s history; `perf.yml` refuses a dispatched comparison `base`
outside the run's own history for that reason.  Anyone who can land a commit on `main`, or a
locked dependency compiled there, can shape an entry; a consumer runs that same locked
dependency's code in the same build anyway, and only unprivileged test and lint jobs restore the
result.

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

On the commit that introduced this design, one `rust.yml` run took 1,479 runner-seconds across
its eight Linux and Windows jobs without the seeded entries and 1,138 with them, 23% less, and its
slowest job, `build-and-test` on `windows-11-vs2026-arm`, went from 479 to 397 seconds.  Consumers
also no longer spend 5 to 20 seconds per job saving.  A producer run that rebuilds every entry
took 18 runner-minutes; one that finds its keys stops after the toolchain setup and the lookup.

## Jobs without caches

A pull request's saves serve only later runs of that pull request, and a merge group's only a
rerun of that queue entry.  These jobs set `cache: false` rather than save entries that rarely
hit, cost about as much to save as they return, and evict the entries above.  Pull-request runs
from 2026-10-27 to 2026-11-01, where each job saved on every miss:

| Job | Runs that restored an entry | Median run, cold / warm | Median save |
| --- | --- | --- | --- |
| `perf.yml` `smoke`, `ubuntu-24.04` | 8 of 72 | 228 / 124 s | 8 s |
| `perf.yml` `smoke`, `windows-2025-vs2026` | 9 of 71 | 429 / 265 s | 27 s |
| `registry.yml` `conformance` | 16 of 114 | 89 / 35 s | 7 s |
| `registry.yml` `kani` | 12 of 98 | 112 / 93 s | 6 s |
| `dist.yml` `binaries`, all five targets | 12 of 80 | 119-300 / 82-244 s | 4-22 s |
| `dist.yml` `demo` | 3 of 17 | 243 / 198 s | 6 s |

The merge-group runs of `conformance`, `kani` and `dist.yml` never restored one.  None of these
jobs is seeded on `main` instead:

- `dist.yml`: `release.yml` calls it to build the attested release binaries, which must not
  contain a restored artifact, and it runs only when its packaging inputs change;
- `perf.yml`'s `smoke`: it is not a required check and does not run in the merge queue, so a
  cold build delays no merge;
- `registry.yml`'s `conformance`, `kani` and `tlc-scan`: the merge queue already ran them cold,
  so it loses nothing, and a pull request loses only the occasional hit above;
- `ports-publish.yml`'s `dry-run`: building takes under a tenth of the job.

`website.yml` and `ci.yml` set up Node without an npm cache for the same reason: restoring the
website's cache took about as long as the downloads it saved, and every merge group saved its own
145 MB copy.  Two small pull-request saves remain: `registry-build-work`'s `worker-build` install,
about 4 MB and saved only when `main`'s copy is missing, and `proofs.yml`'s Lean build, about
1 MB, whose restore cuts that job from about three minutes to 20 seconds.
