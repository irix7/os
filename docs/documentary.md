# The Logical Computer

[The Logical Computer](https://irix7.github.io/os/film/) is a 5 minute 38
second animated documentary about the difference between a regular operating
system and IRIX 7. It is built for the web: one static page, twelve chapters,
no audio and no external media.

The film is an argument, not a product tour. A regular OS governs one machine:
one kernel owns the CPU, memory, devices and process table, and scaling out
means building a distributed system above it. IRIX 7 starts from the opposite
case. Objects, capabilities, homes and the fabric are the operating system's
native vocabulary, so a second node adds capacity instead of adding a second
system to manage.

## Chapters

| Chapter | Subject | Length |
| --- | --- | --- |
| 01 | One machine. Then one computer. | 0:20 |
| 02 | A government for one machine | 0:28 |
| 03 | One logical computer | 0:30 |
| 04 | Everything is an object | 0:30 |
| 05 | Authority is a capability | 0:28 |
| 06 | Homes, not directories | 0:32 |
| 07 | One asynchronous fabric | 0:28 |
| 08 | Failure is a state | 0:30 |
| 09 | Compatibility is native | 0:28 |
| 10 | The 1994 machine | 0:28 |
| 11 | Four hard gates | 0:30 |
| 12 | The point | 0:26 |

The film follows the November 2026 plan. M1 and M2 are done, M2 on two real
kernels. M3 is active: the first shard slice runs on two real kernels and the
film does not claim eight-node convergence. M4 is still ahead. M5 and M9 are
active, M6 to M8 are exploring, and M10 is the real-Indy gate. The
[roadmap tree](roadmap-tree.md) carries the current status of every node; the
film records the shape of the argument rather than a moving snapshot of the
counts.

## Watching it

- **Chapters** opens the chapter grid. Selecting a chapter seeks to its first
  frame and plays from there. The grid also pauses the film.
- **Transcript** opens the full text of every chapter and pauses playback.
- The scrubber seeks. The time readout, chapter name and captions follow.
- Deep links use `#chapter-1` through `#chapter-12`.
- Keyboard: `Space` or `K` plays and pauses, `Left` and `Right` seek ten
  seconds, `Up` and `Down` move between chapters, `C` opens the chapters, `T`
  opens the transcript, `F` toggles fullscreen and `Escape` closes a panel or
  pauses.

There is no audio track. Every line is a caption, and the transcript repeats
all of them in reading order. With `prefers-reduced-motion` set, the film runs
as a captioned slideshow: the scene transitions, counters, starfield and
tilt effects stop, and the timeline still works.

## Production

The film is hand-written HTML, CSS, SVG and JavaScript with no build step. The
typefaces are bundled: Fraunces, Space Grotesk and IBM Plex Mono, all under the
SIL Open Font License. Nothing is fetched at runtime, so the page works from a
local checkout and leaves no network trace beyond the page itself.

Source lives in [`film/`](https://github.com/irix7/os/tree/main/film). To
preview it, serve the repository root and open `/film/`:

```sh
python3 -m http.server 8000
```

The Docs workflow copies `film/` to `book/film/` before the Pages upload, so
the film is published beside the documentation and the roadmap tree at
`/os/film/`.
