#!/bin/sh
set -eu

required_files='README.md docs/README.md docs/overview.md docs/principles.md docs/architecture.md docs/operations.md docs/quality.md docs/product.md docs/technical.md docs/optimization.md'

for file in $required_files; do
    if [ ! -f "$file" ]; then
        printf 'missing required documentation file: %s\n' "$file" >&2
        exit 1
    fi
done

failed=0
for source in README.md docs/*.md docs/evidence/*.md; do
    [ -f "$source" ] || continue
    source_dir=$(dirname "$source")
    while IFS= read -r target; do
        [ -n "$target" ] || continue
        case "$target" in
            '#'*|'http://'*|'https://'*|'mailto:'*) continue ;;
            ../reference/OpenMontage|../reference/OpenMontage/) continue ;;
        esac
        target=${target%%\#*}
        target=${target%%\?*}
        [ -n "$target" ] || continue
        case "$target" in
            /*) resolved=${target#/} ;;
            *) resolved="$source_dir/$target" ;;
        esac
        if [ ! -e "$resolved" ]; then
            printf '%s: broken local link: %s\n' "$source" "$target" >&2
            failed=1
        fi
    done <<EOF
$(rg -o '\]\([^)]+' "$source" | sed 's/^.*](//')
EOF
done

if rg -n 'TODO|FIXME|REPLACE_WITH|REPLACE_OUTPUT|REPLACE_PROMPT' README.md docs --glob '*.md' >/dev/null; then
    printf 'documentation contains an unresolved placeholder\n' >&2
    failed=1
fi

if [ "$failed" -ne 0 ]; then
    exit 1
fi

printf 'Documentation checks passed: required files, local links, and placeholders.\n'
