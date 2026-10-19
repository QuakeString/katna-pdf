#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds katna-pdf-linux-x86_64.tar.gz, the plain Linux download: the
# programs, the files in packaging/ laid out as under /usr, and
# install.sh.
#
#   packaging/linux/make-tarball.sh BINDIR OUTDIR
set -eu

bindir=$1
outdir=$2
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
name=katna-pdf-linux-x86_64

"$here/stage.sh" "$bindir" "$work/stage" /usr
mv "$work/stage/usr" "$work/$name"
install -m 755 "$here/install.sh" "$work/$name/install.sh"
cp "$here/README.txt" "$work/$name/README.txt"
mkdir -p "$outdir"
tar -C "$work" --owner=0 --group=0 -czf "$outdir/$name.tar.gz" "$name"
rm -rf "${work:?}"
