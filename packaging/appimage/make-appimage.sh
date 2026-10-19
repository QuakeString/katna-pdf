#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds Katna-PDF-x86_64.AppImage from built programs.
#
#   packaging/appimage/make-appimage.sh BINDIR OUTDIR
#
# Needs appimagetool on PATH (https://github.com/AppImage/appimagetool).
# Libraries every Linux desktop has (glibc, libxcb, libxkbcommon,
# fontconfig, freetype, Wayland, Vulkan and EGL) are not bundled, so the
# programs must be built on a system as old as the oldest one to support.
set -eu

bindir=$1
outdir=$2
here=$(cd "$(dirname "$0")" && pwd)
appdir=$(mktemp -d)/KatnaPdf.AppDir

"$here/../linux/stage.sh" "$bindir" "$appdir" /usr
install -m 755 "$here/AppRun" "$appdir/AppRun"
# appimagetool takes the desktop entry and icon from the AppDir's top.
desktop=$(ls "$appdir"/usr/share/applications/*.desktop | head -n1)
cp "$desktop" "$appdir/"
icon=$(sed -n 's/^Icon=//p' "$desktop" | head -n1)
cp "$appdir/usr/share/icons/hicolor/scalable/apps/$icon.svg" "$appdir/$icon.svg"
ln -s "$icon.svg" "$appdir/.DirIcon"

mkdir -p "$outdir"
ARCH=x86_64 appimagetool --no-appstream "$appdir" "$outdir/Katna-PDF-x86_64.AppImage"
