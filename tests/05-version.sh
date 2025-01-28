#!/bin/sh

test_description='Test the version command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin version' '
    VERSION=$(grep -m1 version "$WHEREAMI"/../cabin.toml | cut -f 2 -d'\''"'\'') &&
    COMMIT_SHORT_HASH=$(git rev-parse --short=8 HEAD) &&
    COMMIT_DATE=$(git show -s --date=format-local:%Y-%m-%d --format=%cd) &&
    "$CABIN_BIN" version 1>actual &&
    cat >expected <<-EOF &&
cabin $VERSION ($COMMIT_SHORT_HASH $COMMIT_DATE)
EOF
    test_cmp expected actual
'

test_expect_success 'cabin verbose version' '
    "$CABIN_BIN" -vV | grep compiler
'

test_done
