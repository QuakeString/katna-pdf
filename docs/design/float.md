# Float: Katna PDF's look

Katna PDF's own look, apart from Katna Mail's (picked 2026-11-16). The
storyboard of every screen, day and night, is the design source:
<https://claude.ai/artifact/V2YhTg2a2ccj3mKP7NF65P>. This file says how the
app carries it out.

## The window

- **Title bar** (`chrome.rs`), 48 px: Library and the side card's button on
  the left, then undo and redo once there is something to undo, then the
  file's name (Properties on a click; a dot while changes are not saved).
  With two files or more the tabs sit here instead. On the right: Save
  (only while there are changes), Find, the share menu (print, export,
  compare, present, read aloud, make smaller, PDF/A, password,
  properties), the app menu (File, Edit, View, Go: the same menus KDE's
  global menu shows, `menus.rs`) and the person's picture. Under the
  desktop's own frame this is the window's toolbar; where the window
  draws its own frame it also moves the window and holds its buttons.
- **Side card** (`viewer.rs` `pane`): floats 12 px from the window's edges,
  rounded 8 px, with icon tabs (Pages, Contents, Comments, Files, Layers,
  Signatures, Changes). The pages move aside for it.
- **Dock** (`viewer/dock.rs`): floats 18 px above the bottom. The modes
  (Read, Comment, Edit, Pages, Forms, Sign; the mode on shows its name in
  inverse colours), zoom out, the zoom (a card of fits, steps, layouts and
  the hand tool), zoom in, fit page, the page number, the pages' colours.
- **Tray** (`float::tray`): the mode's tools float above the dock (C01,
  E01); the tool in hand shows its name on the accent's wash, colours are
  dots ringed when picked. The bars a tool opens (measure, initials, read
  aloud) and the stamp card (C05: Standard, With date and name, Mine)
  float above it too.
- **Paragraph bar**: the paragraph being typed in has its font, size,
  bold, italic, alignment, spacing, copy and delete in a bar floating
  just over it (`textedit.rs`).
- **Object card** (`objects.rs` `inspector`): the pictures and shapes
  picked get a card at the top right (E07): place and size, fill and
  line with switches and see-through, width, dashes, ends, corners,
  edit points, arrange (line up, turn, flip, fit, order), layer, delete.
  Notices and the signatures line float at the top, Find at the top right.
- **Profile menu**: the person, Mode (System, Day, Night), the colour
  schemes, the pages' colours, the app menu, an update when one is ready,
  Quit.

## Colours

`look.rs` keeps the mode and the scheme (`pdf/look.txt` in the state
folder). A scheme gives five colours for day and five for night: page,
card, text, faint, accent. `theme.rs` mixes the rest from them as the
storyboard does (`Win.dc.html`): fields 6 % of the text over the card,
lines 15 %, soft lines 9 %, the accent's wash 12 % (20 % at night), and so
on. Float's own scheme keeps the storyboard's hand-tuned values
(`core.css`). The schemes are Float's, Katna Mail's thirteen
(`katna_ui::schemes`) and System (the desktop's, from
`katna_platform::colors`).

Katna's shared controls (`katna_ui::widgets`) take a Katna palette made
from the same scheme (`Theme::katna`), so they wear the same colours until
each screen moves to Float's controls (`float.rs`).

## Sizes

Radii: 6 px for icon buttons and tabs, 8 px for cards, menus and dock
buttons, 10 px for floating bars, 12 px for the dock. Text 13 px in bars
and menus, 11 px small capitals for headings. Icons are the storyboard's
line icons (`icons/float/`, served as `icons/f/NAME.svg`), 1.7 px strokes.

## Still to come

One PR per area of the storyboard: pages, forms and signing (P, F, G), the
library (L), and the Settings window with the Profile page and the scheme
editor (X, T).
