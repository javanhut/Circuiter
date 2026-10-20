//! Drawing meshes with the GPU inside the window: an orbiting camera, and an egui paint
//! callback that renders into an offscreen colour and depth target (multisampled, so
//! edges are smooth) and then copies the picture into egui's own pass.

use circuiter_3d::Mesh;
use circuiter_3d::math::{Mat4, V3, add, cross, normalize, scale, sub};
use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;

/// An orbiting camera: looks at `target` from `distance` away, turned `yaw` about the
/// vertical and raised `pitch` above the horizon (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub target: V3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

/// The vertical field of view.
const FOV: f32 = 0.6;

impl Default for Camera {
    fn default() -> Self {
        Camera { target: [0.0; 3], distance: 60.0, yaw: -2.2, pitch: 0.6 }
    }
}

impl Camera {
    pub fn eye(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        add(self.target, scale([cp * cy, cp * sy, sp], self.distance))
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).max(0.01);
        let far = self.distance * 50.0 + 100.0;
        let view = Mat4::look_at(self.eye(), self.target, [0.0, 0.0, 1.0]);
        Mat4::perspective(FOV, aspect.max(0.01), near, far).mul(&view)
    }

    /// Frames everything between `lo` and `hi`, keeping the direction it looks from.
    /// Frames the box `lo`–`hi` in a view `aspect` wide per unit of height: as close as
    /// the camera can get with every corner still inside, with a little margin.
    pub fn fit(&mut self, (lo, hi): (V3, V3), aspect: f32) {
        self.target = scale(add(lo, hi), 0.5);
        let radius = (circuiter_3d::math::length(sub(hi, lo)) / 2.0).max(1.0);
        let corners: Vec<V3> = (0..8)
            .map(|k| {
                [
                    if k & 1 == 0 { lo[0] } else { hi[0] },
                    if k & 2 == 0 { lo[1] } else { hi[1] },
                    if k & 4 == 0 { lo[2] } else { hi[2] },
                ]
            })
            .collect();
        // Start well inside the bounding sphere's distance and back off until it fits.
        self.distance = radius / (FOV / 2.0).sin() * 0.3;
        for _ in 0..80 {
            let vp = self.view_proj(aspect);
            let inside = corners.iter().all(|&c| {
                let p = vp.point(c);
                p[0].abs() <= 0.9 && p[1].abs() <= 0.9 && (0.0..=1.0).contains(&p[2])
            });
            if inside {
                break;
            }
            self.distance *= 1.05;
        }
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-1.55, 1.55);
    }

    /// Slides the view by a drag of `dx`, `dy` points in a view `height` points tall.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        let eye = self.eye();
        let forward = normalize(sub(self.target, eye));
        let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
        let up = cross(right, forward);
        // World units per point at the target's distance.
        let k = 2.0 * self.distance * (FOV / 2.0).tan() / height.max(1.0);
        self.target = add(self.target, add(scale(right, -dx * k), scale(up, dy * k)));
    }

    /// The ray from the eye through the view at `ndc` (−1 to 1 across and up, as the
    /// projection maps it), in a view `aspect` wide per unit of height: where it starts
    /// and its direction.
    pub fn ray(&self, ndc: [f32; 2], aspect: f32) -> (V3, V3) {
        let eye = self.eye();
        let forward = normalize(sub(self.target, eye));
        let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
        let up = cross(right, forward);
        let k = (FOV / 2.0).tan();
        let dir = add(forward, add(scale(right, ndc[0] * k * aspect), scale(up, ndc[1] * k)));
        (eye, normalize(dir))
    }

    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance / factor).clamp(0.5, 50_000.0);
    }
}

/// What to draw in a frame.
pub struct Frame3d {
    /// The mesh, with an id that changes whenever it does (so it's uploaded once).
    pub mesh: Arc<Mesh>,
    pub mesh_id: u64,
    pub view_proj: Mat4,
    pub background: [f64; 4],
    /// The picture's size in physical pixels.
    pub size: [u32; 2],
}

const SAMPLES: u32 = 4;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// GPU objects kept between frames, in egui's callback resources.
pub struct Renderer3d {
    format: wgpu::TextureFormat,
    mesh_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    mesh: Option<(u64, wgpu::Buffer, wgpu::Buffer, u32)>,
    target: Option<Target>,
}

struct Target {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    blit_group: wgpu::BindGroup,
}

const SHADER: &str = r#"
struct Uniforms { view_proj: mat4x4<f32>, light: vec4<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_mesh(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = u.view_proj * vec4<f32>(p, 1.0);
    o.normal = n;
    o.color = c;
    return o;
}

@fragment
fn fs_mesh(i: VOut) -> @location(0) vec4<f32> {
    let n = normalize(i.normal);
    let key = abs(dot(n, normalize(u.light.xyz)));
    let fill = max(dot(n, normalize(vec3<f32>(-0.5, -0.4, 0.4))), 0.0);
    let sky = 0.5 + 0.5 * n.z;
    let shade = 0.22 + 0.62 * key + 0.16 * fill + 0.12 * sky;
    return vec4<f32>(i.color.rgb * shade, 1.0);
}

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var picture_sampler: sampler;

struct BOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs_blit(@builtin(vertex_index) k: u32) -> BOut {
    // One triangle covering the viewport.
    let xy = vec2<f32>(f32((k << 1u) & 2u), f32(k & 2u));
    var o: BOut;
    o.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return o;
}

@fragment
fn fs_blit(i: BOut) -> @location(0) vec4<f32> {
    return textureSample(picture, picture_sampler, i.uv);
}
"#;

impl Renderer3d {
    /// Sets the renderer up in egui's resources; call once, with the window's render
    /// state.
    pub fn install(state: &egui_wgpu::RenderState) {
        let r = Renderer3d::new(&state.device, state.target_format);
        state.renderer.write().callback_resources.insert(r);
    }

    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer3d {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("circuiter-3d"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d-uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3d-uniforms"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d-uniforms"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d-mesh"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3d-mesh"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_mesh"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 28,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4],
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_mesh"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d-blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d-blit"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3d-blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_blit"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_blit"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3d-blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        Renderer3d {
            format,
            mesh_pipeline,
            blit_pipeline,
            uniforms,
            uniform_group,
            blit_layout,
            sampler,
            mesh: None,
            target: None,
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: u64, mesh: &Mesh) {
        if self.mesh.as_ref().is_some_and(|m| m.0 == id) {
            return;
        }
        let mut vertices = Vec::with_capacity(mesh.vertices.len() * 28);
        for v in &mesh.vertices {
            for x in v.pos.iter().chain(&v.normal) {
                vertices.extend_from_slice(&x.to_le_bytes());
            }
            vertices.extend_from_slice(&v.color);
        }
        let mut indices = Vec::with_capacity(mesh.indices.len() * 4);
        for i in &mesh.indices {
            indices.extend_from_slice(&i.to_le_bytes());
        }
        // Buffers can't be empty.
        vertices.resize(vertices.len().max(28), 0);
        indices.resize(indices.len().max(4), 0);
        let buffer = |data: &[u8], usage: wgpu::BufferUsages| {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("3d-mesh"),
                size: (data.len() as u64).next_multiple_of(4),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut padded = data.to_vec();
            padded.resize(padded.len().next_multiple_of(4), 0);
            queue.write_buffer(&b, 0, &padded);
            b
        };
        let vb = buffer(&vertices, wgpu::BufferUsages::VERTEX);
        let ib = buffer(&indices, wgpu::BufferUsages::INDEX);
        self.mesh = Some((id, vb, ib, mesh.indices.len() as u32));
    }

    fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let texture = |samples: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("3d-target"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let msaa = texture(SAMPLES, self.format, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = texture(SAMPLES, DEPTH, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = texture(
            1,
            self.format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d-blit"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resolve),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.target = Some(Target { size, msaa, depth, resolve, blit_group });
    }
}

/// The paint callback for one 3D view.
pub struct Paint3d(pub Frame3d);

impl egui_wgpu::CallbackTrait for Paint3d {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<Renderer3d>() else { return Vec::new() };
        let f = &self.0;
        let size = [f.size[0].max(1), f.size[1].max(1)];
        r.upload(device, queue, f.mesh_id, &f.mesh);
        r.resize(device, size);
        let mut u = Vec::with_capacity(80);
        for col in f.view_proj.0 {
            for x in col {
                u.extend_from_slice(&x.to_le_bytes());
            }
        }
        for x in [0.35f32, -0.55, 0.75, 0.0] {
            u.extend_from_slice(&x.to_le_bytes());
        }
        queue.write_buffer(&r.uniforms, 0, &u);
        let (Some(t), Some((_, vb, ib, count))) = (&r.target, &r.mesh) else { return Vec::new() };
        let [cr, cg, cb, ca] = f.background;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.msaa,
                resolve_target: Some(&t.resolve),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: cr, g: cg, b: cb, a: ca }),
                    store: wgpu::StoreOp::Discard,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if *count > 0 {
            pass.set_pipeline(&r.mesh_pipeline);
            pass.set_bind_group(0, &r.uniform_group, &[]);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..*count, 0, 0..1);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: eframe::egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(r) = resources.get::<Renderer3d>() else { return };
        let Some(t) = &r.target else { return };
        pass.set_pipeline(&r.blit_pipeline);
        pass.set_bind_group(0, &t.blit_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_through_a_point_on_screen_passes_through_it() {
        let c = Camera { target: [3.0, -2.0, 1.0], distance: 40.0, yaw: -1.1, pitch: 0.5 };
        let vp = c.view_proj(1.5);
        let p = [5.0, 4.0, -2.0];
        let ndc = vp.point(p);
        let (from, dir) = c.ray([ndc[0], ndc[1]], 1.5);
        let to = sub(p, from);
        let off = circuiter_3d::math::length(cross(to, dir));
        assert!(off < 1e-3, "{off}");
    }

    #[test]
    fn fitting_frames_the_whole_box_and_orbiting_keeps_the_target() {
        let mut c = Camera::default();
        c.fit(([-10.0, -10.0, 0.0], [10.0, 10.0, 5.0]), 1.0);
        assert_eq!(c.target, [0.0, 0.0, 2.5]);
        // Every corner lands inside the picture.
        let vp = c.view_proj(1.0);
        for x in [-10.0, 10.0] {
            for y in [-10.0, 10.0] {
                for z in [0.0, 5.0] {
                    let p = vp.point([x, y, z]);
                    assert!(p[0].abs() <= 0.9 && p[1].abs() <= 0.9, "{x},{y},{z} → {p:?}");
                }
            }
        }
        let before = c.target;
        c.orbit(100.0, 50.0);
        assert_eq!(c.target, before);
        c.pan(10.0, 0.0, 500.0);
        assert_ne!(c.target, before);
    }
}
