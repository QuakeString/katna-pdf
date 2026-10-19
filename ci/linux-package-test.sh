#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Tries an installed Katna PDF the way a person would: opens a small PDF
# in a virtual X display, checks the window shows and stays up, and
# saves a screenshot. Run by .github/workflows/linux-packages.yml after
# installing each package.
#
#   ci/linux-package-test.sh SCREENSHOT.png
#
# KATNA_PDF and KATNA_PDF_CLI name the programs (default: on PATH).
# Needs Xvfb, x11-utils (xwininfo) and ImageMagick (import).
set -eu

shot=$1
app=${KATNA_PDF:-katna-pdf}
cli=${KATNA_PDF_CLI:-katna-pdf-cli}
work=$(mktemp -d)
# The window's class, as the desktop entry names it.
class=$(sed -n 's/^StartupWMClass=//p' "$(dirname "$0")"/../packaging/desktop/*.desktop)

# A one-page PDF with a line of text on it.
cp "$(dirname "$0")/package-test.pdf" "$work/test.pdf"
$cli --help > /dev/null
$cli info "$work/test.pdf"

Xvfb :77 -screen 0 1280x800x24 > /dev/null 2>&1 &
xvfb=$!
export DISPLAY=:77
sleep 2
XDG_STATE_HOME=$work/state XDG_DATA_HOME=$work/data XDG_CONFIG_HOME=$work/config \
  $app "$work/test.pdf" > "$work/app.log" 2>&1 &
pid=$!
shown=
for _ in $(seq 30); do
  sleep 1
  if ! kill -0 "$pid" 2> /dev/null; then
    break
  fi
  if xwininfo -root -tree | grep -qF "(\"$class\""; then
    shown=yes
    break
  fi
done
sleep 3
import -window root "$shot" || true
if [ -z "$shown" ] || ! kill -0 "$pid" 2> /dev/null; then
  echo "Katna PDF did not show its window:"
  cat "$work/app.log"
  kill "$pid" "$xvfb" 2> /dev/null || true
  exit 1
fi
kill "$pid" "$xvfb" 2> /dev/null || true
echo "Katna PDF opened the file."
