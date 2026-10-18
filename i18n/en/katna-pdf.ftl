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
page-colors = Page colours (Ctrl+Shift+N for normal or the last picked)
colors-normal = Normal
colors-night = Night (keeps photos)
colors-invert = Invert colours
colors-invert-lightness = Invert lightness
colors-sepia = Sepia paper
colors-black-white = Black and white
search-open = Find (Ctrl+F)
organize = Organize pages
annotate = Annotate

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
pane-comments = Comments

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
save = Save
save-tip = Save the changes to the file (Ctrl+S)
save-failed = The file could not be saved: { $error }
unsaved-close = The changes to this file are not saved. Save them with Ctrl+S, or close again to leave them.
form-failed = The form could not be filled in: { $error }
pages-turn-left = Turn the pages picked left
pages-turn-right = Turn the pages picked right
pages-earlier = Move the pages picked up
pages-later = Move the pages picked down
pages-delete = Delete the pages picked
pages-insert = Insert the pages of another PDF after the pages picked
pages-extract = Save the pages picked as a new PDF
pages-extract-name = { $name } (pages).pdf
pages-extracted = The pages were saved as { $name }.
pages-failed = The pages could not be changed: { $error }

## Annotating
tool-highlight = Highlight text
tool-underline = Underline text
tool-strikeout = Strike text out
tool-squiggly = Squiggle under text
tool-note = Note
tool-free-text = Text box
tool-ink = Pen
tool-rectangle = Rectangle
tool-ellipse = Ellipse
tool-line = Line
tool-arrow = Arrow
tool-stamp = Stamp
tool-colour = Colour
annot-shape = Shape
annot-other = Annotation
annotate-failed = The annotation could not be added: { $error }
comment-placeholder = Write a comment
comment-page = Page { $page }
comment-delete = Delete (Del)
comments-none = No comments or annotations yet. Pick a tool with the pen button above the page.
comments-export = Save the comments as an XFDF file
comments-import = Add comments from an XFDF file
comments-exported = The comments were saved as { $name }.
comments-import-none = That file has no comments for these pages.
comments-failed = The comments could not be saved or added: { $error }
error-invalid = { $name } is damaged or is not a PDF.
error-read = { $name } could not be read: { $error }
