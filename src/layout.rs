//! Boxes laid out by Taffy (CSS flexbox): a tree of rows and columns with padding, gaps and
//! borders, holding text (measured by Parley, wrapped when its box is narrower than it) and
//! fixed-size boxes. The scene builds the tree (`row`, `col`, `text`, `box` in reel.ts); this
//! computes where everything goes, and reel.ts draws it.
use rquickjs::{Array, Ctx, Exception, Object, Value};
use taffy::prelude::*;

use crate::script::{Ts, text_style};
use crate::text as T;

type R<T> = rquickjs::Result<T>;

pub enum Kind {
    Box,
    Text(String, Ts),
}

pub struct Node {
    style: Style,
    kind: Kind,
    children: Vec<Node>,
}

fn num(o: &Object, k: &str) -> R<Option<f32>> {
    let v: Value = o.get(k)?;
    Ok(v.as_number().map(|n| n as f32))
}

fn lp(v: f32) -> LengthPercentage {
    LengthPercentage::length(v)
}

fn padding(v: &Value) -> R<Option<taffy::Rect<LengthPercentage>>> {
    let r = |t, r, b, l| taffy::Rect { top: lp(t), right: lp(r), bottom: lp(b), left: lp(l) };
    if let Some(n) = v.as_number() {
        let n = n as f32;
        return Ok(Some(r(n, n, n, n)));
    }
    if let Some(a) = v.as_array() {
        let n: Vec<f32> = a.iter::<f64>().map(|x| x.map(|x| x as f32)).collect::<R<_>>()?;
        return Ok(match n.len() {
            2 => Some(r(n[0], n[1], n[0], n[1])),
            4 => Some(r(n[0], n[1], n[2], n[3])),
            _ => None,
        });
    }
    Ok(None)
}

/// An El object (reel.ts) → a node.
pub fn from_js<'js>(ctx: &Ctx<'js>, o: &Object<'js>) -> R<Node> {
    let kind: String = o.get("kind")?;
    let mut style = Style { display: Display::Flex, align_items: AlignItems::START, ..Default::default() };
    let st: Option<Object> = o.get("style")?;
    let mut children = Vec::new();
    let k = match kind.as_str() {
        "row" | "col" => {
            style.flex_direction = if kind == "row" { FlexDirection::Row } else { FlexDirection::Column };
            if let Some(cs) = o.get::<_, Option<Array>>("children")? {
                for c in cs.iter::<Object>() {
                    children.push(from_js(ctx, &c?)?);
                }
            }
            Kind::Box
        }
        "text" => {
            let (s, spans) = crate::script::text_arg(ctx, &o.get("s")?)?;
            let mut ts = text_style(ctx, st.as_ref())?;
            ts.spans = spans;
            Kind::Text(s, ts)
        }
        "box" => {
            let w = num(o, "w")?.unwrap_or(0.0);
            let h = num(o, "h")?.unwrap_or(0.0);
            style.size = Size { width: length(w), height: length(h) };
            style.flex_shrink = 0.0;
            Kind::Box
        }
        k => return Err(Exception::throw_type(ctx, &format!("layout: unknown element kind {k:?} (use row, col, text, box)"))),
    };
    if let Some(st) = &st {
        if let Some(p) = padding(&st.get("padding")?)? {
            style.padding = p;
        }
        if let Some(g) = num(st, "gap")? {
            style.gap = Size { width: lp(g), height: lp(g) };
        }
        if let Some(a) = st.get::<_, Option<String>>("align")? {
            style.align_items = match a.as_str() {
                "center" => AlignItems::CENTER,
                "end" => AlignItems::END,
                "stretch" => AlignItems::STRETCH,
                _ => AlignItems::START,
            };
        }
        if let Some(j) = st.get::<_, Option<String>>("justify")? {
            style.justify_content = match j.as_str() {
                "center" => JustifyContent::CENTER,
                "end" => JustifyContent::END,
                "between" => JustifyContent::SPACE_BETWEEN,
                "around" => JustifyContent::SPACE_AROUND,
                "evenly" => JustifyContent::SPACE_EVENLY,
                _ => JustifyContent::START,
            };
        }
        if let Some(v) = num(st, "width")? {
            style.size.width = length(v);
        }
        if let Some(v) = num(st, "height")? {
            style.size.height = length(v);
        }
        if let Some(v) = num(st, "minWidth")? {
            style.min_size.width = length(v);
        }
        if let Some(v) = num(st, "minHeight")? {
            style.min_size.height = length(v);
        }
        if let Some(v) = num(st, "maxWidth")? {
            style.max_size.width = length(v);
        }
        if let Some(v) = num(st, "grow")? {
            style.flex_grow = v;
        }
        let look: Option<Object> = st.get("look")?;
        if let Some(l) = look
            && let Some(b) = l.get::<_, Option<Object>>("border")?
        {
            let w = lp(num(&b, "width")?.unwrap_or(1.0));
            style.border = taffy::Rect { top: w, right: w, bottom: w, left: w };
        }
    }
    if matches!(k, Kind::Text(..)) {
        // (a text node's style is its TextStyle: only these box properties apply)
        style.size = Size::auto();
        style.padding = taffy::Rect::zero();
        style.border = taffy::Rect::zero();
    }
    Ok(Node { style, kind: k, children })
}

fn build(tree: &mut TaffyTree<usize>, n: &Node, next: &mut usize) -> NodeId {
    let me = *next;
    *next += 1;
    let kids: Vec<NodeId> = n.children.iter().map(|c| build(tree, c, next)).collect();
    if kids.is_empty() {
        tree.new_leaf_with_context(n.style.clone(), me).unwrap()
    } else {
        let id = tree.new_with_children(n.style.clone(), &kids).unwrap();
        tree.set_node_context(id, Some(me)).unwrap();
        id
    }
}

fn flatten<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    out.push(n);
    for c in &n.children {
        flatten(c, out);
    }
}

fn one_line(s: &str, t: &Ts) -> f64 {
    T::measure(s, &t.font, t.size, t.weight, t.ls)
}

fn text_size(k: &Kind, max_w: Option<f32>) -> Size<f32> {
    let Kind::Text(s, t) = k else { return Size::ZERO };
    let one = one_line(s, t);
    let line = T::line_metrics(&t.font, t.size, t.lh).0;
    match max_w {
        Some(w) if (one as f32) > w + 0.01 => {
            let p = T::shape(&T::ParaKey::new(s, &t.font, t.size, t.weight, t.ls, t.lh, Some(w as f64), t.align).with(&t.spans, t.balance));
            Size { width: p.width as f32, height: (p.baselines.len() as f64 * line) as f32 }
        }
        _ => Size { width: one as f32, height: line as f32 },
    }
}

/// Every node's border box, in the order the tree is walked (a node, then its children):
/// x, y, w, h and whether its text wraps, five numbers each.
pub fn compute(root_node: &Node) -> Vec<f64> {
    let mut tree: TaffyTree<usize> = TaffyTree::new();
    // (fractional sizes, as the browser keeps them: rounding would wrap a text a hair too wide)
    tree.disable_rounding();
    let root = build(&mut tree, root_node, &mut 0);
    let mut flat = Vec::new();
    flatten(root_node, &mut flat);
    tree.compute_layout_with_measure(root, Size::MAX_CONTENT, |inputs, _id, ctx, style| {
        let kind = ctx.map(|i| &flat[*i].kind);
        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
            if let Size { width: Some(width), height: Some(height) } = known {
                return Size { width, height };
            }
            let max_w = known.width.or(match avail.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            kind.map_or(Size::ZERO, |k| text_size(k, max_w))
        })
    })
    .unwrap();
    let mut out = Vec::new();
    fn walk(tree: &TaffyTree<usize>, n: NodeId, ox: f64, oy: f64, out: &mut Vec<[f64; 4]>) {
        let l = tree.layout(n).unwrap();
        let (x, y) = (ox + l.location.x as f64, oy + l.location.y as f64);
        out.push([x, y, l.size.width as f64, l.size.height as f64]);
        for c in tree.children(n).unwrap() {
            walk(tree, c, x, y, out);
        }
    }
    let mut rects = Vec::new();
    walk(&tree, root, 0.0, 0.0, &mut rects);
    for (n, r) in flat.iter().zip(&rects) {
        let wraps = match &n.kind {
            Kind::Text(s, t) => one_line(s, t) > r[2] + 0.01,
            Kind::Box => false,
        };
        out.extend_from_slice(r);
        out.push(if wraps { 1.0 } else { 0.0 });
    }
    out
}
