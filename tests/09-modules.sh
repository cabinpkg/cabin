#!/bin/sh

test_description='Test the modules functionality'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'check compiler supports modules' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    if "$CABIN" new --modules modules_test_support 2>/dev/null; then
        test_set_prereq MODULES_SUPPORTED
    else
        skip_all="Compiler does not support C++23 modules (GCC 14+ or Clang 17+ required)"
        test_done
    fi
'

test_expect_success MODULES_SUPPORTED 'cabin new bin with modules' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new --modules hello_modules 2>actual &&
    (
        test -d hello_modules &&
        cd hello_modules &&
        test -d .git &&
        test -f .gitignore &&
        test -f cabin.toml &&
        test -d src &&
        test -f src/main.cc &&
        grep "modules = true" cabin.toml &&
        grep "edition = \"23\"" cabin.toml &&
        grep "import std;" src/main.cc
    ) &&
    cat >expected <<-EOF &&
     Created binary (application) \`hello_modules\` package
EOF
    test_cmp expected actual
'

test_expect_success MODULES_SUPPORTED 'cabin new lib with modules' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new --lib --modules hello_modules_lib 2>actual &&
    (
        test -d hello_modules_lib &&
        cd hello_modules_lib &&
        test -d .git &&
        test -d src &&
        test -f src/hello_modules_lib.cppm &&
        grep "export module hello_modules_lib;" src/hello_modules_lib.cppm
    ) &&
    cat >expected <<-EOF &&
     Created library \`hello_modules_lib\` package
EOF
    test_cmp expected actual
'

test_expect_success MODULES_SUPPORTED 'cabin new with short modules flag -m' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new -m hello_m 2>actual &&
    (
        test -d hello_m &&
        cd hello_m &&
        grep "modules = true" cabin.toml &&
        grep "edition = \"23\"" cabin.toml
    ) &&
    cat >expected <<-EOF &&
     Created binary (application) \`hello_m\` package
EOF
    test_cmp expected actual
'

test_expect_success MODULES_SUPPORTED 'modules library structure is correct' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new --lib --modules build_test_lib_modules &&
    cd build_test_lib_modules &&
    test -f src/build_test_lib_modules.cppm &&
    grep "export module build_test_lib_modules;" src/build_test_lib_modules.cppm &&
    grep "export namespace build_test_lib_modules" src/build_test_lib_modules.cppm
'

test_done
