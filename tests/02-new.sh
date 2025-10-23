#!/bin/sh

test_description='Test the new command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin new bin hello_world' '
    test_when_finished "rm -rf hello_world" &&
    "$CABIN" new hello_world 2>actual &&
    (
        test_path_is_dir hello_world &&
        cd hello_world &&
        test_path_is_dir .git &&
        test_path_is_file .gitignore &&
        test_path_is_file cabin.toml &&
        test_path_is_dir src &&
        test_path_is_file src/main.cc
    ) &&
    cat >expected <<-EOF &&
     Created binary (application) \`hello_world\` package
EOF
    test_cmp expected actual
'

test_expect_success 'cabin new lib hello_world' '
    test_when_finished "rm -rf hello_world" &&
    "$CABIN" new --lib hello_world 2>actual &&
    (
        test_path_is_dir hello_world &&
        cd hello_world &&
        test_path_is_dir .git &&
        test_path_is_file .gitignore &&
        test_path_is_file cabin.toml &&
        test_path_is_dir include
    ) &&
    cat >expected <<-EOF &&
     Created library \`hello_world\` package
EOF
    test_cmp expected actual
'

test_expect_success 'cabin new empty' '
    test_must_fail "$CABIN" new 2>actual &&
    cat >expected <<-EOF &&
Error: package name must not be empty
EOF
    test_cmp expected actual
'

test_expect_success 'cabin new existing' '
    test_when_finished "rm -rf existing" &&
    mkdir -p existing &&
    test_must_fail "$CABIN" new existing 2>actual &&
    cat >expected <<-EOF &&
Error: directory \`existing\` already exists
EOF
    test_cmp expected actual
'

test_done
