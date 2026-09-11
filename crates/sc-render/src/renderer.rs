//! The wgpu pipeline that sphere-traces a field into a texture.
//!
//! Rendering targets a plain [`wgpu::TextureView`], not a surface, so the same
//! code path serves the on-screen viewport and offscreen capture. The agent
//! layer will need the latter to see what it has built.

use crate::camera::OrbitCamera;
use crate::shader;
use bytemuck::{Pod, Zeroable};

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
}

impl CameraUniform {
    fn new(cam: &OrbitCamera, width: u32, height: u32) -> Self {
        let (right, up, forward) = cam.basis();
        let eye = cam.eye();
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let tan_half = (cam.fov_y * 0.5).tan();
        Self {
            eye: [eye.x, eye.y, eye.z, tan_half],
            right: [right.x, right.y, right.z, aspect],
            up: [up.x, up.y, up.z, cam.far()],
            forward: [forward.x, forward.y, forward.z, cam.pixel_angle(height)],
        }
    }
}

/// Draws an implicit field.
#[derive(Debug)]
pub struct Renderer {
    format: wgpu::TextureFormat,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
}

impl Renderer {
    /// Builds a renderer for a given surface format and field.
    ///
    /// `field_wgsl` is the output of `sc_geom::wgsl::generate`.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, field_wgsl: &str) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sc-render camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sc-render bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sc-render bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sc-render pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = build_pipeline(device, &pipeline_layout, format, field_wgsl);

        Self {
            format,
            uniform,
            bind_group,
            pipeline_layout,
            pipeline,
        }
    }

    /// Recompiles the pipeline for a new field.
    ///
    /// Called whenever the document's topology changes. Numeric edits will
    /// eventually go through a uniform buffer instead, since recompiling a
    /// shader per frame is far too slow for dragging a value.
    pub fn set_field(&mut self, device: &wgpu::Device, field_wgsl: &str) {
        self.pipeline = build_pipeline(device, &self.pipeline_layout, self.format, field_wgsl);
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
            bytemuck::bytes_of(&CameraUniform::new(camera, w as u32, h as u32)),
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
