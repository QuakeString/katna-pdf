#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Writes katna-pdf-update.txt, which describes a build on a release for
# Katna PDF's own update check (apps/katna-pdf/src/update.rs):
#
#   packaging/update-manifest.sh VERSION OUT_DIR FILE...
#
# One `version` line, then a `file SHA256 SIZE NAME` line for each FILE.
set -eu
version=$1
out=$2
shift 2
manifest="$out/katna-pdf-update.txt"
{
  printf 'version %s\n' "$version"
  for file in "$@"; do
    sum=$(sha256sum "$file" | cut -d' ' -f1)
    size=$(wc -c < "$file" | tr -d ' ')
    printf 'file %s %s %s\n' "$sum" "$size" "$(basename "$file")"
  done
} > "$manifest.tmp"
mv "$manifest.tmp" "$manifest"
cat "$manifest"
