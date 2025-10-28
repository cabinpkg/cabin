#!/bin/sh

test_description='cabin build emits Ninja build files'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin build generates Ninja files and uses ninja' '
    OUT=$(mktemp -d) &&
    test_when_finished "rm -rf $OUT" &&
    cd "$OUT" &&

    "$CABIN" new ninja_project &&
    cd ninja_project &&

    "$CABIN" build &&

    test_path_is_file cabin-out/dev/build.ninja &&
    test_path_is_file cabin-out/dev/config.ninja &&
    test_path_is_file cabin-out/dev/rules.ninja &&
    test_path_is_file cabin-out/dev/targets.ninja &&
    test_path_is_file cabin-out/dev/ninja_project &&
    test_path_is_dir cabin-out/dev/ninja_project.d &&
    test ! -e cabin-out/dev/Makefile
'

test_done
