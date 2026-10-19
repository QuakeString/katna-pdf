#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Prints the version of the checked-out commit, as the Arch package's
# pkgver(): `X.Y.Z.rN.gHASH` after a `vX.Y.Z` tag, or 0.0.0.rCOUNT.gHASH
# before the first.
set -eu
if version=$(git describe --long --tags --abbrev=7 --match 'v[0-9]*' 2> /dev/null); then
  printf '%s\n' "$version" | sed 's/^v//;s/-\([0-9]*\)-g/.r\1.g/'
else
  printf '0.0.0.r%s.g%s\n' "$(git rev-list --count HEAD)" "$(git rev-parse --short=7 HEAD)"
fi
