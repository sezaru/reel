// Feature cards: a shader background, a headline, and three cards laid out by flexbox that
// enter one after another, then a chip row. Shows: shaders, layout, staggered springs,
// markers, and splitting a scene into small functions.
import { type Canvas, video, seconds, spring, lerp, Easing, row, col, text, box, measure, rgba } from "reel";

const INK = "#33291F";
const ACCENT = "#C2562F";
const CARD = { fill: "#FFFCF7", radius: 22, border: { width: 1.5, color: "#EADFCF" }, shadow: [{ y: 26, blur: 60, color: rgba(120, 80, 40, 0.18) }, { y: 4, blur: 12, color: rgba(120, 80, 40, 0.1) }] };

const FEATURES = [
  { title: "Everything in one place", body: "Prescriptions, results and visits, sorted into journeys." },
  { title: "Reminders that work", body: "A nudge at the right time, from your lock screen." },
  { title: "Share in one link", body: "Your doctor opens it in a browser. Nothing to install." },
];

// cue frames, in one place
const T = { title: 10, cards: 40, chips: 120, out: 270 };

function card(title: string, body: string, minHeight = 0) {
  return col(
    [
      box(56, 56, (cv, [x, y, w]) => cv.circle(x + w / 2, y + w / 2, w / 2, rgba(194, 86, 47, 0.14))),
      text(title, { size: 40, weight: 800, color: INK, letterSpacing: -0.02 }),
      text(body, { size: 28, weight: 500, color: "#7B6F60", lineHeight: 38 }),
    ],
    { width: 480, minHeight, padding: [36, 36, 40, 36], gap: 18, look: CARD },
  );
}

function enter(cv: Canvas, f: number, at: number, draw: () => void) {
  const p = spring({ frame: f - at, config: { damping: 16, stiffness: 140 } });
  const gone = lerp(f, [T.out, T.out + 14], [0, 1]);
  cv.group({ opacity: Math.min(p, 1 - gone), y: (1 - p) * 40 - gone * 30, blur: (1 - p) * 8 }, draw);
}

export default video(
  { duration: "5s", background: "#F4ECE0", markers: { cards: T.cards, chips: T.chips } },
  (cv, f) => {
    cv.shader("paper.wgsl", { params: [960 + Math.sin(f / 90) * 300, 300, 1100, 0.6] });

    enter(cv, f, T.title, () => {
      cv.text("Your health, organised", cv.width / 2, 120, { size: 84, weight: 800, color: INK, letterSpacing: -0.03, anchor: "center" });
    });

    // three cards side by side, centred, as tall as the tallest, each entering 6 frames
    // after the last
    const tallest = Math.max(...FEATURES.map((it) => measure(card(it.title, it.body)).height));
    const els = FEATURES.map((it) => card(it.title, it.body, tallest));
    const widths = els.map((e) => measure(e).width);
    const gap = 40;
    let x = cv.width / 2 - (widths.reduce((a, b) => a + b, 0) + gap * (els.length - 1)) / 2;
    els.forEach((el, i) => {
      const at = x;
      enter(cv, f, T.cards + i * 6, () => cv.layout(el, at, 300));
      x += widths[i] + gap;
    });

    // a row of chips, laid out as one row
    const chips = row(
      ["Private", "No account", "Free"].map((s) =>
        row([box(14, 14, (cv, [x, y, w]) => cv.circle(x + w / 2, y + w / 2, w / 2, ACCENT)), text(s, { size: 36, weight: 700, color: INK })], {
          align: "center",
          gap: 14,
          padding: [14, 30],
          look: { fill: "#FFFCF7", radius: 40, border: { width: 1.5, color: "#EADFCF" } },
        }),
      ),
      { gap: 22 },
    );
    const w = measure(chips).width;
    enter(cv, f, T.chips, () => cv.layout(chips, cv.width / 2 - w / 2, 860));

    // an underline that draws itself under the title, eased
    const u = lerp(f, [T.title + 10, T.title + 50], [0, 1], Easing.inOut(Easing.cubic));
    cv.fade(1 - lerp(f, [T.out, T.out + 14], [0, 1]), () => cv.path("M 700 240 L 1220 240", { stroke: ACCENT, width: 6, progress: u }));
  },
);
