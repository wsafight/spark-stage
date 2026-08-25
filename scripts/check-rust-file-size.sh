#!/bin/sh
set -eu

line_limit="${SPARKSTAGE_RUST_FILE_LINE_LIMIT:-900}"

find src -type f -name '*.rs' -exec wc -l {} + | awk -v limit="$line_limit" '
    $2 != "total" && $1 >= limit {
        printf "%s has %d lines; Rust files must stay below %d lines\n", $2, $1, limit
        failed = 1
    }
    END { exit failed }
'
