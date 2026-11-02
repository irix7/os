# The Logical Computer — film source

A 5 minute 58 second documentary about IRIX 7, built as a HyperFrames
composition and played in the browser. No video render; the timeline is the
film.

The page follows one frame from a 1994 SGI Indy through the object space,
capabilities, homes, the fabric and a failed node, then back to the machine.
Twelve chapters, silent, captioned, with a full transcript.

## Run it

From the repository root:

```sh
python3 -m http.server 8000
```

Open <http://127.0.0.1:8000/film/>. The page needs a server rather than
`file://` because the player fetches the composition and its scene files.

To work on the composition alone, use the HyperFrames player:

```sh
cd film/hyperframes
npx hyperframes play
```

The CLI injects its runtime into the composition for preview. The static page
loads the vendored `player.js` and the composition loads `hyperframes/runtime.js`
through the `window.__hyperframes` check in `hyperframes/index.html`.

## Files

| Path | Purpose |
| --- | --- |
| `index.html` | Page chrome: poster, player, controls, chapter menu, transcript |
| `film.css` | Page layout and the cinema surround |
| `film.js` | Player controls, chapter and transcript data, deep links |
| `player.js` | Vendored HyperFrames web component |
| `hyperframes/index.html` | Root composition: 12 chapter hosts, captions, 5:58 duration |
| `hyperframes/compositions/` | One scene file per chapter |
| `hyperframes/assets/film.css` | Shared film styles and layout |
| `hyperframes/runtime.js` | Vendored HyperFrames runtime |
| `assets/poster.png` | Page poster, 1920 × 1080 |
| `assets/og.png` | Open Graph card, 1200 × 630 |

## Editing a chapter

Each chapter is a `<section>` host in `hyperframes/index.html`:

```html
<div id="scene-04" data-composition-id="scene-04"
     data-composition-src="compositions/scene-04.html"
     data-start="86" data-duration="30" data-title="Everything gets a name"></div>
```

The scene file under `hyperframes/compositions/` owns its own GSAP timeline,
registered as `window.__timelines["scene-04"]`. Change the host's
`data-duration`, `data-start` and `data-title` together with the scene's
timeline. Captions are `.caption` clips in the root; `film.js` reads the same
attributes to build the chapter menu and transcript.

Scene files use the shared `hyperframes/assets/film.css` and reference local
media with `assets/...` paths. Every timed element needs `data-start`,
`data-duration` and a stable `id`.

## Controls

`Space` or `K` plays and pauses, `Left` and `Right` seek ten seconds, `Up` and
`Down` move between chapters, `C` opens the chapters, `T` opens the transcript,
`F` toggles fullscreen, and `Escape` closes a panel or pauses. Deep links use
`#chapter-1` to `#chapter-12`.

The film is silent; the captions are part of the frame and the transcript
repeats them. The page pauses the film when a panel opens.

## Publishing

`.github/workflows/docs.yml` copies this directory into `book/film` before the
GitHub Pages upload. The published URL is
<https://irix7.github.io/os/film/>.
