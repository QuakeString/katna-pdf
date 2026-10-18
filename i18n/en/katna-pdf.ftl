# SPDX-License-Identifier: GPL-3.0-or-later
# Katna PDF's English text. One message per line: `id = text`, with
# variables written `{ $name }`.

app-name = Katna PDF
usage = Usage: katna-pdf [FILE.pdf…]

## The empty window
empty-title = Open a PDF
empty-detail = Choose a file, or drop one here.
empty-open = Open…

## The toolbar over the pages
page-of = { $page } / { $count }
zoom-in = Zoom in (Ctrl+=)
zoom-out = Zoom out (Ctrl+-)
zoom-fit-width = Fit width (Ctrl+1)
zoom-actual = Actual size (Ctrl+0)
open-file = Open a file (Ctrl+O)
side-pane-toggle = Side pane (F7)

## The side pane beside the pages
pane-pages = Pages
pane-contents = Contents

## Tabs
tab-empty = New tab

## Opening
opening = Opening { $name }…
error-locked = { $name } is protected by a password. Opening protected files comes next.
error-invalid = { $name } is damaged or is not a PDF.
error-read = { $name } could not be read: { $error }
