// A first scene: a title that springs in over a soft gradient, a line that draws itself,
// and a caption that fades in. Run it: `reel preview hello.ts`.
import { video, seconds, spring, lerp, Easing } from "reel";

export default video({ duration: seconds(4), background: "#F4ECE0" }, (cv, f) => {
  cv.rect(0, 0, cv.width, cv.height, { gradient: { angle: 160, stops: [["#F4ECE0", 0], ["#EADFCF", 1]] } });

  // the title rises in on a spring
  const p = spring({ frame: f - 10, config: { damping: 14, stiffness: 120 } });
  cv.group({ opacity: p, y: (1 - p) * 40, blur: (1 - p) * 8 }, () => {
    cv.text("Hello, reel", cv.width / 2, 420, { size: 120, weight: 800, color: "#3A2A1E", letterSpacing: -0.03, anchor: "center" });
  });

  // a line under it draws itself on
  const draw = lerp(f, [30, 70], [0, 1], Easing.inOut(Easing.cubic));
  cv.path("M 660 600 L 1260 600", { stroke: "#C2562E", width: 6, progress: draw });

  // the caption fades in, wrapped in a 900 px box and centred in it
  cv.text("Videos from TypeScript, drawn on the GPU.", cv.width / 2, 650, {
    size: 44,
    weight: 500,
    color: "#6B5544",
    width: 900,
    align: "center",
    anchor: "center",
    opacity: lerp(f, [60, 80], [0, 1]),
  });
});
