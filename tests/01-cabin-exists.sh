#!/bin/sh

test_description='Check if the cabin binary exists'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'The cabin binary exists' '
    test -x "$CABIN"
'

test_done
