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

/// A palette colour as a clear value.
///
/// Both are linear: the target is an sRGB format, and wgpu encodes a clear on
/// the way in exactly as the shader's output is encoded, so the same numbers
/// mean the same colour on both paths.
fn clear_colour(c: [f32; 3]) -> wgpu::Color {
    wgpu::Color {
        r: f64::from(c[0]),
        g: f64::from(c[1]),
        b: f64::from(c[2]),
        a: 1.0,
    }
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
                    // The clear covers the whole attachment while the draw is
                    // confined to `rect`, so this is what shows anywhere the
                    // chrome does not paint. It has to follow the palette: a
                    // fixed light grey here is a white flash around a dark
                    // interface. `haze` is the background the viewport itself
                    // carries at its lower edge, so the two meet without a seam.
                    load: wgpu::LoadOp::Clear(clear_colour(self.scene.haze)),
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

#[cfg(test)]
mod tests {
    use super::{Renderer, ScenePalette};
    use crate::camera::OrbitCamera;
    use crate::snapshot;
    use sc_geom::glam::Vec3;
    use sc_geom::wgsl::Generated;
    use sc_geom::{Arena, Node};

    /// The dark scheme's viewport colours, in linear, as the application sends
    /// them. Held here so the test says what it is measuring rather than
    /// reaching across into `sc-app`.
    const DARK: ScenePalette = ScenePalette {
        sky: [0.0033, 0.0033, 0.0044],
        haze: [0.0091, 0.0091, 0.0110],
        plate: [0.0137, 0.0137, 0.0168],
        grid: [0.0497, 0.0497, 0.0612],
    };

    /// Linear to sRGB, which is what the hardware does on the way into an
    /// `Rgba8UnormSrgb` target.
    fn encoded(linear: f32) -> u8 {
        let s = if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    }

    fn cube(half: f32) -> Generated {
        let mut arena = Arena::new();
        let id = arena
            .insert(Node::Box {
                half: Vec3::splat(half),
                round: 0.0,
            })
            .expect("a box is valid");
        sc_geom::wgsl::generate(&arena, Some(id))
    }

    /// A union of `n` spheres, which is `n` parameters and one node per sphere.
    fn spheres(n: usize) -> Generated {
        let mut arena = Arena::new();
        let mut id = arena
            .insert(Node::Sphere { radius: 1.0 })
            .expect("a sphere is valid");
        for i in 1..n {
            let next = arena
                .insert(Node::Sphere {
                    radius: 1.0 + i as f32,
                })
                .expect("a sphere is valid");
            id = arena
                .insert(Node::Union {
                    a: id,
                    b: next,
                    smooth: 0.0,
                })
                .expect("a union is valid");
        }
        sc_geom::wgsl::generate(&arena, Some(id))
    }

    #[test]
    fn the_clear_colour_follows_the_scene_palette() {
        let Some((device, queue)) = crate::gpu::test_device() else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let mut renderer = Renderer::new(&device, &queue, snapshot::FORMAT, &cube(10.0));
        renderer.set_scene(DARK);

        // The clear covers the whole attachment while the draw is confined to
        // the rectangle, so the right-hand half of this image is the clear and
        // nothing else. A fixed light grey there is a white surround on a dark
        // interface.
        let (w, h) = (64u32, 64u32);
        let image = snapshot::capture(&device, &queue, w, h, |encoder, view| {
            renderer.draw_in(
                &queue,
                encoder,
                view,
                &OrbitCamera::default(),
                [0.0, 0.0, 32.0, 64.0],
            );
        });
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]]
        };
        let outside = at(w - 2, 2);
        let expected = DARK.haze.map(encoded);
        for channel in 0..3 {
            assert!(
                outside[channel].abs_diff(expected[channel]) <= 2,
                "the clear came back {outside:?} where the palette says {expected:?}"
            );
        }
    }

    #[test]
    fn only_a_changed_shader_source_rebuilds_the_pipeline() {
        let Some((device, queue)) = crate::gpu::test_device() else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let mut renderer = Renderer::new(&device, &queue, snapshot::FORMAT, &cube(10.0));

        assert!(
            !renderer.update(&device, &queue, &cube(10.0)),
            "the same model rebuilt the pipeline"
        );
        // A different size is different values against the same source: the
        // fast path the property panel and every drag depend on.
        let resized = cube(12.0);
        assert_ne!(cube(10.0).params, resized.params);
        assert_eq!(cube(10.0).source, resized.source);
        assert!(
            !renderer.update(&device, &queue, &resized),
            "resizing a box rebuilt the pipeline"
        );
        // A different node is a different source, and has to.
        assert!(
            renderer.update(&device, &queue, &spheres(1)),
            "a different node kind did not rebuild the pipeline"
        );
    }

    #[test]
    fn the_parameter_buffer_grows_rather_than_overrunning() {
        let Some((device, queue)) = crate::gpu::test_device() else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        // Start from a model with no parameters at all, which is also the case
        // that must not produce a zero-sized binding.
        let empty = sc_geom::wgsl::generate(&Arena::new(), None);
        assert!(empty.params.is_empty());
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut renderer = Renderer::new(&device, &queue, snapshot::FORMAT, &empty);

        // Straight past the initial capacity, then past the grown one, and on
        // to a count that is not a power of two.
        for count in [1usize, 2, 5, 9, 17] {
            let field = spheres(count);
            assert_eq!(field.params.len(), count);
            renderer.update(&device, &queue, &field);
            let image = snapshot::capture(&device, &queue, 16, 16, |encoder, view| {
                renderer.draw(&queue, encoder, view, &OrbitCamera::default(), 16, 16);
            });
            assert_eq!(image.pixels.len(), 16 * 16 * 4);
        }
        assert!(
            pollster::block_on(scope.pop()).is_none(),
            "the parameter buffer was overrun or bound empty"
        );
    }

    #[test]
    fn a_model_with_no_parameters_still_draws() {
        let Some((device, queue)) = crate::gpu::test_device() else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let empty = sc_geom::wgsl::generate(&Arena::new(), None);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let renderer = Renderer::new(&device, &queue, snapshot::FORMAT, &empty);
        let image = snapshot::capture(&device, &queue, 16, 16, |encoder, view| {
            renderer.draw(&queue, encoder, view, &OrbitCamera::default(), 16, 16);
        });
        assert!(
            pollster::block_on(scope.pop()).is_none(),
            "an empty parameter list produced an invalid binding"
        );
        assert!(image.pixels.iter().any(|&b| b != 0), "nothing was drawn");
    }

    #[test]
    fn the_tracing_tolerance_follows_the_pixel_and_not_the_distance() {
        let Some((device, queue)) = crate::gpu::test_device() else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        // The same scene a hundred times larger and a hundred times further
        // away subtends exactly the same angles, so it has to produce the same
        // image. Scaling the surface tolerance or the normal's sampling radius
        // by the ray distance as well as by the pixel footprint breaks this,
        // and the far one comes back looking carved from soap.
        let near = OrbitCamera {
            target: Vec3::ZERO,
            distance: 60.0,
            yaw: 0.7,
            // Below the plate, so the build grid, whose spacing is absolute, is
            // not in frame to spoil the comparison.
            pitch: -0.5,
            fov_y: 0.8,
        };
        let far = OrbitCamera {
            distance: near.distance * 100.0,
            ..near
        };
        let (w, h) = (96u32, 96u32);
        let shot = |field: &Generated, camera: &OrbitCamera| {
            let renderer = Renderer::new(&device, &queue, snapshot::FORMAT, field);
            snapshot::capture(&device, &queue, w, h, |encoder, view| {
                renderer.draw(&queue, encoder, view, camera, w, h);
            })
        };
        let small = shot(&cube(10.0), &near);
        let large = shot(&cube(1000.0), &far);

        let worst = small
            .pixels
            .iter()
            .zip(&large.pixels)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .expect("the image is not empty");
        assert!(
            worst <= 2,
            "the same scene at a hundred times the scale differed by {worst}/255"
        );
    }
}
