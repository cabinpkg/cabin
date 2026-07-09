#!/usr/bin/env bash
#
# Generate publish-conformance fixtures with the IN-TREE cabin binary:
# real `cabin package` archive + canonical-metadata pairs, so the server's
# publish validation is tested against exactly what the client uploads and
# the two sides can never silently drift.
#
#   scripts/gen-fixtures.sh <out-dir>
#
# Produces two pairs in <out-dir>:
#   nodep-0.1.0.tar.gz   / nodep-0.1.0.json    no dependencies
#   withdep-0.2.0.tar.gz / withdep-0.2.0.json  a dependency + a standards block
#
# The frozen pair under tests/fixtures/ is a checked-in copy of the
# `withdep` output; regenerate it with this script if the canonical
# metadata format changes intentionally.

set -euo pipefail

out="${1:?usage: gen-fixtures.sh <out-dir>}"
repo_root="$(cd "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
mkdir -p "$out"

step() { printf '==> %s\n' "$*"; }

step "building the in-tree cabin binary"
cargo build --locked --manifest-path "$repo_root/Cargo.toml" --bin cabin
cabin="$repo_root/target/debug/cabin"

src="$(mktemp -d)"
trap 'rm -rf "$src"' EXIT

step "authoring the fixture packages"
mkdir -p "$src/nodep/src" "$src/withdep/src"

cat >"$src/nodep/cabin.toml" <<'EOF'
[package]
name = "nodep"
version = "0.1.0"
c-standard = "c11"

[target.nodep]
type = "library"
sources = ["src/nodep.c"]
EOF
printf 'int nodep(void) { return 0; }\n' >"$src/nodep/src/nodep.c"

cat >"$src/withdep/cabin.toml" <<'EOF'
[package]
name = "withdep"
version = "0.2.0"
cxx-standard = "c++20"

[dependencies]
nodep = "^0.1"

[target.withdep]
type = "library"
sources = ["src/withdep.cc"]
interface-cxx-standard = "c++17"
EOF
printf 'void withdep() {}\n' >"$src/withdep/src/withdep.cc"

for pkg in nodep withdep; do
  step "packaging $pkg"
  "$cabin" package --manifest-path "$src/$pkg/cabin.toml" --output-dir "$out"
done

step "fixtures written to $out"
ls -l "$out"
