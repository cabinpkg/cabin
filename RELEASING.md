# Release Process

For maintainers cutting a release.  Cabin follows [semantic versioning](https://semver.org).

## Update versions

Every release crate inherits its version from the root workspace, so the version string lives in the
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
# updates Cargo.lock
cargo check
cargo update --workspace --manifest-path registry/Cargo.toml
cargo update --workspace --manifest-path fuzz/Cargo.toml

rg 'x\.y\.z'  # must not show Cabin workspace version pins
```

## Run all required checks

Run `cargo ci` on the committed version-bump branch before merging. It checks changes relative to
`main`, so running it on `main` after the merge would skip the checks:

```sh
cargo ci

# This is the real pre-flight gate: it packages and verifies every crate without uploading.
cargo publish --workspace --dry-run --allow-dirty
```

Commit the version bump (including `Cargo.lock`) with a conventional-commit message such as `chore:
release X.Y.Z` - CI's commitlint rejects non-conventional messages - then open a pull request and
land it through the merge queue like any other change.  CI validates the pull request and its
merge-group run; workspace validation does not rerun after the merge, so once the queue merges it
the release commit on `main` is the validated one.  (The main push still runs CodeQL, and the
version bump matches the registry filter, so it also deploys the registry Worker - both are normal.)

## GitHub release

Tags are bare semver with no `v` prefix, matching every prior release, and must equal the
workspace version: the dist workflow refuses any other tag, and the release workflow refuses a tag
on a commit `main` does not carry, such as the release commit from before its squash merge.  Tag
the squash-merged release commit on updated `main`:

```sh
git switch main && git pull
git tag X.Y.Z
git push origin X.Y.Z
```

Pushing the tag triggers `.github/workflows/release.yml`, which calls the dist workflow, builds the
supported binary archives, runs each archive's binary on its own platform, attests them with GitHub
artifact attestations, and creates a published GitHub release with auto-generated notes, the
binary archives, and `demo.gif`.  Then check that a published archive's attestation verifies:

```sh
gh release download X.Y.Z --repo cabinpkg/cabin --pattern 'cabin-X.Y.Z-x86_64-unknown-linux-gnu.tar.xz'
gh attestation verify cabin-X.Y.Z-x86_64-unknown-linux-gnu.tar.xz --repo cabinpkg/cabin
```

## crates.io release

Publish the whole workspace:

```sh
cargo publish --workspace
```

This publishes all crates (`cabinpkg` and the `cabinpkg-*` libraries), ordered automatically by
their dependency graph - no per-crate commands needed.
