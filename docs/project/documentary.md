# The Logical Computer

[The Logical Computer](https://irix7.github.io/os/film/) is a 5 minute 58 second
documentary about IRIX 7. It follows one frame — a 1994 test pattern — from the
framebuffer of an SGI Indy, through the object space, the capability model, the
homes, the fabric and a failed node, and back to the same screen.

The film is built as a HyperFrames composition and played in the browser. There
is no video file and no render step: the page loads the composition, the player
drives its timeline, and the same source is the film.

## Chapters

| Chapter | Subject | Length |
| --- | --- | --- |
| 01 | A frame on a desk | 0:24 |
| 02 | The machine has a border | 0:32 |
| 03 | One computer, many boxes | 0:30 |
| 04 | Everything gets a name | 0:30 |
| 05 | Who gets to say yes | 0:30 |
| 06 | Where things live | 0:30 |
| 07 | A call across the room | 0:30 |
| 08 | Pulling the plug | 0:32 |
| 09 | Old programs, new machine | 0:32 |
| 10 | The 1994 machine | 0:32 |
| 11 | What actually runs | 0:30 |
| 12 | The network is the computer | 0:26 |

The first half explains the design through the frame: it becomes an object with
a name and a home; a capability says who may read it; a call carries it across
the mesh. The second half tests that story against the machine: the home node is
unplugged, the frame moves, an old IRIX program draws it, and the same Indy
boots v0.1.3.

The status in the film is the November 2026 status. One node works on iris. Two
nodes work on two real kernels. Eight nodes are in progress, with the first
slice running on two real kernels. The 128-node target is planned. Native
compatibility and the runtime are active; display, X11 and XFS are exploratory;
real Indy hardware is the next step. The [roadmap tree](roadmap-tree.md) carries
the current status of every node.

## Watching it

- **Chapters** opens the chapter grid. Selecting a chapter seeks to its start
  and plays from there. The grid pauses the film.
- **Transcript** opens every line in reading order and pauses playback.
- The scrubber seeks. The chapter name and time follow.
- Deep links use `#chapter-1` through `#chapter-12`.
- Keyboard: `Space` or `K` plays and pauses, `Left` and `Right` seek ten
  seconds, `Up` and `Down` move between chapters, `C` opens the chapters, `T`
  opens the transcript, `F` toggles fullscreen and `Escape` closes a panel or
  pauses.

The film is silent. Every line is a caption inside the frame, and the transcript
repeats all of them. With `prefers-reduced-motion` set, the page keeps the film
paused until you start it and shows a note that the motion is part of the film.

## Production

The composition is hand-written HTML, CSS, SVG and GSAP under
[`film/hyperframes`](https://github.com/irix7/os/tree/main/film/hyperframes).
Each chapter is one scene file. The root composition holds the chapter order,
the captions and the 5:58 timeline. The page around it adds the chapter menu,
transcript, custom controls and deep links.

The typefaces are bundled: Space Grotesk and IBM Plex Mono, both under the SIL
Open Font License. All assets are local; the page makes no network requests
beyond its own files. The published page is a static copy of `film/` in the
mdBook output at `/os/film/`.
