#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Installs the built programs and the files in packaging/ into a staging
# tree, as the Arch PKGBUILD's package() does, for the other Linux
# packages (the plain tarball, .deb, .rpm and AppImage).
#
#   packaging/linux/stage.sh BINDIR DESTDIR [PREFIX]
#
# BINDIR holds katna-pdf and katna-pdf-cli; files go to DESTDIR/PREFIX
# (PREFIX defaults to /usr). Files named by the app ID are installed
# with globs, so no ID is spelled out here.
set -eu

bindir=$1
destdir=$2
prefix=${3:-/usr}
root=$(cd "$(dirname "$0")/../.." && pwd)
out="$destdir$prefix"

put() { # MODE DIR FILE...
  mode=$1 dir=$2
  shift 2
  mkdir -p "$dir"
  for file in "$@"; do
    install -m "$mode" "$file" "$dir/"
  done
}

put 755 "$out/bin" "$bindir/katna-pdf" "$bindir/katna-pdf-cli"
put 644 "$out/share/applications" "$root"/packaging/desktop/*.desktop
# Katna PDF's entry in Discover and GNOME Software.
put 644 "$out/share/metainfo" "$root"/packaging/metainfo/*.metainfo.xml
put 644 "$out/share/icons/hicolor/scalable/apps" "$root"/packaging/icons/*.svg
put 644 "$out/share/licenses/katna-pdf" "$root/LICENSE"
