//! Headless rendering.
//!
//! Renders the field to an image with no window and no surface. This exists for
//! two reasons: tests can assert that the renderer produces something, and the
//! agent layer needs a way to *see* what it has built in order to judge it.

use crate::camera::OrbitCamera;
use crate::renderer::Renderer;

/// An 8-bit RGBA image.
#[derive(Clone, Debug)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tightly packed RGBA rows, top to bottom.
    pub pixels: Vec<u8>,
}

/// Texture copies require each row to start on a 256-byte boundary, so the
/// staging buffer is usually wider than the image and has to be unpadded.
const COPY_ALIGNMENT: u32 = 256;

/// Renders `field` from `camera` into an image, or `None` if the machine
/// has no usable GPU.
#[must_use]
pub fn try_render(
    field: &sc_geom::wgsl::Generated,
    camera: &OrbitCamera,
    width: u32,
    height: u32,
    preference: crate::gpu::Preference,
) -> Option<Image> {
    let instance = crate::gpu::instance();
    let adapter = crate::gpu::try_adapter(&instance, None, preference)?;
    let (device, queue) = crate::gpu::device(&adapter);
    Some(render_with(&device, &queue, field, camera, width, height))
}

/// Renders `field` from `camera` into an image.
///
/// # Panics
/// If no GPU adapter is available.
#[must_use]
pub fn render(
    field: &sc_geom::wgsl::Generated,
    camera: &OrbitCamera,
    width: u32,
    height: u32,
) -> Image {
    try_render(
        field,
        camera,
        width,
        height,
        crate::gpu::Preference::Hardware,
    )
    .expect("no usable GPU adapter found")
}

/// Renders using a caller-supplied device.
///
/// Creating a second wgpu instance while one is already live is not reliable on
/// every driver, so anything that already owns a device (the viewport, or a
/// test that probed for an adapter) must come through here rather than through
/// [`render`].
///
/// # Panics
/// If the device rejects the work or the readback buffer cannot be mapped.
/// Both indicate a broken driver rather than a recoverable condition.
#[must_use]
pub fn render_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    field: &sc_geom::wgsl::Generated,
    camera: &OrbitCamera,
    width: u32,
    height: u32,
) -> Image {
    let renderer = Renderer::new(device, queue, FORMAT, field);
    capture(device, queue, width, height, |encoder, view| {
        renderer.draw(queue, encoder, view, camera, width, height);
    })
}

/// The texture format offscreen capture renders into.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Renders whatever `record` draws into an offscreen target and reads it back.
///
/// Separated from [`render_with`] so callers that compose several passes (the
/// application draws the field and then its chrome over it) can capture the
/// composite rather than just the 3D view.
///
/// # Panics
/// If the device rejects the work or the readback buffer cannot be mapped.
#[must_use]
pub fn capture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    record: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
) -> Image {
    let (width, height) = (width.max(1), height.max(1));

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("snapshot target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let padded_row = (width * 4).div_ceil(COPY_ALIGNMENT) * COPY_ALIGNMENT;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("snapshot staging"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("snapshot"),
    });
    record(&mut encoder, &view);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("buffer mapping failed"));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll failed");

    let mapped = slice.get_mapped_range().expect("mapped range unavailable");
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * padded_row) as usize;
        pixels.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
    }
    drop(mapped);
    staging.unmap();

    Image {
        width,
        height,
        pixels,
    }
}

/// Writes an [`Image`] to a PNG file.
///
/// # Errors
/// Any I/O or encoding failure.
pub fn write_png(image: &Image, path: &std::path::Path) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&image.pixels)?;
    Ok(())
}
