#!/usr/bin/env bash
# Is this exact seeds-ai version served by crates.io?
# 0: exact version present; 1: absent (404); 2: observation unavailable/invalid.
set -uo pipefail
crate=seeds-ai
expected=${1:-}
[[ $expected =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || exit 2
body=$(mktemp)
trap 'rm -f "$body"' EXIT
if ! status=$(curl -sS --max-time 30 --retry 3 --retry-delay 2 \
    -H 'User-Agent: seeds-release-ci (github.com/scbrown/seeds)' \
    -o "$body" -w '%{http_code}' "https://crates.io/api/v1/crates/$crate/$expected"); then
    echo 'Registry request failed; presence is unknown.' >&2
    exit 2
fi
case "$status" in
    200)
        if jq -e --arg expected "$expected" '.version.num == $expected' "$body" >/dev/null; then
            echo "crates.io serves $crate $expected"
            exit 0
        fi
        echo 'Registry response does not identify the expected version.' >&2
        exit 2 ;;
    404) echo "crates.io does not yet serve $crate $expected"; exit 1 ;;
    *) echo "Registry HTTP $status; presence is unknown." >&2; exit 2 ;;
esac
