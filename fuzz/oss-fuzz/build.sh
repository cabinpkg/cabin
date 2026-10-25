#!/bin/bash -eu
cd "$SRC/cabin/fuzz"
cargo fuzz build -O --debug-assertions
targets=$(cargo fuzz list)
for target in $targets; do
  # inspect_zip_wrong_digest runs on inspect_zip's seeds and dictionary.
  corpus=${target%_wrong_digest}
  cp "target/x86_64-unknown-linux-gnu/release/$target" "$OUT/"
  zip -jq "$OUT/${target}_seed_corpus.zip" "seeds/$corpus"/*
  cp "dicts/$corpus.dict" "$OUT/$target.dict"
done
