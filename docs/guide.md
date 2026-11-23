# Katna PDF user guide

<!-- Help > User guide shows this as a PDF (apps/katna-pdf/src/guide.rs):
"## " lines become headings and bookmarks, "- " lines bullets, other
lines paragraphs. Keep to Latin letters: the PDF's fonts are the standard
ones. The keyboard list at the end is made from the app's own menus. -->

## Opening files

Press Open on the start screen or Ctrl+O, drop PDFs on the window, or open them from your file manager. Each file opens in a tab; Ctrl+Tab moves between them and Ctrl+W closes one. A file opens where you left it, and the start screen lists the files you opened lately and offers the last session's files again.

Pictures (PNG, JPEG) open as a new PDF, a page each. A file with a password asks for it. A damaged file is repaired as it opens, and File properties (Ctrl+D) lists what was done.

The library (Ctrl+L) shows every PDF in the folders you choose, with covers, tags, stars and notes, and finds words inside them. It also lists the Google Drive and OneDrive of accounts signed in to Katna Mail, and PDFs attached to your mail.

## Reading

- Zoom with Ctrl+= and Ctrl+-, or Ctrl and the wheel; fit the width with Ctrl+1, the page with Ctrl+2.
- Ctrl+3 shows one page, two side by side, or a book with its cover alone.
- F7 opens the side pane: pages, bookmarks, comments, attached files and layers.
- Ctrl+H hides everything but the pages. Ctrl+Shift+N turns the pages dark for reading at night; Settings > Appearance chooses how.
- Alt+Left and Alt+Right go back and forward through the places you jumped to.
- Right-click a picture on a page to copy it or save it as a file.
- A link to a website asks first, naming the site, since opening it tells the site you opened the file. Always open trusts that site from then on; Settings > Viewing opens such links without asking, or never, and forgets the sites you trusted.
- Esc closes the card in the middle of the window (print, export, properties and the rest), as a click beside it does.
- Ctrl+Shift+Y reads the text aloud from the page you are on. Settings > Read aloud chooses the voice, its speed and its pitch.
- F5 presents: full screen, a page at a time, with the file's own transitions. Space and the arrow keys move, B and W blank the screen, Esc stops.

## Finding text

Ctrl+F opens the find bar; Enter and F3 go to the next match, Shift+F3 to the one before. Put words "in quotes" to find them together. The bar's chips match capitals, whole words only, accents, and close spellings. Settings > Viewing chooses which are on when a file opens.

## Comments

The pen in the dock opens the comment tools: highlight, underline, strike out, notes, text boxes, callouts, drawing, shapes, lines, arrows, clouds, stamps, attached files and measuring. Choose text first to mark it, or choose a tool and draw on the page.

- Click a comment to choose it; drag it to move it, or its handles to resize it. Delete takes it off.
- The comment card holds its words, replies, a status (accepted, rejected and so on), and whether it is locked, hidden or printed.
- The star in the card keeps a comment's colour, line and opacity for new comments made with its tool.
- Insert and Replace mark where text should go in or be swapped, for the file's author to act on.
- The Comments pane filters by kind, author and status, and exports them as text, XFDF or FDF.

Comments carry the name in Settings > Profile.

## Filling and making forms

Click a field to type in it; Tab goes to the next. Date fields show a calendar, totals add up as you type, and the form's own checks run as it was made. Ctrl+S saves what you filled in.

The form designer, on the edit row, adds text fields, check boxes, radio buttons, lists, buttons and signature fields, sets how they look and what buttons do, keeps values to a pattern (a PAN, a phone number) or a range of numbers, works out totals and sums such as Fee * 0.18 + Postage, and finds the fields a flat form asks for from its lines and boxes.

## Editing pages

The pencil in the dock edits what the pages draw.

- Click a paragraph to type in it; it keeps its font, size and colour, and wraps to its width. Drag it to move it.
- Add text, pictures and shapes; move, resize, turn, restack, recolour and delete what is already there.
- Headers, footers, watermarks, Bates numbers and backgrounds go on many pages at once from Page marks.
- Redact takes text, pictures and drawings out for good, and finds PAN and Aadhaar numbers, phone numbers and emails.
- Ctrl+Z undoes and Ctrl+Shift+Z redoes, all the way back to the file as it opened, saved or not.

## Organizing pages

The grid in the dock shows every page. Drag pages to reorder them; choose some to turn, delete, copy, extract or print. Insert blank pages or pages from other files, replace pages, crop margins and change page sizes from the Pages card. Merge files from the library, and split them from the command line.

## Signing

The signing card signs with a certificate file (.pfx or .p12) or a USB token such as an ePass2003, with or without a timestamp, and can certify a file so only the changes you allow are made after. The Signatures pane checks every signature: whether the file changed since, whether the signer's certificate chains to a trusted root (India's CCA roots are built in), and whether it was revoked. See the certificate shows each certificate from the signer up to the root, with who it names, what it may do, its policies and fingerprints; Copy and Save take it out as a .cer file. Esc closes the card.

Initials and your signature picture are kept for the next time.

## Saving, sharing and printing

- Ctrl+S saves. A file opened from a drive is saved back to the drive too.
- A copy of changes not saved is kept every few minutes; after a crash the start screen offers it back.
- Ctrl+P prints: the pages you choose, as a booklet, several to a sheet, or a poster, in colour or grey, at a size of your own (10 to 400 %), upright or on its side, with the margins you pick.
- Ctrl+Shift+E exports pages as pictures, the text as plain text or a Word file, and tables as CSV.
- File > Send by email starts a message in Katna Mail or your mail app with the file attached.
- File properties (Ctrl+D) changes the title and author, puts on or takes off a password, makes the file smaller, and makes it PDF/A for keeping.

## Comparing files

File > Compare with (Ctrl+Shift+D) shows what changed between two versions of a file: words added and taken out, and pictures and drawings that differ, side by side or one over the other.

## Settings

Settings (Ctrl+,) has your profile, the app's look and colours, how files open, editing, signing, the library and reading aloud. Changes apply at once.

## The command line

katna-pdf-cli does most of what the app does from scripts: merge, split, rotate, sign, verify, fill forms, stamp, redact, compress, export and more. Run it with no arguments for the list, and add --json for scripts and agents.
