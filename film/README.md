# The Logical Computer — film source

A 5 minute 38 second animated documentary about IRIX 7, built as one static
page. Twelve chapters, no audio, no build step, no runtime network requests.

Open `/film/` from a server rooted at the repository:

```sh
python3 -m http.server 8000
```

Then visit <http://127.0.0.1:8000/film/>. The page needs a server rather than
`file://` because the fonts are loaded as same-origin assets.

## Files

| Path | Purpose |
| --- | --- |
| `index.html` | Poster, twelve scenes, caption bar, player, chapter menu, transcript |
| `film.css` | Layout, type, colour, every animation, responsive and reduced-motion rules |
| `film.js` | Timeline, captions, counters, controls, deep links, pointer tilt |
| `assets/fonts/` | Fraunces, Space Grotesk and IBM Plex Mono, with OFL licences |
| `assets/og.png` | Open Graph card, 1200 × 630 |

## Editing the timeline

Each chapter is a `<section class="scene">` in `index.html`. The player reads
its duration, chapter number and title from data attributes:

```html
<section class="scene scene-homes" data-scene="6" data-duration="32"
         data-chapter="06" data-title="Homes, not directories">
```

Captions are `<p class="cue" data-at="6">` children of that section. `data-at`
is the offset in seconds from the start of the chapter. Chapter cards,
scrubber segments and transcript entries are generated from the same markup,
so adding a cue updates all three.

Scene-local animation is CSS only. A scene receives `.is-active` while it is
on screen; counters restart with it, and `prefers-reduced-motion` switches the
whole film to a slideshow.

## Controls

`Space` or `K` plays and pauses, `Left` and `Right` seek ten seconds, `Up` and
`Down` move between chapters, `C` opens the chapters, `T` opens the transcript,
`F` toggles fullscreen, and `Escape` closes a panel or pauses. Deep links use
`#chapter-1` to `#chapter-12`.

## Publishing

`.github/workflows/docs.yml` copies this directory into `book/film` before the
GitHub Pages upload. The published URL is
<https://irix7.github.io/os/film/>.
