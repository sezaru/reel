# reel — the guide

reel makes videos from TypeScript scene files. A scene file says what the video is (size,
length, fonts, sound) and how to draw any one frame; reel runs that function for every frame
on all cores, draws the result on the GPU (anti-aliased shapes, real perspective, Gaussian
blur, true motion blur) and encodes it with ffmpeg. There is no browser and no React: a
frame is a function of its number.

    reel docs            this guide
    reel docs api        every function and option, with types (reel.d.ts)
    reel docs shaders    custom WGSL shaders
    reel docs examples   the example scenes

## Start

    reel init my-video           # reel.d.ts (types), tsconfig.json, scene.ts
    reel preview my-video/scene.ts       # a window that plays it; reloads on save
    reel check my-video/scene.ts         # runs every frame, reports errors with file:line
    reel still my-video/scene.ts --at 2s -o frame.png
    reel sheet my-video/scene.ts -o sheet.png     # 24 labelled frames in one image
    reel render my-video/scene.ts -o out.mp4

Times on the command line (`--at`, `--from`, `--to`, `--every`) are a frame (`120`),
seconds (`2.5s`), minutes:seconds (`1:05.5`), `end`, or a marker's name. `--set key=value`
passes values to the scene (`params`).

## A scene file

```ts
import { video, seconds, spring, lerp, Easing } from "reel";

export default video({ duration: "4s", background: "#F4ECE0" }, (cv, f) => {
  const p = spring({ frame: f - 10, config: { damping: 14, stiffness: 120 } });
  cv.group({ opacity: p, y: (1 - p) * 40 }, () => {
    cv.text("Hello", 960, 420, { size: 120, weight: 800, color: "#3A2A1E", anchor: "center" });
  });
  cv.path("M 660 600 L 1260 600", { stroke: "#C2562E", width: 6, progress: lerp(f, [30, 70], [0, 1], Easing.inOut(Easing.cubic)) });
});
```

- The default export must be `video(config, draw)`. `config.duration` is required (frames,
  or `"4s"`, `"1:30"`); the rest has defaults: 1920×1080, 60 fps, black background,
  motion blur on.
- `draw(cv, f)` is called for frame `f` (and, with motion blur, for fractional frames
  between `f` and `f + 0.5`). **It must depend only on `f`**: frames are drawn out of
  order, on many threads, several times each. No state carried between calls, no
  `Math.random()` (use `random(seed)`), no `Date`. Caching pure results in module-level
  maps is fine.
- Module-level code runs once per thread: load data, build tables, precompute keyframes
  there.
- Coordinates are design pixels: (0, 0) top-left, y down, the config's width × height,
  whatever size it is rendered at (`--scale 0.5` draws the same scene at half size).
  Angles are degrees, times are frames, letter-spacing is in em.
- Relative file paths (images, fonts, JSON, shaders, audio) are relative to the scene
  file. Imports are relative to the importing file and need the extension
  (`import { title } from "./lib/type.ts"`); `import data from "./data.json"` gives the
  JSON. `"reel"` is the only package: there is no npm, no Node, no DOM. A module finds
  its own files with `import.meta.dirname` (and `import.meta.filename`, `import.meta.url`),
  as in Node: a library can ship shaders or images next to itself.
- TypeScript types are stripped, not checked (run `tsc --noEmit` yourself if you have it;
  `reel init` writes the tsconfig and types it needs). Features that need compiling away
  don't work: no `enum`, `namespace`, or constructor parameter properties.
- `console.log` prints to the terminal, with the frame.

## Drawing

Everything is drawn in order, later over earlier, onto the current transform.

**Boxes** — `cv.rect(x, y, w, h, look)`. A look is CSS-like:

```ts
cv.rect(100, 100, 600, 300, {
  fill: "#FFFCF7",                         // or gradient: { angle: 160, stops: [["#fff", 0], ["#eee", 1]] }
  radius: 22,                              // or [tl, tr, br, bl]
  border: { width: 1.5, color: "#EADFCF" },// inside the box (border-box)
  shadow: [{ y: 26, blur: 60, color: "rgba(120, 80, 40, 0.18)" }, { y: 4, blur: 12, color: "rgba(120, 80, 40, 0.1)" }],
  inset: { blur: 12, color: "#0002" },     // inner shadows, up to 2
  opacity: 1, feather: 0, blend: "normal", // "multiply" darkens what is under it
});
cv.circle(cx, cy, r, "#C2562F");           // or a look
```

Colours: `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"`, `"rgb(…)"`, `"rgba(r, g, b, a)"`,
`"transparent"`, or `[r, g, b, a]` in 0…1 (`rgba()`, `alpha()`, `mixColor()` make them).

**Images** — `cv.image("shots/home.jpg", x, y, w, h, { fit: "cover", radius: 24, shadow: … })`.
JPEG and PNG; `fit` is `"fill"` (stretch, default), `"cover"` or `"contain"`; `uv` shows
part of the image (`[x, y, w, h]` in fractions). Images are decoded at the size they are
drawn, so a 4K screenshot drawn small costs little. An image sequence (a screen
recording's frames) is a list of files: `const shots = frames("capture/frames")`, then
`cv.image(shots[Math.round(f)], …)`.

**Text** — `cv.text(s, x, y, style)` puts the top-left of the text's *line box* at (x, y)
(as a browser lays out a line of text; the baseline is lower). `anchor: "center"` makes x
the centre (or `"end"`: the right edge). Give a `width` to wrap the text, and `align` to
place the lines in it (`balance: true` evens their lengths, as CSS `text-wrap: balance`).
It returns `{ width, height, lines, baseline }`; `measureText(s, style)` measures without
drawing. Spans colour parts of one text: `cv.text([{ text: "Tap " }, { text: "Save",
color: "#C2562F" }], …)` (a caption highlighting the word being said).

```ts
cv.text("Every record, in one place", 960, 640, {
  size: 58, weight: 700, color: "#33291F", letterSpacing: -0.02,
  width: 1200, align: "center", anchor: "center",
  font: "Hanken Grotesk",       // a family from config.fonts, or "sans-serif", "serif", "monospace"
});
```

Fonts: list the files in `config.fonts` (globs allowed) and name the default `font`. Any
script works: a glyph your fonts lack comes from Noto (CJK, Devanagari, Arabic, emoji…).
Set `config.lang` (`"ja"`, `"pt-BR"`) for the right CJK forms and line breaking.

**Paths** — SVG path data, stroked (round caps and joins) or filled:

```ts
cv.path("M 100 500 C 400 300 700 700 1000 500", { stroke: "#C2562F", width: 6, progress: p });
cv.path(d, { fill: "#fff" });
const pth = path(d);  pth.length;  pth.pointAt(pth.length * p);   // a dot riding the line
```

`progress` (0…1) draws the line on; `from`/`to` (px along it) draw a stretch of it.
`fillRule: "evenodd"` makes shapes inside shapes holes (a dimmed frame with cut-outs).

**Layers** — `cv.group({ opacity, blur, x, y, mask, shadow }, () => { … })` draws its
content into a layer that is then faded, blurred, moved, masked (an ellipse, or a linear
gradient: CSS `mask-image`) or given a drop shadow (CSS `filter: drop-shadow()`, a glow
with no offset) as one (so overlapping parts don't show through each other). `cv.fade(opacity, draw)` is the common case. Layers cost a
little, blur costs more: don't nest dozens.

**Transforms** — `cv.translate(x, y, draw)`, `cv.scale(s, draw, [ox, oy])`,
`cv.rotate(deg, draw, [ox, oy])`, `cv.transform(matrix, draw)`. `matrix.perspective({
rotateX, rotateY, origin, distance })` tilts a flat thing in 3D (a phone turning), with
real perspective: shapes stay sharp and anti-aliased under it.

**Clipping** — `cv.clip([x, y, w, h], radius, draw)` clips everything drawn inside to a
rounded rect (in the coordinates it was set in, whatever transforms come inside). A clip
inside a clip intersects with it.

**Shaders** — `cv.shader("paper.wgsl", { params: [...] })` fills a rect (the whole frame
by default) with your own WGSL: procedural backgrounds, noise, grain, light. See
`reel docs shaders`.

**3D models** — `cv.model("phone.glb", { rect, screen: "shot.png", yaw: 18, pitch: -6 })`
draws a glTF device in studio light with an image on its screen, fitted into `rect`. It
returns the device's box and its screen's corners (`matrix.quad` draws on the screen in
perspective). See `reel docs models`.

## Layout

For boxes whose size depends on their text (cards, chips, buttons, labels in any
language), build a flexbox tree and let reel measure it:

```ts
import { row, col, text, box, measure } from "reel";

const chip = row(
  [box(40, 40, (cv, [x, y, w, h]) => cv.circle(x + w / 2, y + h / 2, w / 2, "#C2562F")), text("Private", { size: 40, weight: 700 })],
  { align: "center", gap: 14, padding: [12, 28], look: { fill: "#FFFCF7", radius: 40, border: { width: 1.5, color: "#EADFCF" } } },
);
const { width } = measure(chip);         // centre it: x = 960 - width / 2
cv.layout(chip, 960 - width / 2, 700);
```

`row`/`col` take `padding` (one number, `[vertical, horizontal]`, or `[t, r, b, l]`),
`gap`, `align` (cross axis), `justify` (main axis), `width`/`height`/`min…`/`maxWidth`,
`grow`, and a `look` (its border takes room, as in CSS). `text()` wraps when its box is
narrower than it. `box(w, h, draw)` reserves room and draws whatever you like there.

## Animation

The timing functions are Remotion's, with the same semantics, so Remotion code ports
directly:

```ts
interpolate(f, [0, 30], [0, 1]);                                   // extends past the ends!
interpolate(f, [0, 30], [0, 1], { extrapolateRight: "clamp" });
lerp(f, [0, 30], [0, 1], Easing.out(Easing.cubic));                // clamped at both ends
spring({ frame: f - 20, config: { damping: 14, stiffness: 120 } }); // 0 → 1, settles
spring({ frame: f, durationInFrames: 30 });                         // stretched to 30 frames
```

Patterns:
- **Enter at a frame**: `const p = spring({ frame: f - at })`, then use `p` for opacity,
  a rise (`y: (1 - p) * 30`), a blur (`blur: (1 - p) * 8`), a scale.
- **Stagger**: `items.forEach((it, i) => { const p = spring({ frame: f - (at + i * 5) }); … })`.
- **Exit**: `Math.min(enter, lerp(f, [out, out + 12], [1, 0]))`.
- **Sequences**: `cv.during(from, to, (local) => { … })` draws only in that range, with
  the frame counted from its start (Remotion's `<Sequence>`). Keep scenes in their own
  modules exporting `draw(cv, f)`, and pick them by frame in the main file.
- **Beats**: put the cue frames in one place (an object of named frames, or computed
  from the music's tempo), and every scene reads them: retiming then means one edit.
- **Markers**: `config.markers` names frames; `reel still --at name`, and ↑/↓ in the
  preview jump between them.
- **Camera moves**: interpolate the numbers of a transform (zoom, pan, tilt) between
  keyframes with an easing, and draw the scene inside that transform.

Motion blur is automatic: reel compares a frame with the moment half a frame later and,
where things moved, averages several sub-frames. Nothing to do in the scene, except the
rule above (draw depends only on f). `motionBlur: false` in the config, or
`--no-motion-blur`, turns it off.

## Sound

```ts
audio: [
  { src: "music.mp3", volume: -7, fadeIn: 0.6, fadeOut: 2.5, duck: true },
  { src: "voice/line1.wav", at: seconds(1.2), voice: true },
  { src: "voice/line2.wav", at: seconds(6.0), voice: true },
],
loudness: -14,
```

Tracks start at `at` (a frame), are trimmed with `from`/`duration` (seconds), and mixed
when rendering; `duck: true` tracks dip while `voice: true` tracks speak; `loudness`
normalises the mix (LUFS). The preview is silent.

## Checking your work (do this; you can't watch the video)

1. `reel check scene.ts` after every change: it runs every frame and reports exceptions
   with the frames and file:line, shaders that don't compile, missing files. It needs no
   GPU and takes seconds.
2. `reel sheet scene.ts -o sheet.png` (or `--from 4s --to 8s --every 10`) and look at the
   image: layout, overlaps, text that runs off the frame, things that never appear.
3. `reel still scene.ts --at 2.5s,3s,end -o stills/` at the moments that matter: every
   entrance, the middle of each transition, the last frame. Look at them at full size.
4. Render a draft: `reel render scene.ts --scale 0.5 --no-motion-blur -o draft.mp4`; the
   full render once it's right. On an NVIDIA GPU, `--encoder nvenc` encodes several
   times faster.

## Images (store screenshots, posters)

A still is a frame: `reel still scene.ts --at 0 -o shot.png`. A set of images is one scene
with a frame per image and a marker naming each; `reel still scene.ts --at markers -o
final/` writes `final/NAME.png` for every marker. Set `motionBlur: false` in such a scene.
`reel docs models` has the whole pattern, with store sizes.

## Rendering

`reel render` draws on the GPU (Vulkan) and encodes H.264 (x264 `slow`, CRF 16 by default,
BT.709, `+faststart`), with the soundtrack mixed in. A 1080p60 minute renders in well under
a minute on an ordinary GPU; x264 is usually the slowest part (`--encoder nvenc`, or
`--crf`/`--preset` to trade size and time). `--from`/`--to` render a part. `--workers`
sets the threads running the scene (default: cores − 2). `--null` draws without encoding,
to time the scene itself.

## Porting from Remotion

| Remotion | reel |
|---|---|
| `useCurrentFrame()` | the `f` passed to draw |
| `useVideoConfig()` | `cv.width`, `cv.height`, `cv.fps` |
| `interpolate`, `spring`, `Easing` | the same, from "reel" |
| `<Sequence from={a} durationInFrames={n}>` | `cv.during(a, a + n, (local) => …)` |
| `<AbsoluteFill style={{background}}>` | `cv.rect(0, 0, cv.width, cv.height, { fill })` |
| a `<div>` with background, border-radius, box-shadow | `cv.rect(x, y, w, h, look)` |
| flexbox `<div>`s around text | `row`/`col`/`text` and `cv.layout` |
| `opacity`, `filter: blur()`, `transform: translateY()` on a wrapper | `cv.group({ opacity, blur, y }, …)` |
| `transform: scale() rotate()` | `cv.scale`, `cv.rotate`, `cv.transform(matrix…)` |
| `perspective` + `rotateX/Y` | `matrix.perspective(…)` |
| `overflow: hidden` + `border-radius` | `cv.clip(rect, radius, …)` |
| `<Img src>` / `<OffthreadVideo>` frames | `cv.image(path, …)` / an image sequence |
| SVG `<path>` + `strokeDasharray` draw-on | `cv.path(d, { stroke, progress })` |
| `<Audio>` | `config.audio` |

CSS lays things out for you; here you place them: compute positions from measured sizes
(`measureText`, `measure`) or use the layout tree.
