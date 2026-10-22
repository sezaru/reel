//! Models into textures (model.rs frames them; mesh.wgsl shades them): 4× multisampled with a
//! depth buffer, at twice the size they are shown (the box shader's mips bring them down), so
//! the edges of glossy metal stay clean. Opaque surfaces first, then the screen, then the
//! see-through ones far to near.
use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;
use wgpu::*;

use super::{Decoded, Gpu, TexEntry};
use crate::canvas::TexKey;
use crate::model::{self, Alpha, ModelReq};

const SAMPLES: u32 = 4;
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
const DEPTH: TextureFormat = TextureFormat::Depth32Float;
const ENV: TextureFormat = TextureFormat::Rgba16Float;
const ENV_W: u32 = 256;
const ENV_H: u32 = 128;
const ROOM_W: usize = 512;
const ROOM_H: usize = 256;
const U_STRIDE: u64 = 256;

#[derive(Default)]
pub(super) struct Meshes {
    state: Option<State>,
    /// by path; the model they were made from (a reloaded file is made again)
    models: HashMap<Arc<str>, (Arc<model::Model>, ModelGpu)>,
}

struct State {
    l_u: BindGroupLayout,
    l_mat: BindGroupLayout,
    env_bg: BindGroup,
    /// opaque, opaque double-sided, blend, blend double-sided, the screen
    pipes: [RenderPipeline; 5],
    smp: Sampler,
    smp_clamp: Sampler,
    white: TextureView,
}

struct PrimGpu {
    vb: Buffer,
    ib: Buffer,
    n: u32,
}

struct ModelGpu {
    prims: Vec<PrimGpu>,
    /// each material's textures (white where it has none)
    bgs: Vec<BindGroup>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Mu {
    vp: [f32; 16],
    eye: [f32; 4],
    base: [f32; 4],
    mr: [f32; 4],
    emissive: [f32; 4],
    has: [f32; 4],
    mode: [f32; 4],
    uv0: [f32; 4],
    uv1: [f32; 4],
    _pad: [f32; 12],
}

/// f32 → IEEE half (round to nearest; out of range clamps to the largest finite value).
fn half(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let a = v.abs();
    if a.is_nan() {
        return sign | 0x7e00;
    }
    if a >= 65504.0 {
        return sign | 0x7bff;
    }
    if a < 6.1035156e-5 {
        // subnormal
        return sign | (a / 5.9604645e-8).round() as u16;
    }
    let e = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let m = b & 0x7f_ffff;
    let mut h = ((e as u32) << 10) | (m >> 13);
    // round to nearest even
    let rest = m & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    sign | h as u16
}

impl Gpu {
    fn mesh_state(&mut self, enc: &mut CommandEncoder) {
        if self.meshes.state.is_some() {
            return;
        }
        let d = &self.device;
        let l_u = d.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("model uniforms"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: std::num::NonZeroU64::new(std::mem::size_of::<Mu>() as u64) },
                count: None,
            }],
        });
        let tex = |binding, view_dimension| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture { sample_type: TextureSampleType::Float { filterable: true }, view_dimension, multisampled: false },
            count: None,
        };
        let smp_e = |binding| BindGroupLayoutEntry { binding, visibility: ShaderStages::FRAGMENT, ty: BindingType::Sampler(SamplerBindingType::Filtering), count: None };
        let l_mat = d.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[tex(0, TextureViewDimension::D2), tex(1, TextureViewDimension::D2), tex(2, TextureViewDimension::D2), tex(3, TextureViewDimension::D2), smp_e(4), smp_e(5)],
        });
        let l_env = d.create_bind_group_layout(&BindGroupLayoutDescriptor { label: Some("environment"), entries: &[tex(0, TextureViewDimension::D2Array), smp_e(1)] });

        // the room, then each of three's PMREM levels from it
        let room = model::room(ROOM_W, ROOM_H);
        let levels = 32 - (ROOM_W as u32).leading_zeros();
        let base = d.create_texture(&TextureDescriptor {
            label: Some("room"),
            size: Extent3d { width: ROOM_W as u32, height: ROOM_H as u32, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: ENV,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut w, mut h, mut px) = (ROOM_W, ROOM_H, room);
        for l in 0..levels {
            let data: Vec<u16> = px.iter().flat_map(|c| [half(c[0]), half(c[1]), half(c[2]), half(1.0)]).collect();
            self.queue.write_texture(
                TexelCopyTextureInfo { texture: &base, mip_level: l, origin: Origin3d::ZERO, aspect: TextureAspect::All },
                bytemuck::cast_slice(&data),
                TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8 * w as u32), rows_per_image: Some(h as u32) },
                Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
            );
            // the next level: 2×2 averages
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = vec![[0.0f32; 3]; nw * nh];
            for y in 0..nh {
                for x in 0..nw {
                    let mut s = [0.0; 3];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let c = px[(2 * y + dy).min(h - 1) * w + (2 * x + dx).min(w - 1)];
                        for k in 0..3 {
                            s[k] += c[k] / 4.0;
                        }
                    }
                    next[y * nw + x] = s;
                }
            }
            (w, h, px) = (nw, nh, next);
        }
        let env = d.create_texture(&TextureDescriptor {
            label: Some("environment"),
            size: Extent3d { width: ENV_W, height: ENV_H, depth_or_array_layers: model::ENV_SIGMAS.len() as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: ENV,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let wrap = d.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..Default::default()
        });
        let m_env = d.create_shader_module(ShaderModuleDescriptor { label: Some("env"), source: ShaderSource::Wgsl(include_str!("../env.wgsl").into()) });
        let l_eu = d.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: &[BindGroupLayoutEntry { binding: 0, visibility: ShaderStages::FRAGMENT, ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }],
        });
        let l_src = d.create_bind_group_layout(&BindGroupLayoutDescriptor { label: None, entries: &[tex(0, TextureViewDimension::D2), smp_e(1)] });
        let pl = d.create_pipeline_layout(&PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&l_eu), Some(&l_src)], immediate_size: 0 });
        let p_env = d.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("env"),
            layout: Some(&pl),
            vertex: VertexState { module: &m_env, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState { module: &m_env, entry_point: Some("fs_env"), compilation_options: Default::default(), targets: &[Some(ColorTargetState { format: ENV, blend: None, write_mask: ColorWrites::ALL })] }),
            multiview_mask: None,
            cache: None,
        });
        let base_view = base.create_view(&Default::default());
        let src_bg = d.create_bind_group(&BindGroupDescriptor { label: None, layout: &l_src, entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&base_view) }, BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&wrap) }] });
        for (i, s) in model::ENV_SIGMAS.iter().enumerate() {
            let sigma = (model::ENV_BASE_SIGMA * model::ENV_BASE_SIGMA + s * s).sqrt();
            // read the mip whose texels are about as far apart as the samples
            let step = (3.0 * sigma).min(std::f64::consts::PI) / 32.0;
            let lod = (step / (std::f64::consts::PI / ROOM_H as f64)).log2().max(0.0);
            let eu: [f32; 4] = [sigma as f32, lod as f32, ENV_W as f32, ENV_H as f32];
            let ub = d.create_buffer_init(&util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&eu), usage: BufferUsages::UNIFORM });
            let ubg = d.create_bind_group(&BindGroupDescriptor { label: None, layout: &l_eu, entries: &[BindGroupEntry { binding: 0, resource: ub.as_entire_binding() }] });
            let view = env.create_view(&TextureViewDescriptor { dimension: Some(TextureViewDimension::D2), base_array_layer: i as u32, array_layer_count: Some(1), ..Default::default() });
            let mut pass = super::begin(enc, &view, true);
            pass.set_pipeline(&p_env);
            pass.set_bind_group(0, &ubg, &[]);
            pass.set_bind_group(1, &src_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        let env_view = env.create_view(&TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..Default::default() });
        let env_smp = d.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let env_bg = d.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &l_env,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&env_view) }, BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&env_smp) }],
        });

        let m = d.create_shader_module(ShaderModuleDescriptor { label: Some("mesh"), source: ShaderSource::Wgsl(include_str!("../mesh.wgsl").into()) });
        let pl = d.create_pipeline_layout(&PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&l_u), Some(&l_mat), Some(&l_env)], immediate_size: 0 });
        let attrs = vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let pipe = |cull: Option<Face>, blend: bool, bias: i32| {
            d.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("mesh"),
                layout: Some(&pl),
                vertex: VertexState {
                    module: &m,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(VertexBufferLayout { array_stride: std::mem::size_of::<model::Vert>() as u64, step_mode: VertexStepMode::Vertex, attributes: &attrs })],
                },
                primitive: PrimitiveState { front_face: FrontFace::Ccw, cull_mode: cull, ..Default::default() },
                depth_stencil: Some(DepthStencilState {
                    format: DEPTH,
                    // (three: see-through glTF materials don't write depth)
                    depth_write_enabled: Some(!blend),
                    depth_compare: Some(CompareFunction::LessEqual),
                    stencil: Default::default(),
                    // the screen pulled forward of the glass and bezel it sits in (three's
                    // polygonOffset)
                    bias: DepthBiasState { constant: bias, slope_scale: bias as f32, clamp: 0.0 },
                }),
                multisample: MultisampleState { count: SAMPLES, ..Default::default() },
                fragment: Some(FragmentState {
                    module: &m,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState {
                        format: COLOR,
                        // three's NormalBlending: colour by its alpha, alpha over
                        blend: blend.then_some(BlendState {
                            color: BlendComponent { src_factor: BlendFactor::SrcAlpha, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
                            alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add },
                        }),
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipes = [pipe(Some(Face::Back), false, 0), pipe(None, false, 0), pipe(Some(Face::Back), true, 0), pipe(None, true, 0), pipe(None, false, -4)];
        let mk_smp = |mode| {
            d.create_sampler(&SamplerDescriptor {
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                mipmap_filter: MipmapFilterMode::Linear,
                anisotropy_clamp: 16,
                ..Default::default()
            })
        };
        let (smp, smp_clamp) = (mk_smp(AddressMode::Repeat), mk_smp(AddressMode::ClampToEdge));
        let white = self.mip_texture(enc, &Decoded { w: 1, h: 1, data: vec![255; 4] }).create_view(&Default::default());
        self.meshes.state = Some(State { l_u, l_mat, env_bg, pipes, smp, smp_clamp, white });
    }

    fn model_gpu(&mut self, enc: &mut CommandEncoder, m: &Arc<model::Model>) {
        if let Some((have, _)) = self.meshes.models.get(&m.path)
            && Arc::ptr_eq(have, m)
        {
            return;
        }
        let views: Vec<TextureView> = m.images.iter().map(|d| self.mip_texture(enc, d).create_view(&Default::default())).collect();
        let st = self.meshes.state.as_ref().unwrap();
        let d = &self.device;
        let prims = m
            .prims
            .iter()
            .map(|p| PrimGpu {
                vb: d.create_buffer_init(&util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&p.verts), usage: BufferUsages::VERTEX }),
                ib: d.create_buffer_init(&util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&p.idx), usage: BufferUsages::INDEX }),
                n: p.idx.len() as u32,
            })
            .collect();
        let pick = |i: Option<usize>| i.and_then(|i| views.get(i)).unwrap_or(&st.white).clone();
        let mviews: Vec<[TextureView; 4]> = m.mats.iter().map(|mt| [pick(mt.base_tex), pick(mt.mr_tex), pick(mt.normal_tex), pick(mt.emissive_tex)]).collect();
        let bgs = mviews.iter().map(|v| material_bg(d, st, v)).collect();
        self.meshes.models.insert(m.path.clone(), (m.clone(), ModelGpu { prims, bgs }));
    }

    /// Renders the model `key` asks for into a texture of its own (mipmapped, like an image).
    pub(super) fn render_model(&mut self, enc: &mut CommandEncoder, key: &TexKey, req: &ModelReq) {
        self.mesh_state(enc);
        let Some(m) = model::loaded(&req.path) else {
            eprintln!("model {}: not loaded", req.path);
            return;
        };
        self.model_gpu(enc, &m);
        let (w, h) = req.size;
        let levels = 32 - w.max(h).leading_zeros();
        let out = self.device.create_texture(&TextureDescriptor {
            label: Some("model"),
            size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: COLOR,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let ms = |format| {
            self.device
                .create_texture(&TextureDescriptor {
                    label: None,
                    size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: SAMPLES,
                    dimension: TextureDimension::D2,
                    format,
                    usage: TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let (color, depth) = (ms(COLOR), ms(DEPTH));
        let level0 = out.create_view(&TextureViewDescriptor { base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });

        // what to draw, in order: (prim, pipeline, uniforms)
        let st = self.meshes.state.as_ref().unwrap();
        let (_, mg) = &self.meshes.models[&m.path];
        let vp = req.view_proj();
        // (rows → WGSL's columns)
        let mut cols = [0.0f32; 16];
        for r in 0..4 {
            for c in 0..4 {
                cols[c * 4 + r] = vp[r * 4 + c];
            }
        }
        let eye = req.eye();
        let screen = m.screen.as_ref().map(|s| s.prim).filter(|_| req.screen.is_some());
        let screen_bg = req.screen.as_ref().and_then(|k| self.textures.get(k)).and_then(|e| e.view.clone()).map(|v| {
            let views = [v, st.white.clone(), st.white.clone(), st.white.clone()];
            material_bg(&self.device, st, &views)
        });
        let (uv0, uv1, wrap) = model::screen_uv(&m, req.landscape);
        let mut draws: Vec<(usize, usize, Mu)> = Vec::new();
        let mut blended: Vec<(f64, usize, usize, Mu)> = Vec::new();
        for (i, p) in m.prims.iter().enumerate() {
            let mt = &m.mats[p.mat];
            let mut u = Mu {
                vp: cols,
                eye: [eye[0], eye[1], eye[2], f32::from_bits(req.exposure)],
                base: mt.base,
                mr: [mt.metal, mt.rough, mt.normal_scale, if let Alpha::Mask(c) = mt.alpha { c } else { 0.5 }],
                emissive: [mt.emissive[0], mt.emissive[1], mt.emissive[2], 0.0],
                has: [mt.base_tex, mt.mr_tex, mt.normal_tex, mt.emissive_tex].map(|t| if t.is_some() { 1.0 } else { 0.0 }),
                mode: [0.0, match mt.alpha { Alpha::Opaque => 0.0, Alpha::Mask(_) => 1.0, Alpha::Blend => 2.0 }, if p.flat { 1.0 } else { 0.0 }, if mt.double { 1.0 } else { 0.0 }],
                uv0: [uv0[0], uv0[1], uv0[2], if wrap { 1.0 } else { 0.0 }],
                uv1: [uv1[0], uv1[1], uv1[2], 0.0],
                _pad: [0.0; 12],
            };
            if Some(i) == screen && screen_bg.is_some() {
                u.mode = [1.0, 0.0, 0.0, 1.0];
                draws.push((i, 4, u));
            } else if mt.alpha == Alpha::Blend {
                let c = p.center;
                let dist = ((c[0] - eye[0] as f64).powi(2) + (c[1] - eye[1] as f64).powi(2) + (c[2] - eye[2] as f64).powi(2)).sqrt();
                blended.push((dist, i, if mt.double { 3 } else { 2 }, u));
            } else {
                draws.push((i, if mt.double { 1 } else { 0 }, u));
            }
        }
        // opaque, the screen last of them; then the see-through, far to near
        draws.sort_by_key(|d| d.1 == 4);
        blended.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        draws.extend(blended.into_iter().map(|(_, i, p, u)| (i, p, u)));
        let mut data = vec![0u8; draws.len() * U_STRIDE as usize];
        for (k, d) in draws.iter().enumerate() {
            let b = bytemuck::bytes_of(&d.2);
            data[k * U_STRIDE as usize..k * U_STRIDE as usize + b.len()].copy_from_slice(b);
        }
        let ub = self.device.create_buffer_init(&util::BufferInitDescriptor { label: Some("model uniforms"), contents: &data, usage: BufferUsages::UNIFORM });
        let ubg = self.device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &st.l_u,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::Buffer(BufferBinding { buffer: &ub, offset: 0, size: std::num::NonZeroU64::new(std::mem::size_of::<Mu>() as u64) }) }],
        });
        {
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("model"),
                color_attachments: &[Some(RenderPassColorAttachment { view: &color, depth_slice: None, resolve_target: Some(&level0), ops: Operations { load: LoadOp::Clear(Color::TRANSPARENT), store: StoreOp::Discard } })],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment { view: &depth, depth_ops: Some(Operations { load: LoadOp::Clear(1.0), store: StoreOp::Discard }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(2, &st.env_bg, &[]);
            for (k, (i, p, _)) in draws.iter().enumerate() {
                let pg = &mg.prims[*i];
                pass.set_pipeline(&st.pipes[*p]);
                pass.set_bind_group(0, &ubg, &[(k as u64 * U_STRIDE) as u32]);
                let bg = if *p == 4 { screen_bg.as_ref().unwrap() } else { &mg.bgs[m.prims[*i].mat] };
                pass.set_bind_group(1, bg, &[]);
                pass.set_vertex_buffer(0, pg.vb.slice(..));
                pass.set_index_buffer(pg.ib.slice(..), IndexFormat::Uint32);
                pass.draw_indexed(0..pg.n, 0, 0..1);
            }
        }
        self.mips(enc, &out, levels);
        let view = out.create_view(&Default::default());
        let bg = self.device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &self.l1t,
            entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view) }, BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&self.smp_tex) }],
        });
        let bytes = w as u64 * h as u64 * 4 * 4 / 3;
        self.tex_bytes += bytes;
        self.textures.insert(key.clone(), TexEntry { bg, view: Some(view), bytes, used: self.stamp });
    }
}

fn material_bg(d: &Device, st: &State, v: &[TextureView; 4]) -> BindGroup {
    d.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &st.l_mat,
        entries: &[
            BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&v[0]) },
            BindGroupEntry { binding: 1, resource: BindingResource::TextureView(&v[1]) },
            BindGroupEntry { binding: 2, resource: BindingResource::TextureView(&v[2]) },
            BindGroupEntry { binding: 3, resource: BindingResource::TextureView(&v[3]) },
            BindGroupEntry { binding: 4, resource: BindingResource::Sampler(&st.smp) },
            BindGroupEntry { binding: 5, resource: BindingResource::Sampler(&st.smp_clamp) },
        ],
    })
}
