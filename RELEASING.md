# Release Process

For maintainers cutting a release.  Cabin follows [semantic versioning](https://semver.org).

## Update versions

Every crate inherits its version from the workspace, so the version string lives in one file: the
root `Cargo.toml`.  Replace the old version with the new one everywhere it appears there - the
`version` under `[workspace.package]` and the `version` pin on every `cabinpkg`/`cabinpkg-*` entry
under `[workspace.dependencies]`.  Leave the per-crate `crates/*/Cargo.toml` files alone; they use
`version.workspace = true`.

For example, to bump the workspace from `x.y.z` to `a.b.c`, replace every matching workspace version
pin:

```sh
perl -0pi -e 's/version = "x\.y\.z"/version = "a.b.c"/g' Cargo.toml
```

This is only an example bulk edit.  Check the resulting `Cargo.toml` diff before continuing; if it
changed any non-Cabin dependency version, revert that hunk and edit the remaining version pins by
hand.

Confirm nothing was missed, then refresh the lockfile:

```sh
cargo check   # updates Cargo.lock
rg 'x\.y\.z'  # must not show Cabin workspace version pins
```

## Run all required checks

These mirror CI, which runs on `main` and pull requests but not on tags, so they must pass on the
release commit before you tag:

```sh
cargo fmt --all --verbose -- --check
taplo fmt --check
typos
cargo clippy --workspace --all-targets --all-features --locked --verbose -- -D warnings
cargo check --workspace --all-targets --locked --verbose
cargo test --workspace --all-targets --all-features --locked --verbose -- --show-output
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --verbose

# Conventional-commit lint of the release commit (mirrors CI's
# @commitlint/config-conventional gate, which runs `commitlint --last`
# on pushes to `main`). The header must be a valid conventional commit
# and stay <= 100 chars, e.g. `chore: release X.Y.Z`.
npx --yes --package @commitlint/cli --package @commitlint/config-conventional \
  commitlint --extends @commitlint/config-conventional --last --verbose

# This is the real pre-flight gate: it packages and verifies every crate without uploading.
cargo publish --workspace --dry-run --allow-dirty
```

Commit the version bump (including `Cargo.lock`) with a conventional-commit message such as `chore:
release X.Y.Z` - CI's commitlint rejects non-conventional messages - then push and confirm CI is
green on `main`.

## GitHub release

Tags are bare semver with no `v` prefix, matching every prior release:

```sh
git tag X.Y.Z
git push origin X.Y.Z
```

Pushing the tag triggers `.github/workflows/release.yml`, which calls the dist workflow, builds the
supported binary archives, generates SHA-256 checksums, and creates a published GitHub release with
auto-generated notes, the binary archives, checksum files, and `demo.gif`.

## crates.io release

Publish the whole workspace:

```sh
cargo publish --workspace
```

This publishes all crates (`cabinpkg` and the `cabinpkg-*` libraries), ordered automatically by
their dependency graph - no per-crate commands needed.
