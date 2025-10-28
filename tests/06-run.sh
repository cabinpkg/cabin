#!/bin/sh

test_description='Test the run command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin run hello_world' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd $OUT &&
    "$CABIN" new hello_world &&
    cd hello_world &&
    "$CABIN" run 1>stdout 2>stderr &&
    cat stdout stderr &&
    (
        test_path_is_dir cabin-out &&
        test_path_is_dir cabin-out/dev &&
        test -x cabin-out/dev/hello_world
    ) &&
    (
        TIME=$(cat stderr | grep Finished | grep -o '\''[0-9]\+\.[0-9]\+'\'') &&
        cat >stderr_exp <<-EOF &&
   Compiling hello_world v0.1.0 ($(realpath $OUT)/hello_world)
    Finished \`dev\` profile [unoptimized + debuginfo] target(s) in ${TIME}s
     Running \`cabin-out/dev/hello_world\`
EOF
        test_cmp stderr_exp stderr
    ) &&
    (
        cat >stdout_exp <<-EOF &&
Hello, world!
EOF
        test_cmp stdout_exp stdout
    )
'

test_done
