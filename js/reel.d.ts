// The `reel` module: everything a scene file can use. This file is the API reference
// (`reel docs api` prints it; `reel init` copies it into a project for editors and type
// checkers). The guide, with how scenes are written and checked: `reel docs`.
//
// Units: lengths are design pixels (the video's `width` × `height`, origin top-left, y down),
// whatever size it renders at; times are frames; angles are degrees; letter-spacing is in em.

declare module "reel" {
  // ── the video ───────────────────────────────────────────────────────────────────────────

  /** What the video is. Only `duration` is required. */
  export interface VideoConfig {
    /** Design width in px (default 1920). Rendering at `--scale 0.5` halves the output, not the design. */
    width?: number;
    /** Design height in px (default 1080). */
    height?: number;
    /** Frames per second (default 60). */
    fps?: number;
    /** Length: frames, or a time ("28s", "1:05.5"). */
    duration: number | string;
    /** What shows where nothing is drawn (default "#000"). */
    background?: Color;
    /** Font files (.ttf/.otf, globs allowed), relative to the scene file. */
    fonts?: string[];
    /** The default font family (default: the first font's family, else "sans-serif"). */
    font?: string;
    /**
     * The text's language, a BCP 47 tag ("en", "pt-BR", "ja"): picks CJK glyph forms and
     * line breaking. Any script renders whatever this says: missing glyphs fall back to
     * system fonts (Noto in reel's own environment).
     */
    lang?: string;
    /**
     * True motion blur (default on): a frame where something moves is drawn several times
     * across the shutter and averaged. `shutter` is the fraction of a frame the shutter is
     * open (0.5 = 180°), `maxSamples` the most sub-frames (16). `false` or `--no-motion-blur`
     * renders each frame once (faster drafts).
     */
    motionBlur?: boolean | { shutter?: number; maxSamples?: number };
    /** Sound, mixed by ffmpeg when the video is rendered (the preview is silent). */
    audio?: AudioTrack[];
    /** Loudness target of the mix in LUFS (e.g. -14 for a promo, -16 for in-app playback). */
    loudness?: number;
    /** Named times (frames or "12s"): `reel still --at name`, and the preview's ↑/↓ jump between them. */
    markers?: Record<string, number | string>;
  }

  /** One sound file in the mix. */
  export interface AudioTrack {
    /** The file (anything ffmpeg reads), relative to the scene file. */
    src: string;
    /** The video frame it starts at (default 0). */
    at?: number;
    /** Seconds into the file to start from (default 0). */
    from?: number;
    /** Seconds of it to play (default: to its end, or the video's). */
    duration?: number;
    /** Gain in dB (default 0). */
    volume?: number;
    /** Fade in / out, seconds. The fade out ends where the track (or the video) ends. */
    fadeIn?: number;
    fadeOut?: number;
    /** A voice track: tracks with `duck` dip under it while it speaks. */
    voice?: boolean;
    /** Dip under the voice tracks while they speak (music beds). */
    duck?: boolean;
  }

  /** The default export of a scene file. */
  export interface Video {
    readonly config: Required<Pick<VideoConfig, "width" | "height" | "fps">> & VideoConfig & { duration: number };
  }

  /**
   * Declares the video: its config and how to draw a frame. `draw` is called for every frame
   * (and, with motion blur, at fractional frames in between): it must depend on `f` only,
   * never on what earlier calls did, since frames are drawn out of order on many threads.
   *
   * ```ts
   * export default video({ duration: seconds(5) }, (cv, f) => {
   *   cv.rect(0, 0, 1920, 1080, { fill: "#F4ECE0" });
   *   cv.text("Hello", 960, 500, { size: 96, weight: 700, anchor: "center", opacity: interpolate(f, [0, 20], [0, 1]) });
   * });
   * ```
   */
  export function video(config: VideoConfig, draw: (cv: Canvas, f: number) => void): Video;

  /** Seconds → frames at the video's fps (inside video()'s config, before it knows the fps, 60 is assumed: pass `fps` there if it differs, or write times as "4s"). */
  export function seconds(s: number, fps?: number): number;

  /** Values passed on the command line with `--set key=value` (e.g. `params.lang`). */
  export const params: Readonly<Record<string, string>>;

  /** An environment variable, or undefined. */
  export function env(name: string): string | undefined;

  // ── drawing ─────────────────────────────────────────────────────────────────────────────

  /** A colour: "#rgb", "#rrggbb", "#rrggbbaa", "rgb(r g b)", "rgba(r, g, b, a)", "transparent", or [r, g, b, a] in 0…1. */
  export type Color = string | [number, number, number, number];
  /** x, y, width, height. */
  export type Rect = [number, number, number, number];
  /** Corner radii: one for all, or top-left, top-right, bottom-right, bottom-left. */
  export type Radius = number | [number, number, number, number];
  export interface Size {
    width: number;
    height: number;
  }

  /** A CSS box-shadow. */
  export interface Shadow {
    x?: number;
    y?: number;
    blur?: number;
    spread?: number;
    color: Color;
  }

  /** How a box looks: CSS background, border-radius, border, box-shadow, opacity. */
  export interface Look {
    fill?: Color;
    /** A CSS linear-gradient: `angle` in degrees (180 = top to bottom, the default), up to 4 stops at 0…1. */
    gradient?: { angle?: number; stops: [Color, number][] };
    radius?: Radius;
    /** Drawn inside the box's edge, as CSS `box-sizing: border-box`. */
    border?: { width: number; color: Color };
    /** Outer shadows, drawn under the box. */
    shadow?: Shadow | Shadow[];
    /** Inner shadows (CSS `inset`), at most 2. */
    inset?: Shadow | Shadow[];
    opacity?: number;
    /** Softens the edge by this many px (a blurred edge without a layer). */
    feather?: number;
    /** "multiply" darkens what is under it (paper shadows, ink). */
    blend?: "normal" | "multiply";
    /** Draw the shadows only outside the box (for translucent boxes, as a browser does). */
    clipShadows?: boolean;
  }

  export interface TextStyle {
    /** px (default 32). */
    size?: number;
    /** 100…900 (default 400). */
    weight?: number;
    /** Default "#000". */
    color?: Color;
    /** A family name (default: the video's `font`). */
    font?: string;
    /** In em, as CSS (-0.02 tightens). */
    letterSpacing?: number;
    /** Line box height in px (default: the font's ascent + descent). */
    lineHeight?: number;
    /** Wrap at this width; lines align within it (`align`). */
    width?: number;
    /** How wrapped lines align in `width` (default "start"). */
    align?: "start" | "center" | "end";
    /** Wrapped lines made as even in length as they can be (CSS `text-wrap: balance`): captions, titles. */
    balance?: boolean;
    /** Where x is on the text: its start (default), centre or end. For one line or a wrapped block alike. */
    anchor?: "start" | "center" | "end";
    opacity?: number;
  }

  /**
   * Text: a string, or spans drawn as one text (one line, or wrapped together) with their
   * own colours: `[{ text: "Tap " }, { text: "Save", color: "#C2562F" }]` (a span without a
   * colour takes the style's). Highlighting the word being said in a caption is one use.
   */
  export type Text = string | { text: string; color?: Color }[];

  export interface TextMetrics extends Size {
    /** Number of lines (1 unless wrapped). */
    lines: number;
    /** The first baseline, from the top of the line box. */
    baseline: number;
  }

  export interface PathStyle {
    /** Stroke colour (round caps and joins). */
    stroke?: Color;
    /** Stroke width (default 2). */
    width?: number;
    /** Fill colour. */
    fill?: Color;
    /** How the fill treats overlaps (default "nonzero"); "evenodd" makes holes of shapes inside shapes, whichever way they run (a dimmed frame with cut-outs). */
    fillRule?: "nonzero" | "evenodd";
    /** Draw only the stretch from `from` to `to` along the path, in px of length. */
    from?: number;
    to?: number;
    /** Or: draw the first `progress` (0…1) of it: a line drawing itself on. */
    progress?: number;
    opacity?: number;
  }

  /** A layer: CSS opacity, filter: blur() and drop-shadow(), mask-image, a translate. */
  export interface GroupStyle {
    opacity?: number;
    /** Gaussian blur in px (CSS filter: blur(), the deviation; or [x, y]). */
    blur?: number | [number, number];
    x?: number;
    y?: number;
    /**
     * A mask, in the frame's coordinates (design px, whatever the transforms):
     * - `{ ellipse: [cx, cy, rx, ry], from, to }`: transparent inside `from`, opaque beyond `to` (fractions of the radii); swap them to keep the middle.
     * - `{ linear: [x0, y0, x1, y1], stops: [[alpha, position], …] }`: alpha along the line from (x0, y0) to (x1, y1), positions 0…1, up to 4 stops (CSS mask-image: linear-gradient). `{ linear: [0, 0, 0, 1080], stops: [[0, 0.1], [1, 0.2]] }` fades the top out.
     */
    mask?: { ellipse: [cx: number, cy: number, rx: number, ry: number]; from: number; to: number } | { linear: [x0: number, y0: number, x1: number, y1: number]; stops: [alpha: number, position: number][] };
    /** A shadow of everything in the layer, by its shape (CSS filter: drop-shadow()): a glow with x = y = 0. */
    shadow?: Shadow;
  }

  export interface ModelOptions {
    /** The box the model is fitted into, whole (contain, with a hair of margin), in the current coordinates. Default: the whole frame. */
    rect?: Rect;
    /** Degrees the camera orbits the device: yaw about its vertical axis (positive: from its right, so you see its right edge), pitch about its horizontal one (negative: from above). Keep both modest so the screen stays legible. */
    yaw?: number;
    pitch?: number;
    /** "landscape" turns the device 90° (tablets); give it a landscape image. Default "portrait". */
    orient?: "portrait" | "landscape";
    /** How much of the rect the device fills: 1 (default) touches its binding side; 0.6 leaves room beside it. */
    scale?: number;
    /** An image (JPEG or PNG) on the screen: a screenshot, a filmed frame. Capture it at the screen's aspect (`ModelInfo.screen.aspect`) or it is stretched. Without it the model's own screen shows. */
    screen?: string;
    /** Vertical field of view in degrees (default 28). */
    fov?: number;
    /** Light (default 1.05). */
    exposure?: number;
    opacity?: number;
    /** Rendered at this many times its size on the frame, then filtered down (default 2: clean edges on glossy metal). */
    supersample?: number;
  }

  /** Where a model landed, in the coordinates its rect was given in. */
  export interface ModelInfo {
    /** The device's box (x, y, w, h): keep copy clear of it, or overlap it on purpose. */
    box: Rect;
    /**
     * Its screen: the corners of the image it shows (top-left, top-right, bottom-right,
     * bottom-left) and that image's aspect (width / height). Draw on the screen, in
     * perspective, with `cv.transform(matrix.quad([0, 0, w, h], screen.corners), …)`. Null
     * when the model has no screen (a mesh or material named screen, display or wallpaper).
     */
    screen: { corners: [[number, number], [number, number], [number, number], [number, number]]; aspect: number } | null;
  }

  /** What `draw` gets: a recorder of shapes. Nothing it draws is visible until the frame ends. */
  export interface Canvas {
    /** The frame being drawn (fractional for motion-blur sub-frames). */
    readonly frame: number;
    readonly width: number;
    readonly height: number;
    readonly fps: number;

    /** A box with border box (x, y, w, h). */
    rect(x: number, y: number, w: number, h: number, look: Look): void;
    /** A circle. */
    circle(cx: number, cy: number, r: number, look: Look | Color): void;
    /**
     * An image file (JPEG or PNG; relative to the scene file) in the box (x, y, w, h),
     * stretched unless `fit` says otherwise; `uv` picks the part of it shown (fractions
     * x, y, w, h). The look's radius, border and shadows apply to the box.
     */
    image(src: string, x: number, y: number, w: number, h: number, look?: Look & { uv?: Rect; fit?: "fill" | "cover" | "contain" }): void;
    /** Text whose line box's top-left is (x, y) (see `anchor`). Returns its size. */
    text(s: Text, x: number, y: number, style?: TextStyle): TextMetrics;
    /** A path (SVG path data, or `path(d)`). */
    path(d: string | Path, style: PathStyle): void;
    /** A custom WGSL shader filling `rect` (default: the whole frame); see `reel docs shaders`. `src` is a .wgsl file or the source itself. */
    shader(src: string, opts?: { rect?: Rect; params?: number[] }): void;
    /**
     * A 3D model (glTF 2.0, `.gltf` or `.glb`, relative to the scene file): a phone, a tablet,
     * in studio light, fitted into `opts.rect` (default: the whole frame), an image on its
     * screen. It is drawn like an image: transforms, clips and groups (a drop shadow) apply.
     * Returns where it landed; see `reel docs models`.
     */
    model(src: string, opts?: ModelOptions): ModelInfo;
    /** Lays out a box tree (see `row`, `col`) with its top-left at (x, y), draws it; returns its size. */
    layout(el: El, x: number, y: number): Size;

    /** Draws into a layer that is faded, blurred, moved or masked as one. */
    group(style: GroupStyle, draw: () => void): void;
    /** `group({ opacity })`. */
    fade(opacity: number, draw: () => void): void;
    translate(x: number, y: number, draw: () => void): void;
    /** Scales about `origin` (default [0, 0]). */
    scale(s: number, draw: () => void, origin?: [number, number]): void;
    /** Rotates clockwise about `origin` (default [0, 0]). */
    rotate(deg: number, draw: () => void, origin?: [number, number]): void;
    /** Any 3×3 transform, perspective included (see `matrix`). */
    transform(m: Matrix, draw: () => void): void;
    /** Clips everything drawn inside to a rounded rect (in the current coordinates). Inside another clip in the same coordinates, the two intersect; under a different transform, the inner clip replaces the outer one. */
    clip(rect: Rect, radius: Radius, draw: () => void): void;
    /** Keeps only the part of boxes drawn inside where a·x + b·y + c ≥ 0. */
    clipHalf(a: number, b: number, c: number, draw: () => void): void;
    /** Draws only while `from ≤ frame < to`; `local` is `frame - from` (a Remotion <Sequence>). */
    during(from: number, to: number, draw: (local: number) => void): void;
  }

  // ── layout (flexbox) ────────────────────────────────────────────────────────────────────

  /** A box in a layout tree. Build them with `row`, `col`, `text`, `box`. */
  export interface El {
    readonly kind: "row" | "col" | "text" | "box";
  }

  export interface BoxStyle {
    /** CSS padding: all, [vertical, horizontal], or [top, right, bottom, left]. */
    padding?: number | [number, number] | [number, number, number, number];
    gap?: number;
    /** Cross-axis alignment of the children (default "start"). */
    align?: "start" | "center" | "end" | "stretch";
    /** Main-axis distribution (default "start"). */
    justify?: "start" | "center" | "end" | "between" | "around" | "evenly";
    width?: number;
    height?: number;
    minWidth?: number;
    minHeight?: number;
    maxWidth?: number;
    /** flex-grow. */
    grow?: number;
    /** The box's background, border (it takes room, as in CSS) and shadows. */
    look?: Look;
  }

  /** Children side by side. */
  export function row(children: El[], style?: BoxStyle): El;
  /** Children stacked. */
  export function col(children: El[], style?: BoxStyle): El;
  /** Text; it wraps when its box is narrower than it (give the box a `width`). */
  export function text(s: Text, style?: TextStyle & { grow?: number }): El;
  /** A fixed-size box, drawn by `draw` at the rect it is laid out at (an icon, a picture, a space). */
  export function box(w: number, h: number, draw?: (cv: Canvas, rect: Rect) => void): El;
  /** A laid-out tree's size, without drawing it. */
  export function measure(el: El): Size;

  /** One line's (or a wrapped block's, with `width`) size. */
  export function measureText(s: Text, style?: TextStyle): TextMetrics;

  // ── animation ───────────────────────────────────────────────────────────────────────────

  export type EasingFn = (t: number) => number;
  export type Extrapolate = "clamp" | "extend" | "identity";

  /**
   * Remotion's interpolate(): maps `x` through the piecewise-linear `input` → `output`.
   * Extends beyond the ends by default: pass `{ extrapolateLeft: "clamp", extrapolateRight: "clamp" }`, or use `lerp`.
   */
  export function interpolate(x: number, input: number[], output: number[], opts?: { easing?: EasingFn; extrapolateLeft?: Extrapolate; extrapolateRight?: Extrapolate }): number;
  /** interpolate(), clamped at both ends, optionally eased. */
  export function lerp(x: number, input: number[], output: number[], easing?: EasingFn): number;

  /**
   * Remotion's spring(), frame for frame: 0 → 1 (or `from` → `to`) as `frame` goes from 0.
   * `durationInFrames` stretches it to settle then; `delay` starts it later.
   */
  export function spring(opts: {
    frame: number;
    fps?: number;
    config?: { damping?: number; mass?: number; stiffness?: number; overshootClamping?: boolean };
    from?: number;
    to?: number;
    durationInFrames?: number;
    delay?: number;
  }): number;

  /** Remotion's Easing. */
  export const Easing: {
    linear: EasingFn;
    ease: EasingFn;
    quad: EasingFn;
    cubic: EasingFn;
    sin: EasingFn;
    circle: EasingFn;
    exp: EasingFn;
    poly(n: number): EasingFn;
    back(s?: number): EasingFn;
    elastic(bounciness?: number): EasingFn;
    bounce: EasingFn;
    bezier(x1: number, y1: number, x2: number, y2: number): EasingFn;
    in(f: EasingFn): EasingFn;
    out(f: EasingFn): EasingFn;
    inOut(f: EasingFn): EasingFn;
  };

  /**
   * A number in [0, 1) that depends only on `seed` (and `salt`): use it wherever you would
   * use Math.random(), which must not be used (frames are drawn out of order, on many
   * threads, several times each for motion blur, so they would disagree).
   * `random(i)` for the i-th item, `random(i, 1)` for a second independent value.
   */
  export function random(seed: number, salt?: number): number;

  export function clamp(x: number, lo?: number, hi?: number): number;
  /** a + (b - a) · t. */
  export function mix(a: number, b: number, t: number): number;

  // ── colours ─────────────────────────────────────────────────────────────────────────────

  /** A colour with its alpha multiplied by `a`. */
  export function alpha(c: Color, a: number): Color;
  /** Two colours mixed (t = 0: a, 1: b). */
  export function mixColor(a: Color, b: Color, t: number): Color;
  /** rgba() with r, g, b in 0…255 and a in 0…1. */
  export function rgba(r: number, g: number, b: number, a?: number): Color;

  // ── paths and transforms ────────────────────────────────────────────────────────────────

  /** A parsed, measured path. */
  export interface Path {
    readonly d: string;
    /** Total length in px. */
    readonly length: number;
    /** x0, y0, x1, y1. */
    readonly bbox: [number, number, number, number];
    /** The point at length `s` along it. */
    pointAt(s: number): [number, number];
  }
  /** Parses SVG path data (M L H V C S Q T A Z, absolute or relative). */
  export function path(d: string): Path;
  /** Path data through points; `close` joins the last to the first. */
  export function polyline(points: [number, number][], close?: boolean): string;

  /** A 3×3 matrix, row-major: [a, b, c, d, e, f, g, h, i] maps (x, y) to ((a·x + b·y + c) / w, (d·x + e·y + f) / w), w = g·x + h·y + i. */
  export type Matrix = number[];
  export const matrix: {
    identity(): Matrix;
    translate(x: number, y: number): Matrix;
    scale(sx: number, sy?: number, origin?: [number, number]): Matrix;
    rotate(deg: number, origin?: [number, number]): Matrix;
    /** CSS `perspective(distance) rotateX(rotateX) rotateY(rotateY)` about `origin`, as a homography of the z = 0 plane. */
    perspective(opts: { rotateX?: number; rotateY?: number; origin?: [number, number]; distance?: number }): Matrix;
    /** a · b (b applies first). */
    multiply(a: Matrix, b: Matrix): Matrix;
    apply(m: Matrix, x: number, y: number): [number, number];
    invert(m: Matrix): Matrix;
    /** The homography that maps `rect` onto four corners (top-left, top-right, bottom-right, bottom-left): draw on a model's screen, a sign in a photo. */
    quad(rect: Rect, corners: [number, number][]): Matrix;
  };

  // ── files ───────────────────────────────────────────────────────────────────────────────
  // Relative paths are relative to the scene file (the one `reel` was given).

  /** The absolute path. */
  export function resolve(path: string): string;
  export function exists(path: string): boolean;
  export function readText(path: string): string;
  export function readJson<T = any>(path: string): T;
  /** A directory's entries (names, sorted), optionally only those matching a glob ("*.jpg"). */
  export function listDir(path: string, pattern?: string): string[];
  /** An image file's size in px (from its header). */
  export function imageSize(path: string): Size;
  /** The image files (JPEG, PNG) in a directory, sorted by name, as paths: an image sequence. */
  export function frames(dir: string): string[];
  /** Where `cv.model(src, opts)` would put a model, without drawing it (to lay a scene out around it). `opts.rect` defaults to the whole frame. */
  export function modelInfo(src: string, opts?: ModelOptions): ModelInfo;
}
