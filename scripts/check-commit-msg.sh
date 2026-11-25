#!/bin/sh
# Conventional-commit gate: one line, <type>: <lowercase imperative summary>
# of at most 72 characters. Merge and release subjects pass through.
pattern='^(feat|fix|perf|refactor|test|docs|chore|ci|build|revert): [a-z].{0,70}$'
subject=$(head -n1 "$1")
case "$subject" in
  "Merge "*|"Release "*v*) exit 0 ;;
esac
if ! printf '%s' "$subject" | grep -Eq "$pattern"; then
  echo "commit message rejected: $subject" >&2
  echo "expected: type: lowercase imperative summary, max 72 chars" >&2
  echo "types: feat fix perf refactor test docs chore ci build revert" >&2
  exit 1
fi
if [ "$(grep -v -e '^#' -e '^[[:space:]]*$' "$1" | wc -l)" -gt 1 ]; then
  echo "commit message rejected: write one line, with no body" >&2
  exit 1
fi
