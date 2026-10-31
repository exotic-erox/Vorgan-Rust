# Changelog

## 0.2.1

### Fixed
- Image previews no longer break the screen. The sixel encoder wrote an escape
  sequence inside the image data (terminals ended the image there) and sent
  colour values as 0-255 instead of percentages.
- The "border behind some parts" of an image: pane, fill and compositing used
  three different colours. Pictures now sit on the pane colour, and transparent
  or letterboxed areas are left unpainted.
- Images keep their aspect ratio (fitted and centred, not stretched) and no
  longer cover the header and meta rows.
- Leftover pictures are blanked when you move to a text file; the picture
  repaints after a dialog closes or you leave the graph.
- Mouse wheel scrolls the pane under the pointer like the python version (the
  cursor stays put). The preview pane now scrolls; lists and preview get a
  scrollbar.
- Every file showed `l` (symlink) in its permissions.
- Text previews showed their meta rows twice; folders no longer show meta.

### Changed
- Graph particles: dots are allotted by bubble size, carriers stream along the
  edges with a trail, hover lights a bubble's neighbourhood, pinned bubbles
  glow gold, pinning gives a small burst.
- Sixel encoder: 252-colour palette with Floyd-Steinberg dithering, run-length
  packing, sixel scrolling disabled while painting.

### Tests
- Sixel round-trip decoder, particle, permission and history tests (19 total).
