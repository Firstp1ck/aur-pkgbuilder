#!/usr/bin/env bash
#
# Module structure visualisation for aur-pkgbuilder.
#
# Generates a text tree plus per-module DOT / PNG / SVG dependency graphs
# for every top-level folder under `src/`.
#
# Requirements:
# - cargo-modules (installed automatically if missing)
# - Graphviz (`dot` command) — install manually (`sudo pacman -S graphviz`).
#
# Output:
#   dev/scripts/Modules/
#   ├── module_tree.txt
#   ├── ui/
#   │   ├── module_graph.dot
#   │   ├── module_graph.png
#   │   └── module_graph.svg
#   └── workflow/
#       └── …

set -e

COLOR_RESET=$(tput sgr0)
# shellcheck disable=SC2034 # Used in printf statements
COLOR_BOLD=$(tput bold)
COLOR_GREEN=$(tput setaf 2)
COLOR_YELLOW=$(tput setaf 3)
COLOR_BLUE=$(tput setaf 4)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MODULES_DIR="$SCRIPT_DIR/Modules"
mkdir -p "$MODULES_DIR"

PROJECT_ROOT="$(cd "$SCRIPT_DIR" && while [ ! -f "Cargo.toml" ] && [ "$PWD" != "/" ]; do cd ..; done && pwd)"
if [ ! -f "$PROJECT_ROOT/Cargo.toml" ]; then
  printf "%bError: Could not find Cargo.toml.%b\n" "$COLOR_YELLOW" "$COLOR_RESET" >&2
  exit 1
fi
cd "$PROJECT_ROOT"

if ! command -v cargo-modules &>/dev/null; then
  printf "%bInstalling cargo-modules…%b\n" "$COLOR_BLUE" "$COLOR_RESET"
  cargo install cargo-modules
fi

# aur-pkgbuilder is a binary crate (no lib.rs), so target the bin.
CRATE_TARGET=(--bin aur-pkgbuilder)

printf "%bGenerating module tree (binary)…%b\n" "$COLOR_BLUE" "$COLOR_RESET"
cargo modules structure "${CRATE_TARGET[@]}" > "$MODULES_DIR/module_tree.txt"
printf "%bModule tree saved to %s%b\n" "$COLOR_GREEN" "$MODULES_DIR/module_tree.txt" "$COLOR_RESET"

if ! command -v dot &>/dev/null; then
  printf "\n%bGraphviz not found. Install with:%b\n" "$COLOR_YELLOW" "$COLOR_RESET"
  echo "  - Linux:   sudo pacman -S graphviz"
  echo "  - macOS:   brew install graphviz"
  exit 0
fi

# Top-level modules under src/. Update when new modules land.
SUBFOLDERS=("ui" "workflow")

generate_module_graph() {
  local module_name=$1
  local focus_path="aur_pkgbuilder::$module_name"
  local module_dir="$MODULES_DIR/$module_name"
  mkdir -p "$module_dir"

  echo ""
  printf "%bGenerating graph for module: %s%b\n" "$COLOR_BLUE" "$module_name" "$COLOR_RESET"

  local dot_file="$module_dir/module_graph.dot"
  cargo modules dependencies "${CRATE_TARGET[@]}" --focus-on "$focus_path" --no-externs > "$dot_file" 2>/dev/null || {
    printf "  %bWarning: Failed to generate DOT for %s, skipping…%b\n" "$COLOR_YELLOW" "$module_name" "$COLOR_RESET"
    rmdir "$module_dir" 2>/dev/null || true
    return 1
  }

  if [ ! -s "$dot_file" ] || [ "$(wc -l < "$dot_file")" -lt 3 ]; then
    printf "  %bNo dependencies found for %s, skipping…%b\n" "$COLOR_YELLOW" "$module_name" "$COLOR_RESET"
    rm -f "$dot_file"
    rmdir "$module_dir" 2>/dev/null || true
    return 1
  fi

  for fmt in png svg; do
    if dot -T"$fmt" \
      -Gdpi=160 \
      -Gsize=30,30 \
      -Goverlap=prism \
      -Gsplines=ortho \
      -Gnodesep=1.5 \
      -Granksep=2.0 \
      -Nfontsize=12 \
      -Nfontname="Arial" \
      -Nshape=box \
      -Nstyle="rounded,filled" \
      -Nfillcolor="#f8f8f8" \
      -Ncolor="#333333" \
      -Epenwidth=1.2 \
      -Ecolor="#666666" \
      -Earrowsize=0.7 \
      "$dot_file" > "$module_dir/module_graph.$fmt" 2>/dev/null; then
      printf "  %b%s saved: %s%b\n" "$COLOR_GREEN" "${fmt^^}" "$module_dir/module_graph.$fmt" "$COLOR_RESET"
    fi
  done
}

for folder in "${SUBFOLDERS[@]}"; do
  generate_module_graph "$folder"
done

echo ""
printf "%bAll module graphs generated under: %s%b\n" "$COLOR_GREEN" "$MODULES_DIR" "$COLOR_RESET"
