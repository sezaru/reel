# Working on reel

Using reel (writing scenes) is in `docs/guide.md` (`reel docs`). This file is for changing
reel itself.

## Layout

| | |
|---|---|
| `src/main.rs` | the command line (clap) |
| `src/script.rs` | QuickJS: loading modules (TypeScript stripped by swc, keeping line and column), the native functions `js/reel.ts` calls (`__reel.*`), turning JS objects into looks, text styles, groups |
| `js/reel.ts` | the `reel` module scenes import: the Canvas wrapper, layout builders, Remotion's interpolate/Easing, colours, matrices, files |
| `js/reel.d.ts` | the API reference: **every export of reel.ts is declared and documented here** (a test checks the names match) |
| `src/config.rs` | the video config and command-line times |
| `src/canvas.rs` | the display list a frame records (boxes, shadows, strokes, shaders, layer push/pop) and the motion signatures |
| `src/gpu.rs`, `src/shaders.wgsl` | executing display lists with wgpu: shapes as SDFs under a homography, layers, blur, motion-blur accumulation, NV12 output, stills, the preview's present pass |
| `src/shader.rs` | scenes' custom WGSL: the prelude around `fn shade`, validation with naga |
| `src/text.rs` | Parley layout + swash rasterising; fonts from the scene's config, fallback through fontconfig (`fonts.nix`) |
| `src/layout.rs` | Taffy flexbox over trees built in reel.ts |
| `src/paths.rs` | SVG path parsing and flattening |
| `src/anim.rs` | Remotion's spring integrator |
| `src/render.rs` | workers (one QuickJS runtime each) → GPU → ffmpeg; audio graph; stills; contact sheets |
| `src/check.rs` | `reel check` |
| `src/preview.rs` | the winit window, hot reload |
| `src/docs.rs`, `docs/`, `examples/` | built-in documentation (`include_str!`) |

## Rules

- Everything a scene can do goes through `js/reel.ts` and is declared in `js/reel.d.ts`
  with a doc comment: that file is what users and agents read. Update `docs/guide.md`
  when behaviour or patterns change.
- A frame is a pure function of its number: never add state that carries between `draw`
  calls (workers draw frames out of order; motion blur draws fractional frames).
- Errors a scene can cause become JS exceptions with a clear message (`Exception::throw_*`
  in script.rs), never panics: `reel check` reports them with the frame and file:line.
- Keep Remotion semantics where a function has a Remotion name.

## Building and testing

`nix-shell --run 'cargo build --release'` (shell.nix has the toolchain, ffmpeg, the Vulkan
loader and the fonts). `./dev.sh [host] [cargo args]` syncs the tree to a build host and
runs cargo there.

```
cargo test --release                       # unit tests (colour parsing, the d.ts ↔ reel.ts names, the prelude strips)
reel check examples/cards.ts               # every example must check clean
reel sheet examples/cards.ts -o sheet.png  # and look right
```

A change to drawing should be compared before/after on real scenes: render the same
frames with both builds (`reel still … --at …`) and compare (`ffmpeg -i a.png -i b.png
-lavfi ssim -f null -`).
