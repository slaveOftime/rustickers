# Desktop design

The GPUI Kit Design Guides are normative. This record describes the manager implementation, not a separate theme framework.

## Manager workspace

**Purpose:** find, inspect, and open saved desktop tools without implicitly executing them. The collection remains the largest region; no dashboard metrics or decorative cards.

- Sidebar: Library (All stickers, Text, Commands, Timers, Sketches, Files), Automation (Scheduled, Selection commands), Settings. TXTs is a separate transient workflow linked at the bottom; Settings is anchored beneath it. Brand uses the embedded `assets/icon.png`, not a pin glyph.
- Main pane: draggable header containing the single section title and description, search/sort/New sticker toolbar, virtualized collection, compact status footer. No repeated page heading.
- Default window: 820 × 620 logical pixels; minimum 560 × 480. Below 48 rem, the sidebar uses the framework's icon-collapse mode and tooltips. Both columns stretch to the viewport.
- Rows: 5.5 rem, stable object IDs, neutral surfaces, type/color identity in one icon slot, title and metadata in the flexible column, status and Open/Delete icon actions in a fixed trailing lane. Actions reveal on hover or selection and have equivalent Enter/Delete commands; there is no per-row settings dropdown. No faded closed rows; Closed and On desktop are explicit states.
- Command summaries: command, working directory, cron/arming state, selection input, and live execution/next-run status. Detailed editing uses the existing command sticker, never an independent inline editor.
- Settings: shared master-password dialogs, immediate clipboard collection switch, editable global shortcuts with explicit Save/default controls and validation feedback, local-data/privacy explanation.

## Tokens and geometry

Use `cx.theme()` background, foreground, sidebar, muted, border, list active/hover, and warning roles. Sticker swatches are data identity, not panel backgrounds. Use rem helpers, component Small/Medium sizes, system UI typography, and theme radii. Main content shares a 5-unit horizontal inset; related controls use 1–3-unit gaps and sections 5–6. Hairlines belong to region boundaries. No custom palette, fonts, shadows, or continuous motion.

## Interaction

Search filters immediately without SQL per keystroke. Selection survives refresh by sticker ID; arrows/Home/End/Page keys navigate and Enter opens. Ctrl/Cmd+F focuses search. Menus and settings controls use framework focus and dismissal. Header controls occlude the drag region. Command inspection does not restart a saved command; explicit selection execution remains separate.

## Verification status

Isolated Windows captures and reports live in Git-ignored `tests/visual/`; earlier captures are under `history/`. Fresh checks cover dark collection/settings, narrow scheduled commands, light/18-pixel base-font zoom, empty search, list-tail navigation, and password-dialog opening/Escape dismissal. OCR verifies outcome text and the last row above the footer; sampled pixels reject blank captures. Width constraints, persistent selection on hover, neutral-icon contrast, and accessible button names were tightened in the confirmation pass. The image-read tool still omits attachments for the available model: these are structural/interaction checks, **not a visual aesthetic approval**. Human review of the captures remains necessary. macOS/Linux visual behavior was not exercised.
