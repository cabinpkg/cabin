#!/bin/sh

export WHEREAMI=$(dirname "$(realpath "$0")")
export CABIN="${CABIN:-"$WHEREAMI/../build/cabin"}"

SAVETZ=${TZ:-UTC}

. $WHEREAMI/sharness.sh

export TZ=$SAVETZ
