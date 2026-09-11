//! Headless capture of the whole application, chrome included.
//!
//! egui needs no window: given a synthetic screen rectangle it will lay out and
//! tessellate exactly as it would on screen. That makes the full UI renderable
//! without a display, which is what lets the interface be reviewed in CI, and
//! later, lets an agent see what the user is looking at.

use crate::state::AppState;
use crate::ui;
use sc_render::{gpu, snapshot as capture, Renderer};

/// Where the pointer sits for [`Scene::Hover`]: the Rectangle tool row, in
/// points. Chosen by eye from a capture at the default size.
const HOVER_POINT: egui::Pos2 = egui::pos2(100.0, 315.0);

/// How [`Scene::Showcase`] poses the camera: a three quarter view from the open
/// side, so the filleted joint, both drilled holes and the upright face are all
/// in frame at once.
const SHOWCASE_YAW: f32 = -1.05;
const SHOWCASE_PITCH: f32 = 0.55;
const SHOWCASE_ZOOM: f32 = 1.02;

/// Where [`Scene::Menu`] opens the context menu, in points. Over the middle of
/// the 3D view, which is where a right click on the model would land.
const MENU_POINT: (f32, f32) = (620.0, 380.0);

/// What the captured frame should show beyond an empty document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scene {
    /// A new, empty document.
    #[default]
    Empty,
    /// The file browser open over an empty document.
    Dialog,
    /// The bundled sample model, with its root selected.
    Sample,
    /// The pointer resting on a tool row, so its tooltip is captured.
    Hover,
    /// The context menu open on the sample model's root.
    Menu,
    /// The sample part, posed for the screenshot in the readme.
    Showcase,
}

/// The node [`Scene::Showcase`] selects: the transform that places the upright
/// wall.
///
/// The wall is found by being the tallest box, but selecting the box itself
/// tints it at its canonical position on the origin, which paints a band lying
/// across the plate rather than the wall standing up. The transform above it is
/// the node that puts the wall where it is, so that is what gets highlighted,
/// and its offsets are worth showing in the property panel.
fn showcase_subject(state: &AppState) -> Option<sc_geom::NodeId> {
    let arena = state.doc.arena();
    let wall = arena
        .live_ids()
        .filter_map(|id| match arena.get(id) {
            Some(sc_geom::Node::Box { half, .. }) => Some((id, half.z)),
            _ => None,
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)?;

    arena.live_ids().find(|id| {
        matches!(
            arena.get(*id),
            Some(sc_geom::Node::Transform { child, .. }) if *child == wall
        )
    })
}

/// Builds the document and camera a scene asks for.
fn pose(scene: Scene) -> AppState {
    let mut state = AppState::new();
    match scene {
        // Hover changes where the pointer is, not what the document holds.
        Scene::Empty | Scene::Hover => {}
        Scene::Dialog => state.browse(crate::dialog::Purpose::Open),
        Scene::Sample => {
            state.load_sample();
            let root = state.doc.root();
            state.select(root);
        }
        Scene::Showcase => {
            state.load_sample();
            // A real feature, so the property panel has dimensions in it rather
            // than a transform's zeroes.
            state.select(showcase_subject(&state));
            state.frame_model();
            state.rig.goal.yaw = SHOWCASE_YAW;
            state.rig.goal.pitch = SHOWCASE_PITCH;
            state.rig.goal.distance *= SHOWCASE_ZOOM;
            state.rig.snap_to(state.rig.goal);
            // The status bar reports the sample being loaded otherwise, which
            // is noise in a picture of the application at rest.
            state.status = "Ready".to_string();
        }
        Scene::Menu => {
            state.load_sample();
            if let Some(root) = state.doc.root() {
                state.open_menu(MENU_POINT, crate::state::MenuTarget::Node(root));
            }
        }
    }
    state
}

/// Renders one frame of the application to a PNG.
///
/// # Panics
/// If no GPU is available or the image cannot be written.
pub(crate) fn write(path: &std::path::Path, width: u32, height: u32, scene: Scene, scale: f32) {
    let mut state = pose(scene);

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

    let generated = state.wgsl();
    let field = Renderer::new(&device, &queue, capture::FORMAT, &generated);
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
    // A tooltip only appears after the pointer has rested on a widget, and its
    // Area then needs its own sizing pass, so the hover capture runs longer.
    let passes = if scene == Scene::Hover { 8 } else { 3 };
    for pass in 0..passes {
        // Time has to advance between passes or egui's animations never run:
        // a modal would be captured mid fade-in, half transparent.
        let mut frame_input = input.clone();
        frame_input.time = Some(f64::from(pass) * 0.25);
        // The pointer is moved once and then left alone: egui measures the
        // tooltip delay from the last movement, so repeating the event on every
        // pass would keep resetting it and no tooltip would ever appear.
        if scene == Scene::Hover && pass == 0 {
            frame_input
                .events
                .push(egui::Event::PointerMoved(HOVER_POINT));
        }
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
