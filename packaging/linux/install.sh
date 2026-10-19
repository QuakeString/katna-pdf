#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Installs Katna PDF from this folder for you alone (~/.local), or for
# everyone (/usr/local) when run as root; `./install.sh --uninstall`
# takes it out again.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
if [ "$(id -u)" = 0 ]; then
  prefix=/usr/local
else
  prefix=$HOME/.local
fi

# Every file this folder installs, relative to it.
files=$(cd "$here" && find bin share -type f)

if [ "${1:-}" = --uninstall ]; then
  for file in $files; do
    rm -f "${prefix:?}/${file:?}"
  done
  echo "Katna PDF was taken out of $prefix."
  exit 0
fi

for file in $files; do
  case $file in
    bin/*) mode=755 ;;
    *) mode=644 ;;
  esac
  mkdir -p "$prefix/$(dirname "$file")"
  install -m "$mode" "$here/$file" "$prefix/$file"
done
if command -v update-desktop-database > /dev/null; then
  update-desktop-database -q "$prefix/share/applications" 2> /dev/null || true
fi
if command -v gtk-update-icon-cache > /dev/null; then
  gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2> /dev/null || true
fi

echo "Katna PDF is installed in $prefix."
case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) echo "Add $prefix/bin to your PATH to run katna-pdf from a terminal." ;;
esac
