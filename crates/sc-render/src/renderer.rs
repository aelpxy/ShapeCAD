//! The wgpu pipeline that sphere-traces a field into a texture.
//!
//! Rendering targets a plain [`wgpu::TextureView`], not a surface, so the same
//! code path serves the on-screen viewport and offscreen capture. The agent
//! layer will need the latter to see what it has built.

use crate::camera::OrbitCamera;
use crate::shader;
use bytemuck::{Pod, Zeroable};
use sc_geom::wgsl::Generated;

/// Camera parameters in the layout the shader consumes.
///
/// Scalars are packed into the `w` lanes of the vectors rather than declared as
/// separate fields, which sidesteps uniform-buffer alignment padding entirely.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    /// xyz = eye position, w = `tan(fov_y / 2)`
    eye: [f32; 4],
    /// xyz = right vector, w = aspect ratio
    right: [f32; 4],
    /// xyz = up vector, w = far distance
    up: [f32; 4],
    /// xyz = forward vector, w = angular size of one pixel
    forward: [f32; 4],
    /// Scene colours, xyz each. Padded to `vec4` because a uniform buffer aligns
    /// three-component vectors to sixteen bytes anyway.
    sky: [f32; 4],
    haze: [f32; 4],
    plate: [f32; 4],
    grid: [f32; 4],
}

/// The viewport's share of the interface palette.
///
/// Held by the renderer rather than passed per draw, because it changes when the
/// user switches appearance and not otherwise. Defaults to the light scheme, so
/// a caller that never sets one gets what the viewport has always looked like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScenePalette {
    /// Top of the background sweep.
    pub sky: [f32; 3],
    /// The background at the horizon.
    pub haze: [f32; 3],
    /// The build plate.
    pub plate: [f32; 3],
    /// Grid lines on the plate.
    pub grid: [f32; 3],
}

impl Default for ScenePalette {
    fn default() -> Self {
        Self {
            sky: [0.700, 0.735, 0.790],
            haze: [0.930, 0.943, 0.962],
            plate: [0.895, 0.910, 0.930],
            grid: [0.66, 0.69, 0.73],
        }
    }
}

fn rgb4(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 0.0]
}

impl CameraUniform {
    fn new(cam: &OrbitCamera, width: u32, height: u32, scene: ScenePalette) -> Self {
        let (right, up, forward) = cam.basis();
        let eye = cam.eye();
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let tan_half = (cam.fov_y * 0.5).tan();
        Self {
            eye: [eye.x, eye.y, eye.z, tan_half],
            right: [right.x, right.y, right.z, aspect],
            up: [up.x, up.y, up.z, cam.far()],
            forward: [forward.x, forward.y, forward.z, cam.pixel_angle(height)],
            sky: rgb4(scene.sky),
            haze: rgb4(scene.haze),
            plate: rgb4(scene.plate),
            grid: rgb4(scene.grid),
        }
    }
}

/// Draws an implicit field.
#[derive(Debug)]
pub struct Renderer {
    format: wgpu::TextureFormat,
    uniform: wgpu::Buffer,
    /// Model values, read by the generated shader.
    params: wgpu::Buffer,
    /// Floats the parameter buffer can hold before it must be reallocated.
    params_capacity: usize,
    /// Scene colours, following the interface palette.
    scene: ScenePalette,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
    /// The source the current pipeline was built from.
    ///
    /// Model values live in the buffer, so identical source means only values
    /// moved and the pipeline can stand.
    source: String,
}

impl Renderer {
    /// Builds a renderer for a given surface format and generated field.
    ///
    /// Uploads the field's values as part of construction. Leaving that to the
    /// caller meant a renderer that compiled the right shader and then drew
    /// nothing, because every parameter was still zero.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        field: &Generated,
    ) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sc-render camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params_capacity = field.params.len().max(1);
        let params = new_param_buffer(device, params_capacity);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc-render bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bind_group = make_bind_group(device, &bind_group_layout, &uniform, &params);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc-render pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        if !field.params.is_empty() {
            queue.write_buffer(&params, 0, bytemuck::cast_slice(&field.params));
        }

        let pipeline = build_pipeline(device, &pipeline_layout, format, &field.source);

        Self {
            format,
            uniform,
            params,
            params_capacity,
            bind_group_layout,
            bind_group,
            pipeline_layout,
            pipeline,
            source: field.source.clone(),
            scene: ScenePalette::default(),
        }
    }

    /// Sets the viewport's colours. Takes effect on the next draw.
    ///
    /// Only the uniform changes, so switching appearance does not rebuild the
    /// pipeline: the scene palette is data the shader reads, not part of its
    /// source.
    pub fn set_scene(&mut self, scene: ScenePalette) {
        self.scene = scene;
    }

    /// Applies a newly generated field.
    ///
    /// Recompiles only when the source actually changed. Because no model value
    /// appears in the source, editing a radius or dragging a sketch point is a
    /// buffer write; adding a node, or a value crossing a threshold that changes
    /// which peepholes apply, is a rebuild.
    ///
    /// Returns true if the pipeline was rebuilt, which callers report as the
    /// cost of the edit.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        field: &Generated,
    ) -> bool {
        if field.params.len() > self.params_capacity {
            // Grow generously; profiles tend to gain a point at a time.
            self.params_capacity = field.params.len().next_power_of_two();
            self.params = new_param_buffer(device, self.params_capacity);
            self.bind_group =
                make_bind_group(device, &self.bind_group_layout, &self.uniform, &self.params);
        }
        if !field.params.is_empty() {
            queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&field.params));
        }

        if self.source == field.source {
            return false;
        }
        self.pipeline = build_pipeline(device, &self.pipeline_layout, self.format, &field.source);
        self.source.clone_from(&field.source);
        true
    }

    /// Records a draw covering the whole target.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) {
        self.draw_in(
            queue,
            encoder,
            view,
            camera,
            [0.0, 0.0, width as f32, height as f32],
        );
    }

    /// Records a draw confined to `rect`, given in physical pixels as
    /// `[x, y, width, height]`.
    ///
    /// Used by the application to fill only the region left over by the chrome.
    /// The aspect ratio comes from the rectangle, not the window, so the model
    /// is not stretched by the side panels.
    pub fn draw_in(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        camera: &OrbitCamera,
        rect: [f32; 4],
    ) {
        let [x, y, w, h] = rect;
        let (w, h) = (w.max(1.0), h.max(1.0));
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&CameraUniform::new(camera, w as u32, h as u32, self.scene)),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sc-render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    // Matches the application's viewport background so the seam
                    // between chrome and 3D is invisible.
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.880,
                        g: 0.897,
                        b: 0.920,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// A storage buffer able to hold `capacity` floats.
fn new_param_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sc-render parameters"),
        size: (capacity.max(1) * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sc-render bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    field_wgsl: &str,
) -> wgpu::RenderPipeline {
    let source = shader::compose(field_wgsl);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sc-render shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sc-render pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(format.into())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
