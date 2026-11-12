# Carrel brand mark

The Carrel mark is a sibling of the Nostos mark, in the way app icons in one
family share a style but not a shape. Shared with Nostos: the 1024 canvas, the
205 px corner radius, one flat white glyph with a heavy stroke, flat-cut ends and
an off-center inner counter, on one earthy tile color.

Its own shape: a round lowercase "c" followed by a text cursor, "c|". The cursor
stands where the next letter will be written. The counter sits 31 px right and
32 px up of the outer circle, so the stroke is thin at the top (66) and heavy at the
lower left (130), the same thin/heavy rhythm as the Nostos arch. Height and optical
center match the Nostos glyph.

| Token      | Hex       | Use                         |
|------------|-----------|-----------------------------|
| Terracotta | `#7D3B25` | tile background             |
| White      | `#FFFFFF` | the "c"                     |
| Amber      | `#E5A93B` | the cursor (Night accent)   |

Geometry (1024 canvas): outer circle (455, 516) r 240; counter (486, 484) r 142;
opening y 456 to 576 on the right; cursor x 745 to 811, y 276 to 756.

Files: `carrel-mark-primary.svg` (app icon, favicon), `carrel-mark-monochrome-dark.svg`
and `carrel-mark-monochrome-light.svg` (glyph and cursor in one color, no tile).

In the shell the mark is drawn with Cairo (`shell/src/ui.rs`, `ui::mark`) from
the same geometry, in the theme's text colour: dark ink on Paper, soft white on
Night. It is static and appears on the first-run screen and on Settings → About.
