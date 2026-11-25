# Example wallpapers

Three web wallpapers and one in Diorama's own format, made for Diorama. Use them, change them, or start your own from them. They are covered by the project's licence (MIT or Apache-2.0).

| | |
|---|---|
| ![Paper Valley](paper-valley/preview.png) | **Paper Valley**: paper-cut hills under a night sky. Fireflies drift between the layers and brighten with music; the hills shift with your cursor. |
| ![Aurora](aurora/preview.png) | **Aurora**: northern lights ripple over snowy peaks and a still lake, and swell with music. |
| ![Dunes](dunes/preview.png) | **Dunes**: a banded sunset over paper-cut dunes. Birds cross the sky, and the sun pulses gently with music. |
| ![Ridges](ridges/preview.png) | **Ridges**: mountain ridges at dusk that shift with your cursor, and words of your choice. In Diorama's own format (not in a release yet). |

## Using them

In Diorama, open Settings → Library folders and add this `examples/wallpapers` folder (or a copy of it). The wallpapers then appear in the library. Each one has settings (colours, counts, speed) in its details panel.

## How they are made

The web wallpapers are each a single `index.html` with a `<canvas>` and no dependencies. They use the standard web wallpaper interface that Diorama shares with Wallpaper Engine:

- `window.wallpaperPropertyListener.applyUserProperties` receives the settings declared in `project.json`;
- `window.wallpaperRegisterAudioListener` receives 128 audio levels (64 per channel) about 30 times a second;
- mouse events give the cursor position.

Options in the address, useful for previews and recordings:

- `?t=<seconds>` draws one still frame at that time;
- `&loop=<seconds>` makes every motion repeat after that many seconds;
- `&demo` (Paper Valley only) moves a pretend cursor and plays pretend music.

For example, `index.html?t=3&loop=8&demo` is the frame used for Paper Valley's preview.

*Ridges* is a document and a few images: see [the format](../../docs/formats/diorama.md).
