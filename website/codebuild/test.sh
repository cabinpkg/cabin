#!/bin/bash
set -e

MIX_ENV=test mix do deps.get, deps.compile, compile, test
