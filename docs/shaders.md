# reel — custom shaders

`cv.shader(src, { rect, params })` fills a rect (by default the whole frame) with a WGSL
function you write: procedural backgrounds, gradients no box can make, noise, film grain,
light. It is drawn in order like any shape, under the current transform, group and clip.

`src` is a `.wgsl` file (relative to the scene) or the WGSL source itself. It must define:

```wgsl
fn shade(p: vec2f, px: vec2f) -> vec4f
```

- `p`: the point being shaded, in the scene's coordinates (design px, the same units as
  `cv.rect`), before the current transform: shade a rect at (100, 100) and `p` runs from
  (100, 100) across it.
- `px`: the output pixel being written (for per-pixel effects such as grain).
- Return straight (not premultiplied) RGBA in 0…1; reel premultiplies it and blends it
  over what is below.

What else the shader can use:

| | |
|---|---|
| `reel.frame` | the frame being drawn (fractional between frames, with motion blur) |
| `reel.scale` | output pixels per design pixel (0.5 in a `--scale 0.5` draft) |
| `reel.size` | the design size (`vec2f`) |
| `reel.rect` | the rect being filled: x, y, w, h (`vec4f`) |
| `param(i)` | `params[4i … 4i + 3]` as a `vec4f` (up to 64 params: `param(0)` … `param(15)`) |
| `hash(p)` | white noise, 0…1, from a 2D point (for smooth inputs; on whole-pixel coordinates it shows a faint weave) |
| `ihash(p)` | white noise, 0…1, from a whole-number point (floored): per-pixel grain with no pattern |
| `noise(p)` | smooth gradient noise, about −0.7…0.7, one feature per unit |
| `fbm(p, octaves)` | fractal noise: octaves of `noise`, each twice the frequency, half the amplitude |
| `sdRoundBox(p, halfSize, radii)` | signed distance to a rounded box centred at 0 |

Animate a shader by passing values that change with the frame in `params` (computed in
TypeScript), or by reading `reel.frame` in it. Keep it a function of those: no state.

`reel check` compiles every shader a scene uses and reports errors at the shader file's
own line numbers.

## Example: warm paper with drifting light

```wgsl
// paper.wgsl — params: param(0) = light (x, y, radius, strength)
fn shade(p: vec2f, px: vec2f) -> vec4f {
    var c = mix(vec3f(0.957, 0.925, 0.878), vec3f(0.918, 0.875, 0.812), p.y / reel.size.y);
    let l = param(0);
    let glow = l.w * max(0.0, 1.0 - length(p - l.xy) / l.z);
    c = mix(c, vec3f(1.0, 0.67, 0.47), glow * glow);
    // fibre: streaky noise, multiplied
    c *= 1.0 - 0.04 * (0.5 + 0.5 * fbm(p * vec2f(0.012, 0.9), 3));
    // grain: one value per output pixel, changing every frame
    c += (ihash(px + floor(reel.frame) * 7.0) - 0.5) * 0.02;
    return vec4f(c, 1.0);
}
```

```ts
cv.shader("paper.wgsl", { params: [960 + Math.sin(f / 90) * 300, 400, 900, 0.5] });
```
