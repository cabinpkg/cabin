#!/bin/sh

test_description='Test the remove command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin remove tbb mydep toml11' '
    test_when_finished "rm -rf remove_test" &&
    "$CABIN" new remove_test &&
    cd remove_test &&
    echo "[dependencies]" >> cabin.toml &&
    echo "tbb = {}" >> cabin.toml &&
    echo "toml11 = {}" >> cabin.toml &&
    (
        "$CABIN" remove tbb mydep toml11 2>actual &&
        ! grep -q "tbb" cabin.toml &&
        ! grep -q "toml11"  cabin.toml
    ) &&
    cat >expected <<-EOF &&
Warning: Dependency \`mydep\` not found in $WHEREAMI/trash directory.07-remove.sh/remove_test/cabin.toml
     Removed tbb, toml11 from $WHEREAMI/trash directory.07-remove.sh/remove_test/cabin.toml
EOF
    test_cmp expected actual
'
test_done
