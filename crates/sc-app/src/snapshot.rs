//! Headless capture of the whole application, chrome included.
//!
//! egui needs no window: given a synthetic screen rectangle it will lay out and
//! tessellate exactly as it would on screen. That makes the full UI renderable
//! without a display, which is what lets the interface be reviewed in CI — and,
//! later, lets an agent see what the user is looking at.

use crate::state::AppState;
use crate::ui;
use sc_render::{gpu, snapshot as capture, Renderer};

/// Renders one frame of the application to a PNG.
///
/// # Panics
/// If no GPU is available or the image cannot be written.
pub(crate) fn write(path: &std::path::Path, width: u32, height: u32, dialog: bool, scale: f32) {
    let mut state = AppState::new();
    if dialog {
        state.browse(crate::dialog::Purpose::Open);
    }

    let instance = gpu::instance();
    let adapter = gpu::adapter(&instance, None, gpu::Preference::Hardware);
    let (device, queue) = gpu::device(&adapter);

    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.set_zoom_factor(scale);

    // `screen_rect` is in points, not pixels. Passing the pixel size here lays
    // the interface out for a screen `scale` times too large and clips it.
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width as f32 / scale, height as f32 / scale),
        )),
        ..Default::default()
    };

    let field = Renderer::new(&device, capture::FORMAT, &state.wgsl());
    let mut egui_renderer = egui_wgpu::Renderer::new(
        &device,
        capture::FORMAT,
        egui_wgpu::RendererOptions::default(),
    );

    // Several passes. egui gives a newly created Area a sizing frame before it
    // can place itself, so anything that just appeared - the file browser, say -
    // is still invisible after one, and fade-in animations need time to run.
    // The last frame is the settled layout.
    //
    // Texture deltas from *both* frames have to be applied: the font atlas is
    // created during the first, and every egui primitive samples it. Dropping
    // that delta leaves the renderer with no atlas, and it silently skips the
    // entire UI.
    let mut chrome = crate::ui::Chrome::default();
    let mut output = None;
    for pass in 0..3 {
        // Time has to advance between passes or egui's animations never run:
        // a modal would be captured mid fade-in, half transparent.
        let mut frame_input = input.clone();
        frame_input.time = Some(f64::from(pass));
        let frame = ctx.run_ui(frame_input, |ui| {
            chrome = ui::draw(ui, &mut state);
        });
        for (id, deltas) in &frame.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(&device, &queue, *id, delta);
            }
        }
        output = Some(frame);
    }
    let output = output.expect("frames were run");

    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let descriptor = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: output.pixels_per_point,
    };

    let image = capture::capture(&device, &queue, width, height, |encoder, view| {
        field.draw_in(
            &queue,
            encoder,
            view,
            &state.camera(),
            [
                chrome.viewport.min.x * scale,
                chrome.viewport.min.y * scale,
                chrome.viewport.width() * scale,
                chrome.viewport.height() * scale,
            ],
        );
        egui_renderer.update_buffers(&device, &queue, encoder, &jobs, &descriptor);
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        egui_renderer.render(&mut pass.forget_lifetime(), &jobs, &descriptor);
    });

    capture::write_png(&image, path).expect("could not write the image");
    println!("wrote {} ({width}x{height})", path.display());
}
