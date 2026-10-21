// A phone in perspective: a screen (here drawn with shapes; use cv.image with a screenshot
// or a recording's frames) clipped to the rounded glass, tilting in 3D on a spring, with a
// callout line drawing itself to a label. Shows: matrix.perspective, clip, paths, the
// camera-move pattern (interpolate the numbers of a transform between keyframes).
import { type Canvas, video, spring, lerp, Easing, matrix, measureText, rgba, interpolate } from "reel";

const W = 420;
const H = 880;

// the "screen": a list of rows; with a real app, `cv.image(frames[i], x, y, w, h, { fit: "cover" })`
function screen(cv: Canvas, x: number, y: number, f: number) {
  cv.rect(x, y, W, H, { fill: "#F4ECE0" });
  cv.text("Today", x + 32, y + 90, { size: 44, weight: 800, color: "#33291F" });
  for (let i = 0; i < 6; i++) {
    const top = y + 170 + i * 110;
    const done = f > 60 + i * 8;
    cv.rect(x + 24, top, W - 48, 92, { fill: "#FFFCF7", radius: 18, shadow: { y: 4, blur: 12, color: rgba(120, 80, 40, 0.1) } });
    cv.circle(x + 70, top + 46, 18, done ? "#5F8466" : "#EADFCF");
    cv.text(`Dose ${i + 1}`, x + 104, top + 26, { size: 30, weight: 600, color: "#33291F" });
  }
}

// camera keyframes: [frame, rotateY, rotateX, x]
const KEYS = [
  [0, -24, 8, 1240],
  [40, -24, 8, 1240],
  [90, 0, 0, 960],
  [180, 12, -4, 760],
];

function cam(f: number) {
  const at = KEYS.map((k) => k[0]);
  const ease = { easing: Easing.inOut(Easing.cubic), extrapolateLeft: "clamp" as const, extrapolateRight: "clamp" as const };
  return [1, 2, 3].map((j) => interpolate(f, at, KEYS.map((k) => k[j]), ease));
}

export default video({ duration: "4s", background: "#EFE6DA" }, (cv, f) => {
  const [ry, rx, cx] = cam(f);
  const pop = spring({ frame: f, config: { damping: 18, stiffness: 90 } });
  const x = cx - W / 2;
  const y = 540 - H / 2;
  const tilt = matrix.perspective({ rotateX: rx, rotateY: ry, origin: [cx, 540], distance: 2600 });
  cv.fade(pop, () => {
    cv.transform(matrix.multiply(tilt, matrix.scale(0.9 + 0.1 * pop, 0.9 + 0.1 * pop, [cx, 540])), () => {
      // the body, the glass, the screen clipped to it
      cv.rect(x - 16, y - 16, W + 32, H + 32, {
        gradient: { angle: 150, stops: [["#5a5048", 0], ["#1d1a17", 0.6], ["#332c27", 1]] },
        radius: 62,
        shadow: [{ x: 30, y: 60, blur: 120, color: rgba(96, 52, 20, 0.3) }],
      });
      cv.clip([x, y, W, H], 48, () => screen(cv, x, y, f));
    });
  });

  // a callout: a line drawing itself from the third row's dot (found on the tilted phone by
  // putting its flat position through the same matrix) to a label
  const p = lerp(f, [120, 150], [0, 1], Easing.out(Easing.cubic));
  if (p > 0) {
    const from = matrix.apply(tilt, x + 70, y + 170 + 2 * 110 + 46);
    cv.path(`M ${from[0]} ${from[1]} L ${from[0] - 260} ${from[1] - 120}`, { stroke: "#C2562F", width: 5, progress: p });
    cv.circle(from[0], from[1], 10 * p, "#C2562F");
    const label = "Taken on time";
    const m = measureText(label, { size: 40, weight: 700 });
    cv.fade(lerp(f, [140, 156], [0, 1]), () => {
      cv.rect(from[0] - 260 - m.width - 56, from[1] - 120 - 40, m.width + 56, 80, { fill: "#FFFCF7", radius: 20, shadow: { y: 10, blur: 30, color: rgba(120, 80, 40, 0.2) } });
      cv.text(label, from[0] - 260 - 28, from[1] - 120 - m.height / 2, { size: 40, weight: 700, color: "#33291F", anchor: "end" });
    });
  }
});
