#!/usr/bin/env python3
# Writes tests/slab.gltf: `python3 tests/slab.py` from the repository root.
# A minimal device for reel's tests: a box body, a screen plane, glass over it; the whole
# thing under a node turned -90° about X (as Y-up exporters do), so baking transforms matters.
import json, struct, base64, math
W, H, D = 0.8, 1.6, 0.08
def box(w, h, d):
    pos, nrm, idx = [], [], []
    faces = [((1,0,0),(0,1,0),(0,0,1)), ((-1,0,0),(0,1,0),(0,0,-1)), ((0,1,0),(0,0,1),(1,0,0)),
             ((0,-1,0),(0,0,-1),(1,0,0)), ((0,0,1),(0,1,0),(-1,0,0)), ((0,0,-1),(0,1,0),(1,0,0))]
    half = (w/2, h/2, d/2)
    for n, u, v in faces:
        # face corners: n·half ± u·half ± v·half; wound counter-clockwise seen from outside
        c = [n[i]*half[i] for i in range(3)]
        ext = lambda a: [a[i]*half[i] for i in range(3)]
        uu, vv = ext(u), ext(v)
        base = len(pos)
        for su, sv in [(-1,-1), (1,-1), (1,1), (-1,1)]:
            pos.append([c[i] + su*uu[i] + sv*vv[i] for i in range(3)])
            nrm.append(list(n))
        # orientation check: (u × v) should point along n
        cr = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]]
        tri = [0,1,2, 0,2,3] if sum(cr[i]*n[i] for i in range(3)) > 0 else [0,2,1, 0,3,2]
        idx += [base+t for t in tri]
    return pos, nrm, None, idx
def plane(w, h, z):
    pos = [[-w/2,-h/2,z],[w/2,-h/2,z],[w/2,h/2,z],[-w/2,h/2,z]]
    return pos, [[0,0,1]]*4, [[0,1],[1,1],[1,0],[0,0]], [0,1,2,0,2,3]
meshes = [("Body", box(W, H, D), 0), ("Screen", plane(0.72, 1.52, D/2 + 0.001), 1), ("Glass", plane(0.76, 1.56, D/2 + 0.002), 2)]
buf = b""; views = []; accs = []; gm = []
def add(data, fmt, comp, typ, count, target, mn=None, mx=None):
    global buf
    while len(buf) % 4: buf += b"\0"
    off = len(buf); raw = b"".join(struct.pack(fmt, *x) if isinstance(x, (list, tuple)) else struct.pack(fmt, x) for x in data)
    buf += raw
    views.append({"buffer": 0, "byteOffset": off, "byteLength": len(raw), "target": target})
    a = {"bufferView": len(views)-1, "componentType": comp, "count": count, "type": typ}
    if mn: a["min"], a["max"] = mn, mx
    accs.append(a); return len(accs)-1
for name, (p, n, uv, idx), mat in meshes:
    attrs = {"POSITION": add(p, "<3f", 5126, "VEC3", len(p), 34962, [min(v[i] for v in p) for i in range(3)], [max(v[i] for v in p) for i in range(3)]),
             "NORMAL": add(n, "<3f", 5126, "VEC3", len(n), 34962)}
    if uv: attrs["TEXCOORD_0"] = add(uv, "<2f", 5126, "VEC2", len(uv), 34962)
    gm.append({"name": name, "primitives": [{"attributes": attrs, "indices": add(idx, "<H", 5123, "SCALAR", len(idx), 34963), "material": mat}]})
s = math.sqrt(0.5)
g = {
  "asset": {"version": "2.0", "generator": "reel tests (slab.py)"},
  "scene": 0, "scenes": [{"nodes": [0]}],
  "nodes": [{"name": "Device", "rotation": [-s, 0, 0, s], "children": [1]},
            {"name": "Upright", "rotation": [s, 0, 0, s], "children": [2, 3, 4]},
            {"name": "Body", "mesh": 0}, {"name": "Screen", "mesh": 1}, {"name": "Glass", "mesh": 2}],
  "meshes": gm,
  "materials": [
    {"name": "Body", "pbrMetallicRoughness": {"baseColorFactor": [0.05, 0.05, 0.06, 1], "metallicFactor": 0.9, "roughnessFactor": 0.3}},
    {"name": "Screen", "pbrMetallicRoughness": {"baseColorFactor": [0, 0, 0, 1], "metallicFactor": 0, "roughnessFactor": 0.2}},
    {"name": "Glass", "alphaMode": "BLEND", "pbrMetallicRoughness": {"baseColorFactor": [0.1, 0.1, 0.1, 0.15], "metallicFactor": 0, "roughnessFactor": 0.05}}],
  "accessors": accs, "bufferViews": views,
  "buffers": [{"byteLength": len(buf), "uri": "data:application/octet-stream;base64," + base64.b64encode(buf).decode()}],
}
json.dump(g, open("tests/slab.gltf", "w"), separators=(",", ":"))
