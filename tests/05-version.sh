#!/bin/sh

test_description='Test the version command'

WHEREAMI=$(dirname "$(realpath "$0")")
. $WHEREAMI/setup.sh

test_expect_success 'cabin version' '
    "$CABIN_BIN" version 1>actual &&
    VERSION=$(grep -m1 version "$WHEREAMI"/../cabin.toml | cut -f 2 -d'\''"'\'') &&
    COMMIT_SHORT_HASH=$(git rev-parse --short HEAD) &&
    COMMIT_DATE=$(git show -s --date=format-local:'\''%Y-%m-%d'\'' --format=%cd) &&
    cat >expected <<-EOF &&
cabin $VERSION ($COMMIT_SHORT_HASH $COMMIT_DATE)
EOF
    test_cmp expected actual
'

test_done
