#!/usr/bin/env bash

# Analyze Clippy output and emit aggregated statistics into clippy_errors.txt.
# Useful for triaging a big batch of lint findings after a toolchain bump or a
# clippy.toml rules change.

COLOR_RESET=$(tput sgr0)
COLOR_BOLD=$(tput bold)
# shellcheck disable=SC2034 # Used in printf statements
COLOR_GREEN=$(tput setaf 2)
# shellcheck disable=SC2034 # Used in printf statements
COLOR_YELLOW=$(tput setaf 3)
COLOR_BLUE=$(tput setaf 4)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_FILE="${SCRIPT_DIR}/clippy_errors.txt"
TEMP_FILE=$(mktemp)

printf "%bRunning cargo clippy (this may take a moment)...%b\n" "$COLOR_BLUE" "$COLOR_RESET" >&2

cargo clippy --all-targets -- -D warnings 2>&1 \
  | grep "^error:" \
  | sed 's/^error: //' > "$TEMP_FILE"

CLIPPY_LINTS=$(grep -v "could not compile" "$TEMP_FILE" | sort | uniq -c | sort -rn)
COMPILE_ERRORS=$(grep    "could not compile" "$TEMP_FILE" | sort | uniq -c | sort -rn)

printf "%b=== Clippy Lints ===%b\n" "$COLOR_BOLD" "$COLOR_RESET" | tee "$OUTPUT_FILE"
if [ -n "$CLIPPY_LINTS" ]; then
  echo "$CLIPPY_LINTS" | awk '{
    count = $1
    total += count
    $1 = ""
    error_msg = substr($0, 2)
    printf "%5d %s\n", count, error_msg
  } END {
    printf "\n%5d total clippy lints\n", total
    printf "%5d unique lint types\n", NR
  }' | tee -a "$OUTPUT_FILE"
else
  echo "No clippy lints found." | tee -a "$OUTPUT_FILE"
fi

echo "" | tee -a "$OUTPUT_FILE"
printf "%b=== Compilation Errors ===%b\n" "$COLOR_BOLD" "$COLOR_RESET" | tee -a "$OUTPUT_FILE"
if [ -n "$COMPILE_ERRORS" ]; then
  echo "$COMPILE_ERRORS" | awk '{
    count = $1
    total += count
    $1 = ""
    error_msg = substr($0, 2)
    printf "%5d %s\n", count, error_msg
  } END {
    printf "\n%5d total compilation errors\n", total
  }' | tee -a "$OUTPUT_FILE"
else
  echo "No compilation errors found." | tee -a "$OUTPUT_FILE"
fi

rm -f "$TEMP_FILE"

echo "" >&2
printf "%bResults saved to: %s%b\n" "$COLOR_BLUE" "$OUTPUT_FILE" "$COLOR_RESET" >&2
