# reel — 3D models

`cv.model(src, opts)` draws a glTF 2.0 model (`.gltf` with its files, or `.glb`): a phone,
a tablet, a laptop. It is lit as a product shot, with an image on the device's screen,
and fitted into a rect. It is drawn like an image, so transforms, clips, groups (opacity,
a drop shadow) and motion blur apply to it. Use it for store screenshots, posters, and
videos where a device turns.

```ts
import { video, matrix } from "reel";

export default video({ width: 1080, height: 2160, duration: 1, background: "#F4ECE0", motionBlur: false }, (cv) => {
  cv.group({ shadow: { y: 40, blur: 80, color: "#5A3A2050" } }, () => {
    const m = cv.model("models/s21-ultra/scene.gltf", {
      rect: [90, 560, 900, 1500],   // fitted whole into this box
      screen: "raw/home.png",       // the app's screenshot, on the screen
      yaw: 18, pitch: -6,           // degrees the camera orbits it
    });
  });
  cv.text("Every visit, on one timeline.", 540, 200, { size: 88, weight: 800, width: 900, align: "center", anchor: "center" });
});
```

## Options

| | |
|---|---|
| `rect` | `[x, y, w, h]`, the box the whole device is fitted into (contain, with a hair of margin), in the current coordinates. Default: the whole frame. |
| `yaw`, `pitch` | Degrees the camera orbits the device. A positive `yaw` looks from its right, so you see its right edge. A negative `pitch` looks from above. 0, 0 is head-on: the most legible, for the first shot. Keep angles modest (≤ 25°) so the screen stays readable. |
| `orient` | `"landscape"` turns the device 90° (tablets); give it a landscape image. |
| `scale` | How much of the rect the device fills: 1 (default) touches its binding side; 0.6 leaves room beside it inside the same rect. |
| `screen` | An image (JPEG or PNG) shown on the screen, as is (unlit, exact colours). Without it, the model's own screen material shows. |
| `fov` | Vertical field of view, degrees (default 28: a longish lens, little distortion). |
| `exposure` | Default 1.05. |
| `opacity` | |
| `supersample` | Rendered at this many times its size on the frame, then filtered down (default 2). |

The light is a studio's: three.js's RoomEnvironment (a white room with soft panels) for
reflections, a key light and a fill light, ACES tone mapping. The framing is three.js's
too. A layout written for the store-screenshot skill's `render_phone.py` lands in the same
place, and so does its device box.

## Where it landed

`cv.model` returns a `ModelInfo`. `modelInfo(src, opts)` returns the same without drawing,
for laying a scene out around it before drawing anything:

- `box`: the device's box `[x, y, w, h]`. Put copy in the clear area, or overlap it on
  purpose (a card bursting out of the frame).
- `screen.aspect`: the aspect (width / height) of the image the screen shows. **Capture
  screenshots at this aspect**, or they are stretched. A screenshot taken on that device
  already has it. A mismatched one stretches; never pad it with bars.
- `screen.corners`: the screen's four corners on the frame (top-left, top-right,
  bottom-right, bottom-left of its image). Drawing on the screen, in perspective:

```ts
const m = cv.model(PHONE, { rect, screen: "raw/home.png", yaw: 15 });
// the screenshot is 1440 × 3120: place things in its pixels
cv.transform(matrix.quad([0, 0, 1440, 3120], m.screen!.corners), () => {
  cv.rect(80, 1300, 1280, 260, { fill: "#C4502D", radius: 56 });   // highlight a row
});
```

A highlight drawn this way lies on the glass: it doesn't go behind the device's own
notch or camera hole.

## Models

A model needs a screen: a mesh (or its material, or its node) whose name contains
`screen`, `display` or `wallpaper` (and not `bezel` or `glass`). The image is mapped onto
that mesh's flat face, so its own UVs don't matter. The four models the store-screenshot
skill bundles all work, and so do most device models on Sketchfab:

| | |
|---|---|
| `s21-ultra` | Samsung Galaxy S21 Ultra (Google Play phone; screen 0.454) |
| `iphone-12-pro` | iPhone 12 Pro (App Store phone; 0.465) |
| `ipad-pro-12-9` | iPad Pro 12.9" (App Store tablet; 0.76, landscape 1.32) |
| `ipad-mini-6` | iPad mini 6 (a compact tablet; 0.66) |

They are CC BY 4.0 (DatSketch; the iPad Pro by Konstantin Koretskyi; the skill's
`assets/models/NOTICE.md` has the credit lines): credit the author wherever the images
are shown.

Supported glTF: meshes of triangles, node transforms, metal/rough materials (base colour,
metal/rough, normal and emissive textures; opaque, masked and blended). Not supported:
skins, morph targets, animations, cameras, lights, and KHR material extensions (they are
ignored).

## Store screenshots and other image sets

A set of screenshots is one scene: one frame per shot, a marker naming each. It renders
with `reel still … --at markers`, which writes `NAME.png` for every marker:

```ts
const SHOTS = ["hero", "scan", "share", "privacy"];

export default video(
  { width: 1080, height: 2160, duration: SHOTS.length, motionBlur: false,
    markers: Object.fromEntries(SHOTS.map((s, i) => [s, i])) },
  (cv, f) => {
    const shot = SHOTS[Math.round(f)];
    // … each shot's own composition
  },
);
```

```
reel still shots.ts --at markers -o final/          # final/hero.png, final/scan.png, …
reel still shots.ts --at markers -o final/pt-BR/ --set lang=pt-BR
reel sheet shots.ts --count 4 --cols 4               # the whole set side by side
```

Turn motion blur off (`motionBlur: false`): a still with it draws the frame at fractional
times too. The PNGs have no alpha channel, as the stores want. Sizes: Google Play phone
1080 × 2160 (or 1080 × 1920), its feature graphic 1024 × 500; App Store iPhone 6.9"
1320 × 2868, 6.7" 1290 × 2796, iPad 13" 2064 × 2752.

## Cost

A model is rendered into a texture whenever something about it changes (its angle, its
size on the frame, the image on its screen), with 4× multisampling at twice its size: a
few milliseconds each on a desktop GPU. A device standing still costs nothing after the
first frame. The first model also builds the studio light once per run (well under a
second).
