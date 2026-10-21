// Warm paper with one drifting light, fibre and grain (used by cards.ts).
// params: param(0) = the light (x, y, radius, strength)
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
