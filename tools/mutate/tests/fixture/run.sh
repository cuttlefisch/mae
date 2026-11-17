#!/bin/sh
# The fixture's runner: records each call, lets a test add behaviour through an
# optional hook.sh (sourced, so it sees $1 and MUTATE_ID), then runs cargo.
echo "$MUTATE_ID $1" >> calls.log
if [ -f hook.sh ]; then . ./hook.sh; fi
case "$1" in
  build) exec cargo test --no-run ;;
  test) exec cargo test -- "$2" ;;
esac
exit 64
