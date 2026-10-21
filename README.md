# reel

Videos from TypeScript scene files, drawn on the GPU.

A scene file default-exports `video(config, draw)`, and `draw(cv, f)` draws frame `f` on a
canvas: boxes with CSS-like looks (gradients, borders, shadows), images, text, SVG paths,
flexbox layouts, layers with opacity, blur and masks, 3D perspective, clipping, and
custom WGSL shaders. reel runs the scene on every core, draws on the GPU (wgpu: Vulkan),
adds true motion blur where things move, mixes the soundtrack and encodes H.264 with
ffmpeg. The timing functions are Remotion's (`interpolate`, `spring`, `Easing`), frame for
frame, so Remotion compositions port directly.

```ts
import { video, spring } from "reel";

export default video({ duration: "3s", background: "#F4ECE0" }, (cv, f) => {
  const p = spring({ frame: f - 10 });
  cv.text("Hello", 960, 480, { size: 120, weight: 800, anchor: "center", opacity: p });
});
```

```
reel preview scene.ts     a window that plays it, reloading on save
reel check scene.ts       every frame run, errors reported with frame and file:line
reel sheet scene.ts       a contact sheet of labelled frames
reel still scene.ts --at 2s
reel render scene.ts -o out.mp4
reel docs                 the guide (also: docs api, docs shaders, docs examples)
reel init                 a new project: types for editors, tsconfig, a first scene
```

Speed: the 55-second Caremate store promo (1080p60, phone footage, motion blur) takes
341 s in Remotion. reel draws it at hundreds of frames a second; the whole render takes
about a minute with x264, or about 30 s with NVENC.

## Install

```
nix profile install github:sezaru/reel     # or: nix build, ./result/bin/reel
```

The package wraps ffmpeg, the Vulkan loader and its own fonts (Noto for every script, so
text renders the same on any host). The GPU driver comes from the system
(`/run/opengl-driver` on NixOS; elsewhere run it through nixGL).

## Documentation

- `docs/guide.md` (`reel docs`): writing, checking and rendering scenes.
- `js/reel.d.ts` (`reel docs api`): the whole API with types; `reel init` copies it into a
  project.
- `docs/shaders.md` (`reel docs shaders`): custom shaders.
- `examples/`: `hello.ts`, `cards.ts` (layout, a shader background, staggered entrances),
  `phone.ts` (a phone in perspective, clipping, a callout).
- `AGENTS.md`: working on reel itself.
