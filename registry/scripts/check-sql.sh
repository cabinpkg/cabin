#!/usr/bin/env bash
#
# Every SQL statement the Worker executes must live in src/sql.rs, where
# tests/sql_validation.rs prepares it against the real migrated schema
# (docs/architecture.md, "Why no ORM"). This guard keeps executed SQL
# from growing outside that module: the two literal patterns below must
# never match, every prepare() call must name a sql:: const, and D1's
# unprepared escape hatch (exec) is rejected outright. The last two are
# a lexical scan (scripts/check-sql.pl) rather than a line grep, so no
# comment can hide a call or fake one and no multi-line spelling slips
# through; tests/check_sql_guard.rs pins every case.
#
# ponytail: a lexical scan, not a Rust parser, so it has no receiver
# types - an unrelated `prepare`/`exec` method on some other receiver
# would be flagged too (loudly, at the call site: rename it or teach the
# scanner), and a call assembled by a macro would pass. It is a
# regression tripwire for ordinary contributions - deliberate evasion is
# a code-review question, and the statements still have to work - so
# make it syntax-aware only if that stops holding.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

fail=0
# The two commissioned literal patterns, on the source as written.
if grep -rn 'prepare("' src/; then
  fail=1
fi
if grep -rn 'prepare(&format!' src/; then
  fail=1
fi
if ! find src -name '*.rs' -print0 | xargs -0 perl scripts/check-sql.pl; then
  fail=1
fi

if [ "${fail}" -ne 0 ]; then
  echo "error: executed SQL outside src/sql.rs; route the statements above through sql:: consts" >&2
  exit 1
fi
