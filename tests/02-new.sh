#!/bin/sh

WHEREAMI=$(dirname "$(realpath "$0")")
export CABIN_TERM_COLOR='never'

test_description='Test the new command'

. $WHEREAMI/sharness.sh

test_expect_success 'cabin new bin hello_world' '
    test_when_finished "rm -rf hello_world" &&
    "$WHEREAMI"/../build/cabin new hello_world 2>actual &&
    (
        test -d hello_world &&
        cd hello_world &&
        test -d .git &&
        test -f .gitignore &&
        test -f cabin.toml &&
        test -d src &&
        test -f src/main.cc
    ) &&
    cat >expected <<-EOF &&
     Created binary (application) \`hello_world\` package
EOF
    test_cmp expected actual
'

test_expect_success 'cabin new lib hello_world' '
    test_when_finished "rm -rf hello_world" &&
    "$WHEREAMI"/../build/cabin new --lib hello_world 2>actual &&
    (
        test -d hello_world &&
        cd hello_world &&
        test -d .git &&
        test -f .gitignore &&
        test -f cabin.toml &&
        test -d include
    ) &&
    cat >expected <<-EOF &&
     Created library \`hello_world\` package
EOF
    test_cmp expected actual
'

test_expect_success 'cabin new empty' '
    test_must_fail "$WHEREAMI"/../build/cabin new 2>actual &&
    cat >expected <<-EOF &&
Error: package name must not be empty: \`\`
Error: '\''cabin new'\'' failed with exit code \`1\`
EOF
    test_cmp expected actual
'

test_expect_success 'cabin new existing' '
    test_when_finished "rm -rf existing" &&
    mkdir -p existing &&
    test_must_fail "$WHEREAMI"/../build/cabin new existing 2>actual &&
    cat >expected <<-EOF &&
Error: directory \`existing\` already exists
Error: '\''cabin new'\'' failed with exit code \`1\`
EOF
    test_cmp expected actual
'

test_done
