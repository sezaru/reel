//! Executes display lists on the GPU (wgpu, headless Vulkan): shapes into a half-float frame,
//! groups into layers (opacity, Gaussian blur, masks), sub-frames into an accumulation buffer
//! for motion blur, and the result straight to BT.709 NV12 planes for the encoder.
mod mesh;

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::*;

use crate::canvas::{Blend, Clip, Col, Fill, Layer, Mask, Op, TexKey};

const STRIDE: u64 = 512;
const U_SIZE: u64 = 480;
type U = [[f32; 4]; 32];

// the uniform struct's fields (shaders.wgsl `U`)
const H0: usize = 0;
const QUAD: usize = 3;
const RECT: usize = 4;
const RADIUS: usize = 5;
const COLOR: usize = 6;
const STOPS: usize = 7;
const POS: usize = 11;
const UV: usize = 12;
const BCOLOR: usize = 13;
const IN0: usize = 14;
const CLIP: usize = 18;
const CLIPR: usize = 19;
const HP0: usize = 20;
const P0: usize = 22;
const P1: usize = 23;
const VIEW: usize = 24;
const CM: usize = 25;
const CUT: usize = 28;
const CUTR: usize = 29;

/// A decoded image: premultiplied RGBA8.
pub struct Decoded {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

struct Target {
    view: TextureView,
    /// sampled through the filtering sampler (blur, mips)
    bg_t: Option<BindGroup>,
    /// read texel by texel (composite, accumulate, convert)
    bg_n: BindGroup,
}

struct TexEntry {
    bg: BindGroup,
    /// (models sample images: a screen's)
    view: Option<TextureView>,
    bytes: u64,
    used: u64,
}

struct PathEntry {
    bg: BindGroup,
    n: u32,
    used: u64,
}

struct Pipes {
    box_normal: RenderPipeline,
    box_multiply: RenderPipeline,
    shadow: RenderPipeline,
    stroke: RenderPipeline,
    comp: RenderPipeline,
    tint: RenderPipeline,
    blur: RenderPipeline,
    accum: RenderPipeline,
    y: RenderPipeline,
    uv: RenderPipeline,
    rgb: RenderPipeline,
    mip: RenderPipeline,
}

struct Readback {
    buf: Buffer,
    ready: Arc<AtomicBool>,
    sub: Option<SubmissionIndex>,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub ops: u64,
    pub layers: u64,
    pub uploaded: u64,
}

pub struct Gpu {
    device: Device,
    queue: Queue,
    pub w: u32,
    pub h: u32,
    /// the design size (what scenes draw in)
    pub design: (f64, f64),
    pub scale: f64,
    /// premultiplied
    background: [f64; 4],
    pub adapter: String,
    l1t: BindGroupLayout,
    l1n: BindGroupLayout,
    l1p: BindGroupLayout,
    l0: BindGroupLayout,
    ubuf: Buffer,
    ucap: u64,
    ubg: BindGroup,
    udata: Vec<u8>,
    p: Pipes,
    smp_tex: Sampler,
    smp_lin: Sampler,
    /// [0] is the frame; the rest are layers
    layers: Vec<Target>,
    free: Vec<usize>,
    accum: Target,
    yt: (Texture, Target),
    uvt: (Texture, Target),
    rgbt: (Texture, Target),
    textures: HashMap<TexKey, TexEntry>,
    tex_bytes: u64,
    budget: u64,
    paths: HashMap<u64, PathEntry>,
    stamp: u64,
    ring: Vec<Readback>,
    ypitch: u32,
    uvpitch: u32,
    pub stats: Stats,
    /// the window's pipelines, one per surface format (preview)
    present: HashMap<TextureFormat, RenderPipeline>,
    pl_n: PipelineLayout,
    pl_u: PipelineLayout,
    m_src: ShaderModule,
    /// scenes' custom shaders, by code id
    custom: HashMap<u64, RenderPipeline>,
    /// 3D models (gpu/mesh.rs)
    meshes: mesh::Meshes,
}

fn align(v: u32, a: u32) -> u32 {
    v.div_ceil(a) * a
}

fn shader_sources() -> HashMap<&'static str, String> {
    let src = include_str!("shaders.wgsl");
    let mut parts = src.split("//#section ");
    let common = parts.next().unwrap();
    let mut out = HashMap::new();
    for p in parts {
        let (name, body) = p.split_once('\n').unwrap();
        let name: &'static str = match name.trim() {
            "box" => "box",
            "shadow" => "shadow",
            "stroke" => "stroke",
            "src" => "src",
            n => panic!("unknown shader section {n}"),
        };
        out.insert(name, format!("{common}\n{body}"));
    }
    out
}

impl Gpu {
    /// Headless: renders to textures only.
    pub fn new(design: (u32, u32), scale: f64, budget: u64, background: Col) -> anyhow::Result<Gpu> {
        let instance = Instance::new(InstanceDescriptor { backends: Backends::VULKAN | Backends::GL, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))?;
        Self::from_adapter(&adapter, design, scale, budget, background)
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn from_adapter(adapter: &Adapter, design: (u32, u32), scale: f64, budget: u64, background: Col) -> anyhow::Result<Gpu> {
        let info = adapter.get_info();
        let blendable = adapter.features().contains(Features::FLOAT32_BLENDABLE);
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: None,
            required_features: if blendable { Features::FLOAT32_BLENDABLE } else { Features::empty() },
            required_limits: Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }))?;
        let accum_fmt = if blendable { TextureFormat::Rgba32Float } else { TextureFormat::Rgba16Float };
        // (even: NV12 halves both)
        let w = ((design.0 as f64 * scale).round() as u32 / 2 * 2).max(2);
        let h = ((design.1 as f64 * scale).round() as u32 / 2 * 2).max(2);

        let l0 = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("uniforms"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: NonZeroU64::new(U_SIZE) },
                count: None,
            }],
        });
        let l1t = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture { sample_type: TextureSampleType::Float { filterable: true }, view_dimension: TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                BindGroupLayoutEntry { binding: 1, visibility: ShaderStages::FRAGMENT, ty: BindingType::Sampler(SamplerBindingType::Filtering), count: None },
            ],
        });
        let l1n = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("texels"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture { sample_type: TextureSampleType::Float { filterable: false }, view_dimension: TextureViewDimension::D2, multisampled: false },
                count: None,
            }],
        });
        let storage = |binding| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer { ty: BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let l1p = device.create_bind_group_layout(&BindGroupLayoutDescriptor { label: Some("path"), entries: &[storage(0), storage(1)] });

        let src = shader_sources();
        let module = |name: &str| device.create_shader_module(ShaderModuleDescriptor { label: Some(name), source: ShaderSource::Wgsl(src[name].as_str().into()) });
        let (m_box, m_shadow, m_stroke, m_src) = (module("box"), module("shadow"), module("stroke"), module("src"));
        let layout = |groups: &[&BindGroupLayout]| {
            let g: Vec<Option<&BindGroupLayout>> = groups.iter().map(|g| Some(*g)).collect();
            device.create_pipeline_layout(&PipelineLayoutDescriptor { label: None, bind_group_layouts: &g, immediate_size: 0 })
        };
        let (pl_u, pl_t, pl_n, pl_p) = (layout(&[&l0]), layout(&[&l0, &l1t]), layout(&[&l0, &l1n]), layout(&[&l0, &l1p]));
        let premul = BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        let multiply = BlendState {
            color: BlendComponent { src_factor: BlendFactor::Dst, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
            alpha: BlendComponent { src_factor: BlendFactor::DstAlpha, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
        };
        let add = BlendState {
            color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
            alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
        };
        let f16 = TextureFormat::Rgba16Float;
        let pipe = |m: &ShaderModule, pl: &PipelineLayout, vs: &str, fs: &str, fmt: TextureFormat, blend: Option<BlendState>| {
            let strip = vs == "vs_shape";
            device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(pl),
                vertex: VertexState { module: m, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[] },
                primitive: PrimitiveState {
                    topology: if strip { PrimitiveTopology::TriangleStrip } else { PrimitiveTopology::TriangleList },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                fragment: Some(FragmentState {
                    module: m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState { format: fmt, blend, write_mask: ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let p = Pipes {
            box_normal: pipe(&m_box, &pl_t, "vs_shape", "fs_box", f16, Some(premul)),
            box_multiply: pipe(&m_box, &pl_t, "vs_shape", "fs_box", f16, Some(multiply)),
            shadow: pipe(&m_shadow, &pl_u, "vs_shape", "fs_shadow", f16, Some(premul)),
            stroke: pipe(&m_stroke, &pl_p, "vs_shape", "fs_stroke", f16, Some(premul)),
            comp: pipe(&m_src, &pl_n, "vs_full", "fs_comp", f16, Some(premul)),
            tint: pipe(&m_src, &pl_n, "vs_full", "fs_tint", f16, None),
            blur: pipe(&m_box, &pl_t, "vs_full", "fs_blur", f16, None),
            accum: pipe(&m_src, &pl_n, "vs_full", "fs_accum", accum_fmt, Some(add)),
            y: pipe(&m_src, &pl_n, "vs_full", "fs_y", TextureFormat::R8Unorm, None),
            uv: pipe(&m_src, &pl_n, "vs_full", "fs_uv", TextureFormat::Rg8Unorm, None),
            rgb: pipe(&m_src, &pl_n, "vs_full", "fs_rgb", TextureFormat::Rgba8Unorm, None),
            mip: pipe(&m_box, &pl_t, "vs_full", "fs_mip", TextureFormat::Rgba8Unorm, None),
        };

        let smp_tex = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let smp_lin = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        let ucap = 4 << 20;
        let ubuf = device.create_buffer(&BufferDescriptor { label: Some("uniforms"), size: ucap, usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST, mapped_at_creation: false });
        let ubg = Self::uniform_group(&device, &l0, &ubuf);

        let ypitch = align(w, 256);
        let uvpitch = align(w / 2 * 2, 256);
        let rb_size = (ypitch * h + uvpitch * (h / 2)) as u64;
        let ring = (0..3)
            .map(|_| Readback {
                buf: device.create_buffer(&BufferDescriptor { label: Some("readback"), size: rb_size, usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST, mapped_at_creation: false }),
                ready: Arc::new(AtomicBool::new(false)),
                sub: None,
            })
            .collect();

        let mk = |w, h, fmt, filterable| make_target(&device, &l1t, &l1n, &smp_lin, w, h, fmt, filterable);
        let (main, accum) = (mk(w, h, f16, true).1, mk(w, h, accum_fmt, false).1);
        let (yt, uvt, rgbt) = (mk(w, h, TextureFormat::R8Unorm, false), mk(w / 2, h / 2, TextureFormat::Rg8Unorm, false), mk(w, h, TextureFormat::Rgba8Unorm, false));
        let g = Gpu {
            present: HashMap::new(),
            custom: HashMap::new(),
            meshes: Default::default(),
            pl_n: pl_n.clone(),
            pl_u: pl_u.clone(),
            design: (design.0 as f64, design.1 as f64),
            background: { let a = background[3]; [background[0] * a, background[1] * a, background[2] * a, a] },
            m_src: m_src.clone(),
            adapter: format!("{} ({:?}, {})", info.name, info.backend, info.driver_info),
            device,
            queue,
            w,
            h,
            scale,
            l1t,
            l1n,
            l1p,
            l0,
            ubuf,
            ucap,
            ubg,
            udata: Vec::new(),
            p,
            smp_tex,
            smp_lin,
            layers: vec![main],
            free: Vec::new(),
            accum,
            yt,
            uvt,
            rgbt,
            textures: HashMap::new(),
            tex_bytes: 0,
            budget,
            paths: HashMap::new(),
            stamp: 0,
            ring,
            ypitch,
            uvpitch,
            stats: Stats::default(),
        };
        Ok(g)
    }

    fn uniform_group(device: &Device, l0: &BindGroupLayout, buf: &Buffer) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: l0,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(BufferBinding { buffer: buf, offset: 0, size: NonZeroU64::new(U_SIZE) }) }],
        })
    }

    fn target(&self, w: u32, h: u32, fmt: TextureFormat, filterable: bool) -> (Texture, Target) {
        make_target(&self.device, &self.l1t, &self.l1n, &self.smp_lin, w, h, fmt, filterable)
    }

    fn layer(&mut self) -> usize {
        if let Some(i) = self.free.pop() {
            return i;
        }
        let t = self.target(self.w, self.h, TextureFormat::Rgba16Float, true).1;
        self.layers.push(t);
        self.layers.len() - 1
    }

    // ── uniforms ─────────────────────────────────────────────────────────────────────────────
    /// A draw's uniforms, zeroed but for the view (half the design size, the scale).
    fn u0(&self) -> U {
        let mut u: U = [[0.0; 4]; 32];
        u[VIEW] = [(self.design.0 / 2.0) as f32, (self.design.1 / 2.0) as f32, self.scale as f32, 0.0];
        u
    }

    /// Appends one draw's uniforms; returns its dynamic offset.
    fn push_u(&mut self, u: &U) -> u32 {
        let off = self.udata.len();
        self.udata.extend_from_slice(bytemuck::cast_slice(u));
        self.udata.resize(off + STRIDE as usize, 0);
        off as u32
    }

    fn ensure_ucap(&mut self, need: u64) {
        if need <= self.ucap {
            return;
        }
        self.ucap = need.next_power_of_two();
        self.ubuf = self.device.create_buffer(&BufferDescriptor { label: Some("uniforms"), size: self.ucap, usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST, mapped_at_creation: false });
        self.ubg = Self::uniform_group(&self.device, &self.l0, &self.ubuf);
    }

    // ── textures ─────────────────────────────────────────────────────────────────────────────

    /// RGBA8 pixels into a texture with its whole mip chain.
    fn mip_texture(&mut self, enc: &mut CommandEncoder, d: &Decoded) -> Texture {
        let levels = 32 - d.w.max(d.h).leading_zeros();
        let tex = self.device.create_texture(&TextureDescriptor {
            label: None,
            size: Extent3d { width: d.w, height: d.h, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.queue.write_texture(
            TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            &d.data,
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * d.w), rows_per_image: Some(d.h) },
            Extent3d { width: d.w, height: d.h, depth_or_array_layers: 1 },
        );
        self.mips(enc, &tex, levels);
        tex
    }

    /// A texture's mip chain from its level 0, each level a 2×2 box of the one above.
    fn mips(&mut self, enc: &mut CommandEncoder, tex: &Texture, levels: u32) {
        let level = |l: u32| tex.create_view(&TextureViewDescriptor { base_mip_level: l, mip_level_count: Some(1), ..Default::default() });
        for l in 1..levels {
            let src = level(l - 1);
            let dst = level(l);
            let bg = self.device.create_bind_group(&BindGroupDescriptor {
                label: None,
                layout: &self.l1t,
                entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&src) }, BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&self.smp_lin) }],
            });
            let mut pass = begin(enc, &dst, true);
            pass.set_pipeline(&self.p.mip);
            pass.set_bind_group(0, &self.ubg, &[0]);
            pass.set_bind_group(1, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn upload(&mut self, enc: &mut CommandEncoder, key: &TexKey, d: &Decoded) {
        let tex = self.mip_texture(enc, d);
        let view = tex.create_view(&Default::default());
        let bg = self.device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &self.l1t,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view) }, BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&self.smp_tex) }],
        });
        let bytes = d.w as u64 * d.h as u64 * 4 * 4 / 3;
        self.tex_bytes += bytes;
        self.textures.insert(key.clone(), TexEntry { bg, view: Some(view), bytes, used: self.stamp });
        self.stats.uploaded += 1;
    }

    /// Drops the least recently used textures (not this frame's) over the budget; returns the
    /// keys dropped.
    fn evict(&mut self) -> Vec<TexKey> {
        let mut out = Vec::new();
        if self.tex_bytes <= self.budget {
            return out;
        }
        let mut old: Vec<(u64, TexKey)> = self.textures.iter().filter(|(_, e)| e.used < self.stamp).map(|(k, e)| (e.used, k.clone())).collect();
        old.sort_by_key(|o| o.0);
        for (_, k) in old {
            if self.tex_bytes <= self.budget {
                break;
            }
            let e = self.textures.remove(&k).unwrap();
            self.tex_bytes -= e.bytes;
            out.push(k);
        }
        out
    }

    fn path_entry(&mut self, p: &crate::paths::Path) {
        if let Some(e) = self.paths.get_mut(&p.id) {
            e.used = self.stamp;
            return;
        }
        let mk = |data: &[u8], label| {
            let b = self.device.create_buffer(&BufferDescriptor { label: Some(label), size: data.len() as u64, usage: BufferUsages::STORAGE | BufferUsages::COPY_DST, mapped_at_creation: false });
            self.queue.write_buffer(&b, 0, data);
            b
        };
        let segs = mk(bytemuck::cast_slice(&p.segs), "segs");
        let starts = mk(bytemuck::cast_slice(&p.starts), "starts");
        let bg = self.device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &self.l1p,
            entries: &[BindGroupEntry { binding: 0, resource: segs.as_entire_binding() }, BindGroupEntry { binding: 1, resource: starts.as_entire_binding() }],
        });
        self.paths.insert(p.id, PathEntry { bg, n: p.segs.len() as u32, used: self.stamp });
        if self.paths.len() > 512 {
            let stamp = self.stamp;
            self.paths.retain(|_, e| e.used + 2 >= stamp);
        }
    }

    /// Makes everything the frame's lists sample resident: textures (decoded by `get`) and
    /// path buffers.
    fn prepare(&mut self, enc: &mut CommandEncoder, samples: &[Vec<Op>], get: &mut dyn FnMut(&TexKey) -> Arc<Decoded>) -> Vec<TexKey> {
        for ops in samples {
            for op in ops {
                match op {
                    Op::Box(b) => {
                        if let Fill::Tex { key, .. } = &b.fill {
                            let (img, model) = match key {
                                TexKey::Model(m) => (m.screen.as_ref(), Some(m)),
                                k => (Some(k), None),
                            };
                            if let Some(k) = img {
                                if let Some(e) = self.textures.get_mut(k) {
                                    e.used = self.stamp;
                                } else {
                                    let d = get(k);
                                    self.upload(enc, k, &d);
                                }
                            }
                            if let Some(m) = model {
                                if let Some(e) = self.textures.get_mut(key) {
                                    e.used = self.stamp;
                                } else {
                                    self.render_model(enc, key, m);
                                }
                            }
                        }
                    }
                    Op::Stroke(s) => self.path_entry(&s.path),
                    _ => {}
                }
            }
        }
        self.evict()
    }

    // ── drawing ──────────────────────────────────────────────────────────────────────────────
    fn draw_box(&mut self, pass: &mut RenderPass<'static>, b: &crate::canvas::BoxOp) {
        let mut u = self.u0();
        set_h(&mut u, &b.h);
        u[QUAD] = f4(b.quad);
        u[RECT] = f4(b.rect);
        u[RADIUS] = f4(b.radius);
        u[BCOLOR] = b.bcolor;
        let fill = match &b.fill {
            Fill::Solid(c) => {
                u[COLOR] = *c;
                0.0
            }
            Fill::Grad { angle, stops } => {
                for i in 0..4 {
                    let s = &stops[i.min(stops.len() - 1)];
                    u[STOPS + i] = s.0;
                    u[POS][i] = s.1 as f32;
                }
                u[P0][1] = stops.len() as f32;
                u[P0][2] = *angle as f32;
                1.0
            }
            Fill::Tex { uv, .. } => {
                u[UV] = f4(*uv);
                2.0
            }
        };
        u[P0][0] = fill;
        u[P0][3] = b.border as f32;
        u[P1][0] = b.opacity as f32;
        u[P1][1] = b.feather as f32;
        for (i, (s, c)) in b.insets.iter().enumerate() {
            u[IN0 + 2 * i] = f4(*s);
            u[IN0 + 2 * i + 1] = *c;
        }
        set_clip(&mut u, &b.clip);
        for (i, hp) in b.hp.iter().take(2).enumerate() {
            u[HP0 + i] = [hp[0] as f32, hp[1] as f32, hp[2] as f32, 0.0];
        }
        let off = self.push_u(&u);
        let tbg = match &b.fill {
            Fill::Tex { key, .. } => self.textures[key].bg.clone(),
            _ => self.dummy_bg(),
        };
        pass.set_pipeline(if b.blend == Blend::Multiply { &self.p.box_multiply } else { &self.p.box_normal });
        pass.set_bind_group(0, &self.ubg, &[off]);
        pass.set_bind_group(1, &tbg, &[]);
        pass.draw(0..4, 0..1);
    }

    /// A bind group for boxes that sample nothing (a 1×1 texture).
    fn dummy_bg(&mut self) -> BindGroup {
        let key = TexKey::Img(Arc::from(""), 0);
        if let Some(e) = self.textures.get(&key) {
            return e.bg.clone();
        }
        let tex = self.device.create_texture(&TextureDescriptor {
            label: Some("dummy"),
            size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let bg = self.device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &self.l1t,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view) }, BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&self.smp_tex) }],
        });
        // pinned: never evicted
        self.textures.insert(key, TexEntry { bg: bg.clone(), view: None, bytes: 0, used: u64::MAX });
        bg
    }

    fn draw_shadow(&mut self, pass: &mut RenderPass<'static>, s: &crate::canvas::ShadowOp) {
        let mut u = self.u0();
        set_h(&mut u, &s.h);
        u[QUAD] = f4(s.quad);
        u[RECT] = f4(s.rect);
        u[COLOR] = s.color;
        u[P0] = [s.sigma as f32, s.radius as f32, 0.0, 0.0];
        if let Some((r, rr)) = s.cut {
            u[CUT] = f4(r);
            u[CUTR] = f4(rr);
        }
        set_clip(&mut u, &s.clip);
        let off = self.push_u(&u);
        pass.set_pipeline(&self.p.shadow);
        pass.set_bind_group(0, &self.ubg, &[off]);
        pass.draw(0..4, 0..1);
    }

    fn draw_stroke(&mut self, pass: &mut RenderPass<'static>, s: &crate::canvas::StrokeOp) {
        let n = self.paths[&s.path.id].n;
        let mut u = self.u0();
        set_h(&mut u, &s.h);
        u[QUAD] = f4(s.quad);
        u[COLOR] = s.color;
        u[P0] = [s.a as f32, s.b as f32, s.hw as f32, n as f32];
        u[P1][0] = if s.fill { 1.0 } else { 0.0 };
        u[P1][1] = if s.evenodd { 1.0 } else { 0.0 };
        set_clip(&mut u, &s.clip);
        let off = self.push_u(&u);
        pass.set_pipeline(&self.p.stroke);
        pass.set_bind_group(0, &self.ubg, &[off]);
        pass.set_bind_group(1, &self.paths[&s.path.id].bg, &[]);
        pass.draw(0..4, 0..1);
    }

    fn draw_shader(&mut self, pass: &mut RenderPass<'static>, s: &crate::canvas::ShaderOp) {
        use crate::shader::*;
        if !self.custom.contains_key(&s.code.id) {
            let m = self.device.create_shader_module(ShaderModuleDescriptor { label: Some(&s.code.name), source: ShaderSource::Wgsl(s.code.wgsl.as_str().into()) });
            let p = self.device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some(&s.code.name),
                layout: Some(&self.pl_u),
                vertex: VertexState { module: &m, entry_point: Some("reel_vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: PrimitiveState { topology: PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                fragment: Some(FragmentState {
                    module: &m,
                    entry_point: Some("reel_fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState { format: TextureFormat::Rgba16Float, blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.custom.insert(s.code.id, p);
        }
        let mut u: U = [[0.0; 4]; 32];
        set_h(&mut u, &s.h);
        u[QUAD] = f4(s.quad);
        u[SLOT_RECT] = f4(s.rect);
        u[SLOT_VIEW] = [(self.design.0 / 2.0) as f32, (self.design.1 / 2.0) as f32, self.scale as f32, s.f as f32];
        if let Some((r, rr, m)) = &s.clip {
            u[SLOT_CLIP] = f4(*r);
            u[SLOT_CLIPR] = f4(*rr);
            for i in 0..3 {
                u[SLOT_CM + i] = [m[i][0] as f32, m[i][1] as f32, m[i][2] as f32, 0.0];
            }
        }
        for (i, c) in s.params.chunks(4).enumerate() {
            for (j, v) in c.iter().enumerate() {
                u[SLOT_P + i][j] = *v;
            }
        }
        let off = self.push_u(&u);
        pass.set_pipeline(&self.custom[&s.code.id]);
        pass.set_bind_group(0, &self.ubg, &[off]);
        pass.draw(0..4, 0..1);
    }

    fn blur(&mut self, enc: &mut CommandEncoder, layer: usize, sx: f64, sy: f64) {
        let tmp = self.layer();
        for (src, dst, dir, s) in [(layer, tmp, [1.0 / self.w as f32, 0.0], sx), (tmp, layer, [0.0, 1.0 / self.h as f32], sy)] {
            let mut u = self.u0();
            u[P0] = [dir[0], dir[1], (s * self.scale) as f32, 0.0];
            let off = self.push_u(&u);
            let mut pass = begin(enc, &self.layers[dst].view, false);
            pass.set_pipeline(&self.p.blur);
            pass.set_bind_group(0, &self.ubg, &[off]);
            pass.set_bind_group(1, self.layers[src].bg_t.as_ref().unwrap(), &[]);
            pass.draw(0..3, 0..1);
        }
        self.free.push(tmp);
    }

    /// One display list into the frame target (layers 0).
    /// Composites layer `l` onto `parent` with the layer's opacity and mask.
    fn composite(&mut self, enc: &mut CommandEncoder, l: usize, parent: usize, layer: &Layer) {
        let mut u = self.u0();
        u[P0][0] = layer.opacity as f32;
        match &layer.mask {
            Some(Mask::Ellipse(m, ramp)) => {
                u[P0][1] = 1.0;
                u[RECT] = f4(*m);
                u[P1] = [ramp[0] as f32, ramp[1] as f32, self.scale as f32, 0.0];
            }
            Some(Mask::Linear(line, stops)) => {
                u[P0][1] = 2.0;
                u[RECT] = f4(*line);
                for (i, (a, p)) in stops.iter().take(4).enumerate() {
                    u[COLOR][i] = *a as f32;
                    u[POS][i] = *p as f32;
                }
                u[P1] = [0.0, 0.0, self.scale as f32, stops.len().min(4) as f32];
            }
            None => {}
        }
        let off = self.push_u(&u);
        let mut pass = begin(enc, &self.layers[parent].view, false);
        pass.set_pipeline(&self.p.comp);
        pass.set_bind_group(0, &self.ubg, &[off]);
        pass.set_bind_group(1, &self.layers[l].bg_n, &[]);
        pass.draw(0..3, 0..1);
    }

    /// One display list into the frame target (layers 0).
    fn draw(&mut self, enc: &mut CommandEncoder, ops: &[Op]) {
        let mut stack: Vec<(usize, Option<Layer>)> = vec![(0, None)];
        let bg = self.background;
        let mut pass = begin_with(enc, &self.layers[0].view, LoadOp::Clear(Color { r: bg[0], g: bg[1], b: bg[2], a: bg[3] }));
        let mut skip = 0;
        for op in ops {
            if skip > 0 {
                match op {
                    Op::Push(..) => skip += 1,
                    Op::Pop => skip -= 1,
                    _ => {}
                }
                continue;
            }
            match op {
                Op::Push(layer) => {
                    if layer.opacity <= 0.002 {
                        skip = 1;
                        continue;
                    }
                    let l = self.layer();
                    drop(pass);
                    pass = begin(enc, &self.layers[l].view, true);
                    stack.push((l, Some((**layer).clone())));
                }
                Op::Pop => {
                    let (l, layer) = stack.pop().unwrap();
                    let layer = layer.unwrap();
                    drop(pass);
                    let (bx, by) = layer.blur;
                    if bx > 0.3 || by > 0.3 {
                        self.blur(enc, l, bx, by);
                    }
                    let parent = stack.last().unwrap().0;
                    // a drop shadow: the layer's alpha, moved, blurred, tinted, under it
                    if let Some(([dx, dy, b], col)) = layer.shadow {
                        let s = self.layer();
                        let mut u = self.u0();
                        u[P0] = [(dx * self.scale) as f32, (dy * self.scale) as f32, 0.0, 0.0];
                        u[COLOR] = col;
                        let off = self.push_u(&u);
                        {
                            let mut tp = begin(enc, &self.layers[s].view, true);
                            tp.set_pipeline(&self.p.tint);
                            tp.set_bind_group(0, &self.ubg, &[off]);
                            tp.set_bind_group(1, &self.layers[l].bg_n, &[]);
                            tp.draw(0..3, 0..1);
                        }
                        // (CSS: the blur radius is twice the deviation)
                        if b > 0.6 {
                            self.blur(enc, s, b / 2.0, b / 2.0);
                        }
                        self.composite(enc, s, parent, &layer);
                        self.free.push(s);
                    }
                    self.composite(enc, l, parent, &layer);
                    pass = begin(enc, &self.layers[parent].view, false);
                    self.free.push(l);
                    self.stats.layers += 1;
                }
                Op::Box(b) => self.draw_box(&mut pass, b),
                Op::Shadow(s) => self.draw_shadow(&mut pass, s),
                Op::Stroke(s) => self.draw_stroke(&mut pass, s),
                Op::Shader(s) => self.draw_shader(&mut pass, s),
            }
            self.stats.ops += 1;
        }
        drop(pass);
    }

    /// Draws the sub-frames of one frame, averaged; returns which target holds it (None: the
    /// frame target, Some: the accumulation buffer).
    fn frame(&mut self, enc: &mut CommandEncoder, samples: &[Vec<Op>]) -> bool {
        if samples.len() == 1 {
            self.draw(enc, &samples[0]);
            return false;
        }
        let w = 1.0 / samples.len() as f32;
        for (i, ops) in samples.iter().enumerate() {
            self.draw(enc, ops);
            let mut u = self.u0();
            u[P0][0] = w;
            let off = self.push_u(&u);
            let mut pass = begin(enc, &self.accum.view, i == 0);
            pass.set_pipeline(&self.p.accum);
            pass.set_bind_group(0, &self.ubg, &[off]);
            pass.set_bind_group(1, &self.layers[0].bg_n, &[]);
            pass.draw(0..3, 0..1);
        }
        true
    }

    /// Records everything for one frame: returns the encoder (not yet submitted) and the
    /// keys of textures evicted.
    fn record(&mut self, samples: &[Vec<Op>], get: &mut dyn FnMut(&TexKey) -> Arc<Decoded>) -> (CommandEncoder, bool, Vec<TexKey>) {
        self.stamp += 1;
        self.udata.clear();
        // room for every draw: each op, plus composites, blurs and accumulation
        let draws: usize = samples.iter().map(|s| s.len() * 2 + 8).sum::<usize>() + 16;
        self.ensure_ucap(draws as u64 * STRIDE);
        let mut enc = self.device.create_command_encoder(&Default::default());
        let evicted = self.prepare(&mut enc, samples, get);
        let accum = self.frame(&mut enc, samples);
        (enc, accum, evicted)
    }

    fn convert(&mut self, enc: &mut CommandEncoder, accum: bool, pipe: &RenderPipeline, view: &TextureView, seed: f32) {
        let mut u = self.u0();
        u[P0][0] = seed;
        let off = self.push_u(&u);
        let src = if accum { &self.accum.bg_n } else { &self.layers[0].bg_n };
        let mut pass = begin(enc, view, true);
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &self.ubg, &[off]);
        pass.set_bind_group(1, src, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Draws a frame and starts its NV12 readback into ring slot `slot`; returns the evicted
    /// texture keys.
    pub fn submit_nv12(&mut self, samples: &[Vec<Op>], f: u64, slot: usize, get: &mut dyn FnMut(&TexKey) -> Arc<Decoded>) -> Vec<TexKey> {
        let (mut enc, accum, evicted) = self.record(samples, get);
        let (yp, uvp) = (self.p.y.clone(), self.p.uv.clone());
        let (yv, uvv) = (self.yt.1.view.clone(), self.uvt.1.view.clone());
        self.convert(&mut enc, accum, &yp, &yv, (f % 997) as f32);
        self.convert(&mut enc, accum, &uvp, &uvv, (f % 991) as f32);
        let rb = &self.ring[slot];
        let yrows = self.ypitch * self.h;
        enc.copy_texture_to_buffer(
            TexelCopyTextureInfo { texture: &self.yt.0, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            TexelCopyBufferInfo { buffer: &rb.buf, layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.ypitch), rows_per_image: Some(self.h) } },
            Extent3d { width: self.w, height: self.h, depth_or_array_layers: 1 },
        );
        enc.copy_texture_to_buffer(
            TexelCopyTextureInfo { texture: &self.uvt.0, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            TexelCopyBufferInfo { buffer: &rb.buf, layout: TexelCopyBufferLayout { offset: yrows as u64, bytes_per_row: Some(self.uvpitch), rows_per_image: Some(self.h / 2) } },
            Extent3d { width: self.w / 2, height: self.h / 2, depth_or_array_layers: 1 },
        );
        self.queue.write_buffer(&self.ubuf, 0, &self.udata);
        let sub = self.queue.submit([enc.finish()]);
        let rb = &mut self.ring[slot];
        rb.ready.store(false, Ordering::SeqCst);
        let ready = rb.ready.clone();
        rb.buf.map_async(MapMode::Read, .., move |r| {
            r.expect("readback");
            ready.store(true, Ordering::SeqCst);
        });
        rb.sub = Some(sub);
        evicted
    }

    /// Waits for ring slot `slot` and returns its NV12 bytes (Y plane, then Cb Cr).
    pub fn collect_nv12(&mut self, slot: usize, out: &mut Vec<u8>) {
        let rb = &mut self.ring[slot];
        if let Some(sub) = rb.sub.take() {
            self.device.poll(PollType::Wait { submission_index: Some(sub), timeout: None }).expect("poll");
        }
        while !rb.ready.load(Ordering::SeqCst) {
            self.device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("poll");
        }
        out.clear();
        {
            let view = rb.buf.get_mapped_range(..).expect("mapped");
            let (w, h) = (self.w as usize, self.h as usize);
            for r in 0..h {
                let s = r * self.ypitch as usize;
                out.extend_from_slice(&view[s..s + w]);
            }
            let base = self.ypitch as usize * h;
            for r in 0..h / 2 {
                let s = base + r * self.uvpitch as usize;
                out.extend_from_slice(&view[s..s + w / 2 * 2]);
            }
        }
        rb.buf.unmap();
    }

    /// Draws a frame and shows it in a window's surface texture, letterboxed, with a
    /// progress bar (`progress` 0…1) along the bottom.
    pub fn present(&mut self, samples: &[Vec<Op>], get: &mut dyn FnMut(&TexKey) -> Arc<Decoded>, view: &TextureView, fmt: TextureFormat, win: (u32, u32), progress: f32) {
        let (mut enc, accum, _) = self.record(samples, get);
        if !self.present.contains_key(&fmt) {
            let p = self.device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("present"),
                layout: Some(&self.pl_n),
                vertex: VertexState { module: &self.m_src, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                fragment: Some(FragmentState {
                    module: &self.m_src,
                    entry_point: Some("fs_present"),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState { format: fmt, blend: None, write_mask: ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.present.insert(fmt, p);
        }
        // window px → frame px: fit the frame, centred
        let k = (win.0 as f32 / self.w as f32).min(win.1.saturating_sub(8) as f32 / self.h as f32);
        let (ox, oy) = ((win.0 as f32 - self.w as f32 * k) / 2.0, (win.1.saturating_sub(8) as f32 - self.h as f32 * k) / 2.0);
        let mut u = self.u0();
        u[P0] = [1.0 / k, ox, oy, progress];
        u[P1] = [self.w as f32, self.h as f32, win.0 as f32, win.1 as f32];
        let off = self.push_u(&u);
        let src = if accum { self.accum.bg_n.clone() } else { self.layers[0].bg_n.clone() };
        {
            let mut pass = begin(&mut enc, view, true);
            pass.set_pipeline(&self.present[&fmt]);
            pass.set_bind_group(0, &self.ubg, &[off]);
            pass.set_bind_group(1, &src, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.write_buffer(&self.ubuf, 0, &self.udata);
        self.queue.submit([enc.finish()]);
    }

    /// A frame as RGB8 (stills).
    pub fn still_rgb(&mut self, samples: &[Vec<Op>], get: &mut dyn FnMut(&TexKey) -> Arc<Decoded>) -> Vec<u8> {
        let (mut enc, accum, _) = self.record(samples, get);
        let rp = self.p.rgb.clone();
        let rv = self.rgbt.1.view.clone();
        self.convert(&mut enc, accum, &rp, &rv, 0.0);
        let pitch = align(self.w * 4, 256);
        let buf = self.device.create_buffer(&BufferDescriptor { label: None, size: (pitch * self.h) as u64, usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST, mapped_at_creation: false });
        enc.copy_texture_to_buffer(
            TexelCopyTextureInfo { texture: &self.rgbt.0, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            TexelCopyBufferInfo { buffer: &buf, layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(pitch), rows_per_image: Some(self.h) } },
            Extent3d { width: self.w, height: self.h, depth_or_array_layers: 1 },
        );
        self.queue.write_buffer(&self.ubuf, 0, &self.udata);
        self.queue.submit([enc.finish()]);
        buf.map_async(MapMode::Read, .., |r| r.expect("readback"));
        self.device.poll(PollType::Wait { submission_index: None, timeout: None }).expect("poll");
        let view = buf.get_mapped_range(..).expect("mapped");
        let mut out = Vec::with_capacity((self.w * self.h * 3) as usize);
        for r in 0..self.h as usize {
            let row = &view[r * pitch as usize..r * pitch as usize + self.w as usize * 4];
            for px in row.chunks(4) {
                out.extend_from_slice(&px[..3]);
            }
        }
        out
    }
}

#[allow(clippy::too_many_arguments)]
fn make_target(device: &Device, l1t: &BindGroupLayout, l1n: &BindGroupLayout, smp: &Sampler, w: u32, h: u32, fmt: TextureFormat, filterable: bool) -> (Texture, Target) {
    let tex = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: fmt,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let bg_t = filterable.then(|| {
        device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: l1t,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view) }, BindGroupEntry { binding: 1, resource: BindingResource::Sampler(smp) }],
        })
    });
    let bg_n = device.create_bind_group(&BindGroupDescriptor { label: None, layout: l1n, entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view) }] });
    (tex, Target { view, bg_t, bg_n })
}

fn begin(enc: &mut CommandEncoder, view: &TextureView, clear: bool) -> RenderPass<'static> {
    begin_with(enc, view, if clear { LoadOp::Clear(Color::TRANSPARENT) } else { LoadOp::Load })
}

fn begin_with(enc: &mut CommandEncoder, view: &TextureView, load: LoadOp<Color>) -> RenderPass<'static> {
    enc.begin_render_pass(&RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load, store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
    .forget_lifetime()
}

fn f4(v: [f64; 4]) -> [f32; 4] {
    v.map(|x| x as f32)
}

fn set_h(u: &mut U, h: &crate::canvas::M3) {
    for i in 0..3 {
        u[H0 + i] = [h[i][0] as f32, h[i][1] as f32, h[i][2] as f32, 0.0];
    }
}

fn set_clip(u: &mut U, clip: &Clip) {
    if let Some((r, rr, m)) = clip {
        u[CLIP] = f4(*r);
        u[CLIPR] = f4(*rr);
        for i in 0..3 {
            u[CM + i] = [m[i][0] as f32, m[i][1] as f32, m[i][2] as f32, 0.0];
        }
    }
}
