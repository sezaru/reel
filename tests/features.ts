// Every drawing feature on one frame, for looking at after an engine change:
//   reel still tests/features.ts --at 30 -o features.png
import { video, rgba, row, text, matrix } from "reel";

export default video({ duration: 60, background: "#EFE6DA" }, (cv, f) => {
  // boxes: fill, gradient, radius, border, shadows, inset, feather, multiply
  cv.rect(40, 40, 260, 160, { fill: "#FFFCF7", radius: 22, border: { width: 2, color: "#C2562F" }, shadow: { y: 18, blur: 40, color: rgba(120, 80, 40, 0.3) } });
  cv.rect(330, 40, 260, 160, { gradient: { angle: 135, stops: [["#FF8A45", 0], ["#C2562F", 1]] }, radius: [40, 0, 40, 0], inset: { blur: 20, color: "#0006" } });
  cv.rect(620, 40, 260, 160, { fill: "#5F8466", radius: 80, feather: 20 });
  cv.rect(910, 40, 260, 160, { fill: "#BC8A33", blend: "multiply", radius: 12 });
  // paths: stroke draw-on, fill nonzero vs evenodd (two squares one inside the other)
  cv.path("M 40 300 C 200 220 300 380 460 300", { stroke: "#C2562F", width: 8, progress: 0.8 });
  const sq = "M 520 230 H 680 V 390 H 520 Z M 560 270 H 640 V 350 H 560 Z";
  cv.path(sq, { fill: "#33291F" });
  cv.translate(200, 0, () => cv.path(sq, { fill: "#33291F", fillRule: "evenodd" }));
  // layers: opacity, blur, linear mask, ellipse mask, drop shadow
  cv.group({ opacity: 0.5 }, () => cv.rect(940, 230, 200, 160, { fill: "#C2562F" }));
  cv.group({ blur: 6 }, () => cv.text("blur", 1180, 260, { size: 60, weight: 800 }));
  cv.group({ mask: { linear: [0, 230, 0, 390], stops: [[0, 0], [1, 1]] } }, () => cv.rect(1400, 230, 200, 160, { fill: "#33291F" }));
  cv.group({ mask: { ellipse: [1740, 310, 100, 80], from: 1, to: 0.4 } }, () => cv.rect(1640, 230, 200, 160, { fill: "#33291F" }));
  cv.group({ shadow: { y: 10, blur: 16, color: rgba(194, 86, 47, 0.8) } }, () => cv.path("M 60 470 L 300 470 L 180 600 Z", { fill: "#FFFCF7" }));
  // text: spans, wrapping, balance, anchors, letter-spacing, emoji and other scripts
  cv.text([{ text: "Tap " }, { text: "Save", color: "#C2562F" }, { text: " to keep it" }], 360, 460, { size: 52, weight: 700, color: "#33291F" });
  const long = "Every record, prescription and lab result kept in one place";
  cv.rect(360, 540, 640, 150, { fill: "#0001" });
  cv.text(long, 360, 540, { size: 40, width: 640, align: "center" });
  cv.rect(1040, 540, 640, 150, { fill: "#0001" });
  cv.text(long, 1040, 540, { size: 40, width: 640, align: "center", balance: true });
  cv.text("健康のすべてを、ひとつの場所に。 आपकी पूरी सेहत 🔥🩸", 960, 760, { size: 48, anchor: "center", color: "#33291F" });
  // layout, transforms, perspective, clip
  cv.layout(row([text("chip", { size: 36, weight: 700 }), text([{ text: "• two" }, { text: " spans", color: "#5F8466" }], { size: 36 })], { gap: 16, padding: [10, 26], align: "center", look: { fill: "#FFFCF7", radius: 30 } }), 40, 860);
  cv.rotate(-8, () => cv.rect(500, 860, 200, 120, { fill: "#5F8466", radius: 16 }), [600, 920]);
  cv.transform(matrix.perspective({ rotateY: 35, origin: [900, 920], distance: 1200 }), () => cv.rect(800, 860, 200, 120, { fill: "#BC8A33", radius: 16 }));
  cv.clip([1100, 860, 200, 120], 40, () => cv.rect(1050, 820, 300, 200, { gradient: { stops: [["#33291F", 0], ["#C2562F", 1]] } }));
  cv.shader("fn shade(p: vec2f, px: vec2f) -> vec4f { let n = 0.5 + 0.5 * fbm(p * 0.02, 4); return vec4f(n, n * 0.8, n * 0.6, 1.0); }", { rect: [1400, 860, 300, 160] });
});
