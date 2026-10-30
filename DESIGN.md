# Siphon Player design

The approved visual system for the native app. Direction **D (Hybrid)** was
chosen by the owner on 2026-10-30 from four interactive mockups (A Ledger,
B Stage, C Workbench, D Hybrid). Mockups are review evidence and are not in
this repository. `PRODUCT.md` holds the product decisions; this file holds
how the app looks and moves. `docs/_reference/siphon-frontend-handoff.md`
holds the frontend's status and what it needs from the backend.

## Shape

- **Top bar** (44 px): nested-U mark and "Siphon", one search field in the
  middle ("Search, or paste a Spotify, SoundCloud or YouTube link",
  Ctrl K), helper status dot, Windows caption buttons.
- **Floating sidebar** (default): a glass card that slides out when the
  pointer reaches the left edge (16 px hot zone with a visible handle that
  grows as the pointer arrives) and hides 350 ms after the pointer leaves.
  Ctrl B or the panel button docks it as a card beside the content (232 px,
  8 px from the window edge). At narrow widths the docked card becomes a
  60 px icon rail. Contents: Home, Group (with unread badge), then Library
  playlists with artwork, kind and count.
- **Content**: Home, playlist/album/artist/search lists, Group feed.
- **Player island**: a floating glass bar centred at the bottom of the
  content (up to 760 px wide, 68 px tall, 16 px from the bottom). Cover,
  coloured title and artist, the next three queue songs as small stacked
  tiles, previous / play / next, elapsed and total time, BPM and key,
  share, open. A 2 px progress line runs along its bottom edge.
- **Now playing**: the island grows into a full-content view (the sidebar
  stays reachable). Large cover, title, artist and album, tool pills
  (Download, BPM/key, Credits, last-used playlist), transport, and a side
  panel with Up next / Lyrics / Credits. Artwork tint and grain sit behind
  it in both themes. Esc or the chevron shrinks it back into the island.

## Tokens

### Colour

Neutral zinc ramp. The only hues in the chrome are the source colours on
titles and the semantic warning colour.

| Token | Dark | Light |
| --- | --- | --- |
| window | `#0B0B0C` | `#F6F6F7` |
| panel | `#121214` | `#FFFFFF` |
| raised | `#17171A` | `#EFEFF1` |
| hover | white 5.5% | black 4.5% |
| active | white 10% | black 8% |
| line | white 7% | black 7% |
| line strong | white 11% | black 10% |
| text | `#EDEDEF` | `#09090B` |
| secondary | `#A1A1AA` | `#52525B` |
| dim | `#8A8A93` | `#6B6B73` |
| inverse (primary button) | `#FAFAFA` on `#09090B` | `#18181B` on `#FAFAFA` |
| warning | `#F2B85C` | `#9A6400` |
| glass fill | `#18181C` at 72% | white at 76% |

Source colours, on song titles only, never as chips:

| Source | Dark | Light | On artwork (now playing, share hero) |
| --- | --- | --- | --- |
| Spotify | `#1ED760` | `#097F3A` | `#3EE27A` |
| SoundCloud | `#FF8A1F` | `#B84D00` | `#FF9A3D` |
| YouTube | `#FF4848` | `#CC1F26` | `#FF5A5A` |

Orange leans yellow and red leans crimson so they never read as the same
colour. An unavailable song's title drops to dim.

### Type

Geist for the interface and Geist Mono for figures (BPM, key, times, ISRC,
counts). Weights 400 and 500; 600 only for page titles. Sizes: 11 (meta),
12 (secondary), 13 (body), 15 (section), 17 to 18 (section heads), 26 to
30 (page and now-playing titles), 42 (playlist header).

### Space and shape

4 px grid. Row height 56 (list), 42 (sidebar item), 34 (nav item). Square
row tools 30 px with 16 px icons and 2 px gaps. Radii: controls 8, popovers
and cards 12 to 14, island 22, covers 8 (rows) to 18 (now playing).

## Components

- **Source title**: one line, elided, coloured by source. Never a badge.
- **Row tools**, always in this order and visible on hover, focus, the
  playing row or while a download runs: Download, BPM/key, Credits, Add to
  last-used playlist. Every one has a tooltip that states the result, and a
  disabled one says why ("Download needs the Siphon helper").
  - Download: click downloads with the remembered format; right-click opens
    format (WAV, MP3 320, FLAC), stems and DAW naming. While running the
    button is a progress ring with a stop square; click to cancel.
  - BPM/key: click copies "124 BPM · A minor". Pending shows "Analysing…".
  - Credits: a popover on the row. It never changes what is playing.
  - Add to last-used playlist: right-click picks another playlist.
- **Toasts** rise out of the island and leave after 2.6 s. They carry at
  most one action (Undo, Show, Open).
- **Banners** at the top of the content for a service that is down or the
  helper being off, each with one action (Retry, Start helper).
- **States** every list and feed must draw: loading (static skeleton),
  empty (one sentence on how to fill it), error (partial: other sources
  stay), unavailable row (dimmed, reason in the subtitle).

## Motion

Every animation answers something the user did. Nothing loops, nothing
moves while music simply plays, and a finished animation costs nothing.

One curve, `cubic-bezier(0.22, 1, 0.36, 1)` (ease-out quint), and these
durations: feedback 120 to 180 ms, popovers 160 to 220 ms, entrances and
song changes 300 to 500 ms, stagger 30 to 50 ms over at most six items.

| Trigger | Motion | Why |
| --- | --- | --- |
| Play a song from a list | Island: current song, then its queue tiles, swipe out left one after another; new song and queue slide in from the right. Up next does the same. | You replaced what was playing; you see it go. |
| Next | First queue tile slides and grows into the cover; tiles shift one place; a new tile enters at the end. Up next rows slide up one slot. | The queue advanced by one. |
| Previous | Reverse of next: the cover shrinks back into the first tile. | It went back into the queue. |
| Next / previous with nothing there | The arrow nudges 4 px. | The press was received but there is nowhere to go. |
| Queue tile click | Same as next, jumping several. | Skip ahead. |
| Open now playing | The island's rectangle grows to fill the content (clip and radius interpolate), cover rises, title types in letter by letter, tool pills stagger. Close reverses into the island. | One object, two sizes. |
| Add to playlist | Cover thumbnail flies on an arc to the playlist in the sidebar (the floating sidebar peeks to catch it), its art pulses, count rolls up, toast with Undo. | Shows where the song went. |
| Share | Thumbnail flies to Group; badge bumps; your post drops into the feed. | Same. |
| Download | Arrow drops, ring fills with real progress, ring bursts into a check, toast. Cancel unwinds the ring. | Progress you can stop. |
| Copy BPM/key | Icon becomes a check; the value floats up 20 px and fades. | Confirms what was copied. |
| Play an unavailable song | Cover shakes 4 px. | Refusal without a dialog. |
| Heart | Icon pops, six dots burst, count rolls. | Confirms the tap. |
| Row play | A highlight sweeps across the row once. | Confirms which row started. |
| Hover row | Tools slide in 6 px with a 20 ms stagger. | Tools are near but quiet. |
| Navigate | Sidebar highlight slides to the new item; page sections rise 10 px with a stagger. | Where you are and what changed. |
| Tabs, format choice, feed filter | Selection pill slides; content enters from the side you moved toward. | Direction of travel. |
| Retry / Start helper | Button fills while waiting; restored Spotify rows slide back in, or row tools wake across the list and the helper dot ripples once. | The fix worked. |
| Paste a link in search | Field widens, panel unfolds, link types in, a skeleton resolves into the song. | Recognised and resolved. |
| Loading finishes | Skeleton is replaced by content rising in. | No shimmer while waiting. |

When Windows asks for less motion (`src/motion.rs`), every one of these
shows its end state at once.

## Native implementation notes

All of the above uses only what egui draws cheaply: translation and scale
(layer transforms), alpha, clip rectangles and interpolated rectangles.
No blur, no rotated text, no shaders per frame.

- **Glass**: the current artwork is blurred and down-scaled once on the
  image worker when the song changes, cached with the art, and painted
  behind glass panels at low alpha with a 1 px top highlight and a hairline
  border. No live backdrop blur.
- **Grain**: one small tiling texture uploaded once, painted at low alpha.
- **Island to now playing**: one rectangle and radius interpolated between
  the island and the content area; contents fade and rise inside its clip.
- **Swipes**: the island keeps a snapshot of the outgoing song and queue
  for the length of the transition and paints it with an offset and alpha;
  the snapshot is dropped when the transition ends.
- **Flying thumbnail**: a textured quad on the top layer between two rects
  egui already knows (the button and the sidebar item).
- **Letter reveal**: lay the galley out once, then paint it with per-glyph
  alpha and offset. Only for the now-playing title and the newest share.
- **Repaint rule**: an animation requests repaints only while it runs.
  Static and hidden content never repaints. The progress line moves a few
  pixels per second, so it repaints a few times per second, not per frame.
- **Budget**: during an animation a frame should stay under a few
  milliseconds of CPU. The demo build gets frame-time logging to check it.

## What is built (30 October, evening)

The sections above are the target. This is what the native app does
today, on `claude/focused-rubin-pxfk32`; the handoff in
`docs/_reference/siphon-frontend-handoff.md` has the commits.

| Piece | State |
| --- | --- |
| Palette, source-coloured titles, no chips | Built, dark and light. |
| Top bar | The existing Spotifast bar in the new palette. The mark, the link-aware search and the helper dot are not built. |
| Floating sidebar | Built: docked card, or edge handle and slide-out when hidden. The narrow rail is the existing compact sidebar. Group item and badge are not built. |
| Player island | Built with every existing control, the three queue tiles and the Replace, Next and Previous swipes. It is up to 1,120 px wide (the existing controls need the room), not 760. BPM and key, share and the bottom progress line are not on it. |
| Row tools | Built in song lists (not the compact queue rows): Download, BPM/key (off, with its reason), Credits popover, Add to last-used playlist (this session and account only; right-click picks another playlist). Download formats, stems and DAW naming on right-click are not built. |
| Now playing | Built: grows out of the island, title wipe, pills, Up next, Lyrics and Credits tabs with a sliding selection. No grain, no transport of its own (the island stays below it). |
| Home | The group's newest shares first, with join, error, loading and empty lines; Spotify's shelves after. New releases and blended For you need backend data first. |
| Group feed and composer | The backend's friends panel; not restyled. |
| Toasts, banners | Existing ones in the new palette; not rebuilt. |
| Motion built | Island swipes, Now playing grow and title wipe, tab selection slide, row tool reveal, sidebar slide and handle, group shares arriving in turn, group heart pop. All respect reduced motion. |
| Motion not built | Flying thumbnail, download burst, BPM copy float, refusal shake, nudge, row play sweep, search link unfold. |
| Glass and grain | Not built: panels are flat fills. |
| Geist | Not bundled; Inter remains. |
