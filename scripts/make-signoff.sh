#!/usr/bin/env bash
# Write the AutoCAD sign-off pack into signoff/ (or the folder
# given): the drawings opendwg writes, the originals they came from,
# untouched copies, and SIGNOFF.md, the checklist a person fills in while
# checking each drawing in AutoCAD on Windows.
#
#   scripts/make-signoff.sh [--force] [--allow-partial] [out dir]
#
# The sample corpus must be fetched first (scripts/fetch-corpus.sh);
# --allow-partial makes a pack without the corpus round trips instead.
# An existing pack whose SIGNOFF.md has results filled in is kept unless
# --force is given.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
force=0
partial=0
out=""
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    --allow-partial) partial=1 ;;
    -*) echo "make-signoff: unknown option $arg" >&2; exit 2 ;;
    *) out="$arg" ;;
  esac
done
out="${out:-$root/signoff}"

# The corpus: every file of the manifest fetched.
absent=""
while read -r name; do
  [ -f "$root/corpus/files/$name.dwg" ] || absent="$absent $name"
done < <(sed -n 's/^name = "\(.*\)"$/\1/p' "$root/corpus/manifest.toml" 2>/dev/null)
if [ -n "$absent" ] && [ "$partial" -ne 1 ]; then
  echo "make-signoff: the sample corpus is not fetched (missing:$absent)." >&2
  echo "Run scripts/fetch-corpus.sh first, or pass --allow-partial for a pack without it." >&2
  exit 1
fi

# Only replace a folder this script made (or an empty one), and not one
# with results recorded in it.
if [ -e "$out" ]; then
  if [ -f "$out/SIGNOFF.md" ]; then
    if grep -qiE '^\|.*\| *(pass|fail)[^|]*\|' "$out/SIGNOFF.md" && [ "$force" -ne 1 ]; then
      echo "make-signoff: $out/SIGNOFF.md has results filled in; keeping it." >&2
      echo "Move it away, or pass --force to replace it." >&2
      exit 1
    fi
    rm -rf "$out"
  elif [ -z "$(ls -A "$out")" ]; then
    rm -rf "$out"
  else
    echo "make-signoff: $out exists and is not a sign-off pack; not touching it" >&2
    exit 1
  fi
fi

commit="$(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo unknown)"
if [ -n "$(git -C "$root" status --porcelain 2>/dev/null)" ]; then
  commit="$commit (with uncommitted changes)"
fi
date="$(date -u +%Y-%m-%d)"

cargo run --release --quiet --manifest-path "$root/Cargo.toml" \
  --example make_signoff -- "$out" "$commit" "$date"

# Check the file list: the drawings every pack has, the checklist, and one
# untouched copy per drawing.
missing=0
for f in SIGNOFF.md new_empty.dwg new_every_entity.dwg new_200k_lines.dwg edit_dimlfac.dwg; do
  if [ ! -s "$out/$f" ]; then
    echo "make-signoff: missing $f" >&2
    missing=1
  fi
done
for f in "$out"/*.dwg; do
  if ! cmp -s "$f" "$out/untouched/$(basename "$f")"; then
    echo "make-signoff: no untouched copy of $(basename "$f")" >&2
    missing=1
  fi
done
if [ "$missing" -ne 0 ]; then
  exit 1
fi

echo
echo "Sign-off pack in $out:"
(cd "$out" && find . -type f | sort)
echo
echo "Copy the folder to the Windows machine and follow SIGNOFF.md."
