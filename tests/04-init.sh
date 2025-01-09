#!/bin/sh

test_description='Test the init command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin init' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    mkdir $OUT/pkg &&
    cd $OUT/pkg &&
    "$CABIN_BIN" init 2>actual &&
    cat >expected <<-EOF &&
     Created binary (application) \`pkg\` package
EOF
    test_cmp expected actual &&
    test -f cabin.toml
'

test_expect_success 'cabin init existing' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    mkdir $OUT/pkg &&
    cd $OUT/pkg &&
    "$CABIN_BIN" init 2>actual &&
    cat >expected <<-EOF &&
     Created binary (application) \`pkg\` package
EOF
    test_cmp expected actual &&
    test -f cabin.toml
    test_must_fail "$CABIN_BIN" init 2>actual &&
    cat >expected <<-EOF &&
Error: cannot initialize an existing cabin package
EOF
    test_cmp expected actual &&
    test -f cabin.toml
'

test_done
