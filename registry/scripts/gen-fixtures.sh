#!/usr/bin/env bash
#
# Generate publish-conformance fixtures with the IN-TREE cabin binary:
# real `cabin package` archive + canonical-metadata pairs, so the server's
# publish validation is tested against exactly what the client uploads and
# the two sides can never silently drift.
#
#   scripts/gen-fixtures.sh <out-dir>
#
# Produces three pairs in <out-dir> (scoped names, so the filenames carry
# the flattened `<scope>-<name>` stem):
#   smoke-nodep-0.1.0.zip        / smoke-nodep-0.1.0.json         no dependencies
#   smoke-withdep-0.2.0.zip      / smoke-withdep-0.2.0.json       a dependency + standards + links blocks
#   smoke-withupstream-0.3.0.zip / smoke-withupstream-0.3.0.json  a [package.upstream] block
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
name = "smoke/nodep"
version = "0.1.0"
c-standard = "c11"

[target.nodep]
type = "library"
sources = ["src/nodep.c"]
EOF
printf 'int nodep(void) { return 0; }\n' >"$src/nodep/src/nodep.c"

cat >"$src/withdep/cabin.toml" <<'EOF'
[package]
name = "smoke/withdep"
version = "0.2.0"
cxx-standard = "c++20"

[dependencies]
"smoke/nodep" = "^0.1"

[target.withdep]
type = "library"
sources = ["src/withdep.cc"]
interface-cxx-standard = "c++17"
links = "withdep-native"
EOF
printf 'void withdep() {}\n' >"$src/withdep/src/withdep.cc"

mkdir -p "$src/withupstream/src"
cat >"$src/withupstream/cabin.toml" <<'EOF'
[package]
name = "smoke/withupstream"
version = "0.3.0"
c-standard = "c11"

[package.upstream]
url = "https://example.com/withupstream-0.3.0.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
format = "tar.gz"
strip-prefix = "withupstream-0.3.0"
patches = ["patches/0001-fix.patch"]

[[package.upstream.copy]]
from = "scripts/config.h.prebuilt"
to = "config.h"

[target.withupstream]
type = "library"
sources = ["src/withupstream.c"]
EOF
printf 'int withupstream(void) { return 0; }\n' >"$src/withupstream/src/withupstream.c"
# The declared patch must exist in the tree: `cabin package` refuses
# to stage a manifest whose declared patch file is absent, and the
# conformance leg proves the Worker accepts patches-bearing metadata.
mkdir -p "$src/withupstream/patches"
printf -- '--- a/src/withupstream.c\n+++ b/src/withupstream.c\n@@ -1,1 +1,1 @@\n-int withupstream(void) { return 1; }\n+int withupstream(void) { return 0; }\n' \
  >"$src/withupstream/patches/0001-fix.patch"

for pkg in nodep withdep withupstream; do
  step "packaging $pkg"
  "$cabin" package --manifest-path "$src/$pkg/cabin.toml" --output-dir "$out"
done

step "fixtures written to $out"
ls -l "$out"
