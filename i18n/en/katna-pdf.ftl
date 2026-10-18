# SPDX-License-Identifier: GPL-3.0-or-later
# Katna PDF's English text. One message per line: `id = text`, with
# variables written `{ $name }`.

app-name = Katna PDF
usage = Usage: katna-pdf [FILE.pdf…]

## The empty window
empty-title = Open a PDF
empty-detail = Choose a file, or drop one here.
empty-open = Open…
recent-title = Recent

## The toolbar over the pages
page-of = { $page } / { $count }
zoom-in = Zoom in (Ctrl+=)
zoom-out = Zoom out (Ctrl+-)
zoom-fit-width = Fit width (Ctrl+1)
zoom-fit-page = Fit page (Ctrl+2)
spread-single = Layout: one page at a time (Ctrl+3 to change)
spread-facing = Layout: two pages side by side (Ctrl+3 to change)
spread-book = Layout: two pages, first page alone (Ctrl+3 to change)
zoom-actual = Actual size (Ctrl+0)
open-file = Open a file (Ctrl+O)
side-pane-toggle = Side pane (F7)
night-pages = Night pages (Ctrl+Shift+N)
search-open = Find (Ctrl+F)

## Find in the document
search-placeholder = Find in the document
search-count = { $current } of { $total }
search-searching = Searching…
search-none = No matches
search-previous = Previous match (Shift+F3)
search-next = Next match (F3)
search-close = Close (Esc)

## The side pane beside the pages
pane-pages = Pages
pane-contents = Contents

## Tabs
tab-empty = New tab

## Opening
opening = Opening { $name }…
error-encryption = { $name } is encrypted in a way Katna PDF cannot read yet.
locked-title = { $name } is protected
locked-detail = Enter its password to open it.
locked-wrong = That password did not open it. Try again.
locked-open = Open
password-placeholder = Password
print = Print (Ctrl+P)
print-no-dialog = There is no print dialog on this desktop. Install xdg-desktop-portal to print.
print-failed = Printing failed: { $error }
notice-close = Close
error-invalid = { $name } is damaged or is not a PDF.
error-read = { $name } could not be read: { $error }
