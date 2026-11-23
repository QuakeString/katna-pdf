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
- **Pages mode** (P01, P02): the organizer's grid fills the window; its
  tray adds pages, turns, moves, deletes, saves the pages picked as a
  file, crops and numbers them. The Pages card is a dialog with tabs
  (Add, Replace, Page size, Margins, Split), one height for all.
- **Forms mode** (F03): the tray holds the pointer, the eight kinds of
  field (the one in hand shows its name), Find fields, form data in and
  out, and reset. The field picked gets a card at the top right with
  Field, Look and (for buttons) When clicked tabs; with none picked the
  card lists the page's Tab order.
- **Sign mode** (G01): the tray holds Sign, Sign without a box, the
  Signatures card and Cancel, with a hint over it until the signature
  has its place; then the certificate card opens at the top right (USB
  token or certificate file as a segmented control).
- **Paragraph bar**: the paragraph being typed in has its font, size,
  bold, italic, alignment, spacing, copy and delete in a bar floating
  just over it (`textedit.rs`).
- **Object card** (`objects.rs` `inspector`): the pictures and shapes
  picked get a card at the top right (E07): place and size, fill and
  line with switches and see-through, width, dashes, ends, corners,
  edit points, arrange (line up, turn, flip, fit, order), layer, delete.
  Notices and the signatures line float at the top, Find at the top right.
- **Library** (`library.rs`, L01): the places are a card floating clear
  of the window's edges (headed Library, with a close button), rows
  rounded with the accent's wash for the place shown and section names
  in small capitals; the covers are cards with a shadow and a badge at
  the top right; the chosen file's strip and the strip for many chosen
  float at the bottom.
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

## Settings

Settings is a window of its own (`settings.rs`, X row), as Okular's is,
and what changes there applies at once to every window. Pages down the
left, Close at the foot; the open page on the right under its name.
Profile and signatures… in the person's menu (or a click on who they
are) opens it at Profile, Settings… at Appearance, Keyboard shortcuts
at Shortcuts; asking again brings the open window forward.

- Profile (X00): name, short name, job title and organisation, kept in
  the state folder (`pdf/profile.txt`, `profile.rs`). Comments and
  stamps carry the short name, else the name; the person's picture and
  menu show the name and the job. Under them the initials, the
  signature picture and the person's own stamps, each saying where it
  is made.
- Appearance (X01): Mode as a segmented control, then a tile for each
  scheme, its day half beside its night half, the desktop's, and the
  person's own beside Make your own and Import…. The scheme editor
  (X10) floats over the page: a name, Day side and Night side, five
  colours a side typed as `#rrggbb` with a dot of each, and a preview
  of the side shown. Import (X11) reads a KDE `.colors` file into the
  editor, its own side as it is and the other made from Float's with
  its accent; anything else says why under the tiles. Kept in
  `pdf/schemes.txt`; the profile menu's dots list them too. Under the
  tiles the accent: the scheme's own, or one of Katna Mail's seven,
  over any scheme (`Look::accent`, Katna Mail's `accent` setting).
  Then (X09) the pages' colours when the window is dark, the size of
  everything (`katna_ui::scale`, 90 to 125 %), the mode's name in the
  dock, and Same colours as Katna Mail: shown when Katna Mail's
  `config.toml` exists, it takes Katna Mail's mode, scheme and accent,
  again whenever a window comes to the front, and writes the ones picked
  here back to it (one of the person's own schemes stays Katna PDF's).
  Pages that follow the scheme's card colour and a side pane that hides
  by itself are not built.
- Viewing (X02): where files open (last page read or page 1), their
  zoom and layout, the last session's files on start, and drawing in
  worker processes; kept in `pdf/prefs.txt` (`prefs.rs`).
- Editing (X03): the unit an object's place and size are shown and
  typed in (a bare number counts in it; `1in` still works), the grid
  from the start, and the folder of the person's stamps and initials.
- Signing (X04): token drivers found, timestamping and its server,
  long-term proof, and the trusted roots folder. These are the signing
  card's own remembered choices, so either place changes both.
- Library (X05): the watched folders with the files in each, taken
  off or added; an open library reads them again at once.
- Read aloud (X06): which voice reads (shown once one has been opened,
  as opening Speech Dispatcher is slow), its speed (SSIP `RATE`,
  espeak's `-s`, Windows' `Rate`) and a button to hear it.
- Shortcuts (X07): every menu item with keys, and a box to find one.
- About (X08): the build, whether it looks for updates, and the crash
  reports folder.

## Signing dialog

Placing the box opens the signing card as a dialog over the dimmed page
(G02): USB token or certificate file, the token's certificates or the
file, the PIN or password, reason and place side by side, what the box
shows, then switches for the timestamp, long-term proof and locking the
form, certifying, more files, and Cancel and Sign at the foot. A wrong
PIN says what the token says of the tries left (PKCS#11's count-low and
final-try flags, `pkcs11::PinTries`), and a locked token how to unlock
it (G05).

The profile menu (S14) says "Add your name" until Settings has one, and
under it whether a token is plugged in, looked for each time the menu
opens, with a way to Settings > Signing. Settings… is Ctrl+, and About
Katna PDF opens Settings > About.

## Still to come

Pages at night in the scheme's own card colour, and a side pane that
hides by itself (X09).
