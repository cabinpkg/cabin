#!/bin/sh

export WHEREAMI=$(dirname "$(realpath "$0")")
export CABIN_BIN="${CABIN_BIN:-"$WHEREAMI/../build/cabin"}"
export CABIN_TERM_COLOR='never'

. $WHEREAMI/sharness.sh
