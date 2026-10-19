#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds katna-pdf_VERSION-1_amd64.deb, for Ubuntu 22.04 and later and
# Debian 12 and later, from the plain Linux download (make-tarball.sh):
# the same files, under /usr.
#
#   packaging/deb/make-deb.sh katna-pdf-linux-x86_64.tar.gz VERSION OUTDIR
#
# The programs are built on Ubuntu 22.04, so they need its glibc (2.35).
# Depends names the libraries they load, so `sudo apt install ./FILE.deb`
# brings them in. Needs dpkg-deb.
set -eu

tarball=$1
version=$2
outdir=$3
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
root=$work/root

mkdir -p "$root/usr" "$work/unpacked"
tar -C "$work/unpacked" -xzf "$tarball"
for dir in bin share; do
  mv "$work/unpacked/katna-pdf-linux-x86_64/$dir" "$root/usr/"
done
# Debian keeps the licence as the package's copyright file.
mkdir -p "$root/usr/share/doc/katna-pdf"
mv "$root/usr/share/licenses/katna-pdf/LICENSE" "$root/usr/share/doc/katna-pdf/copyright"
rm -r "${root:?}/usr/share/licenses"

mkdir -p "$root/DEBIAN"
size=$(du -sk "$root/usr" | cut -f1)
sed -e "s/@VERSION@/$version-1/" -e "s/@SIZE@/$size/" "$here/control" > "$root/DEBIAN/control"
(cd "$root" && find usr -type f -exec md5sum {} + | sort -k2) > "$root/DEBIAN/md5sums"

mkdir -p "$outdir"
dpkg-deb --root-owner-group -Zxz --build "$root" "$outdir/katna-pdf_${version}-1_amd64.deb"
rm -rf "${work:?}"
