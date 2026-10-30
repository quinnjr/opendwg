#!/bin/sh
# Download the public sample corpus listed in corpus/manifest.toml and corpus/extended.toml into
# corpus/files/. Idempotent: files whose SHA-256 already matches are skipped. Exits non-zero on any
# failure. Git LFS media URLs (media.githubusercontent.com) are plain HTTPS downloads.
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
dest="$root/corpus/files"
mkdir -p "$dest"

# name<TAB>url<TAB>sha256 per [[file]] entry of both manifests.
entries=""
for manifest in "$root/corpus/manifest.toml" "$root/corpus/extended.toml"; do
  [ -f "$manifest" ] || { echo "missing $manifest" >&2; exit 1; }
  found=$(awk '
  function val(s) { sub(/^[^=]*= *"/, "", s); sub(/".*$/, "", s); return s }
  function flush() { if (name != "") print name "\t" url "\t" sha; name = ""; url = ""; sha = "" }
  /^\[\[file\]\]/ { flush(); next }
  /^name *=/   { name = val($0) }
  /^url *=/    { url = val($0) }
  /^sha256 *=/ { sha = val($0) }
  END { flush() }
' "$manifest")
  [ -n "$found" ] || { echo "no entries found in $manifest" >&2; exit 1; }
  entries="$entries${entries:+
}$found"
done

fail=0
tmp=""
cleanup() { [ -z "$tmp" ] || rm -f "$tmp"; }
# A signal stops the whole run (not just the current download), with the
# shell's usual status for it.
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM
tab=$(printf '\t')
while IFS="$tab" read -r name url sha; do
  out="$dest/$name.dwg"
  if [ -f "$out" ] && [ "$(sha256sum "$out" | cut -d' ' -f1)" = "$sha" ]; then
    echo "OK (cached) $name"
    continue
  fi
  tmp="$out.part"
  if ! curl -fsSL --retry 3 -o "$tmp" "$url"; then
    echo "FAIL download $name ($url)" >&2
    rm -f "$tmp"; fail=1; continue
  fi
  got=$(sha256sum "$tmp" | cut -d' ' -f1)
  if [ "$got" != "$sha" ]; then
    echo "FAIL checksum $name: expected $sha, got $got" >&2
    rm -f "$tmp"; fail=1; continue
  fi
  mv "$tmp" "$out"
  echo "OK $name"
done <<EOF
$entries
EOF
exit $fail
