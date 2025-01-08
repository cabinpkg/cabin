#!/bin/sh

test_description='Test the fmt command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

command -v clang-format >/dev/null && test_set_prereq CLANG_FORMAT

if ! test_have_prereq CLANG_FORMAT; then
    test_expect_success 'cabin fmt without clang-format' '
        test_when_finished "rm -rf pkg" &&
        "$CABIN_BIN" new pkg &&
        cd pkg &&
        (
            test_must_fail "$CABIN_BIN" fmt 2>actual &&
            cat >expected <<-EOF &&
Error: fmt command requires clang-format; try installing it by:
  apt/brew install clang-format
Error: '\''cabin fmt'\'' failed with exit code \`1\`
EOF
            test_cmp expected actual
        )
    '

    # Skip the rest of the tests
    test_done
fi

test_expect_success 'cabin fmt' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN_BIN" new pkg &&
    cd pkg &&
    (
        echo "int main(){}" >src/main.cc &&
        md5sum src/main.cc >before &&
        "$CABIN_BIN" fmt 2>actual &&
        md5sum src/main.cc >after &&
        test_must_fail test_cmp before after &&
        cat >expected <<-EOF &&
  Formatting pkg
EOF
        test_cmp expected actual
    )
'

test_expect_success 'cabin fmt no targets' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN_BIN" new pkg &&
    cd pkg &&
    (
        rm src/main.cc &&
        "$CABIN_BIN" fmt 2>actual &&
        cat >expected <<-EOF &&
Warning: no files to format
EOF
        test_cmp expected actual
    )
'

test_expect_success 'cabin fmt without manifest' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN_BIN" new pkg &&
    cd pkg &&
    (
        rm cabin.toml &&
        test_must_fail "$CABIN_BIN" fmt 2>actual &&
        cat >expected <<-EOF &&
Error: cabin.toml not find in \`$(realpath $OUT)/pkg\` and its parents
EOF
        test_cmp expected actual
    )
'

test_expect_success 'cabin fmt without name in manifest' '
    echo $SHARNESS_TEST_OUTDIR &&
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN_BIN" new pkg &&
    cd pkg &&
    (
        echo "[package]" >cabin.toml &&
        test_must_fail "$CABIN_BIN" fmt 2>actual &&
        cat >expected <<-EOF &&
Error: [error] toml::value::at: key "name" not found
 --> $(realpath $OUT)/pkg/cabin.toml
   |
 1 | [package]
   | ^^^^^^^^^-- in this table

EOF
        test_cmp expected actual
    )
'

test_done
