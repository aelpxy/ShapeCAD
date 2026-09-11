//! `ShapeCAD` desktop application.
//!
//! The sphere-traced viewport and the chrome share one wgpu device: the field is
//! drawn straight into the central rectangle and egui composites its panels on
//! top in a second pass. No intermediate texture, no copy.

mod dialog;
mod plane;
mod settings;
mod snapshot;
mod state;
mod theme;
mod ui;

use std::sync::Arc;

use sc_geom::glam::Vec2;
use sc_render::{gpu, Renderer};
use state::AppState;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        let path = args.get(i + 1).map_or("shapecad.png", String::as_str);
        let dialog = args.iter().any(|a| a == "--dialog");
        let value = |flag: &str| -> Option<f32> {
            let i = args.iter().position(|a| a == flag)?;
            args.get(i + 1)?.parse().ok()
        };
        let width = value("--width").unwrap_or(1500.0) as u32;
        let height = value("--height").unwrap_or(940.0) as u32;
        let scale = value("--scale").unwrap_or(1.0);
        snapshot::write(std::path::Path::new(path), width, height, dialog, scale);
        return;
    }

    let event_loop = EventLoop::new().expect("could not create an event loop");
    // A CAD viewport is static most of the time; redrawing only on input keeps
    // the GPU idle instead of spinning at the refresh rate.
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop
        .run_app(&mut App::new())
        .expect("event loop failed");
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    field: Renderer,
    egui_renderer: egui_wgpu::Renderer,
    egui_state: egui_winit::State,
}

/// Width of the display in physical pixels, or zero if it cannot be determined.
///
/// Tries the window's own monitor first, then the primary, then the widest
/// available. Wayland answers none of these reliably before the surface is
/// mapped, which is why the caller retries.
fn monitor_width(event_loop: &ActiveEventLoop, window: &Window) -> u32 {
    window
        .current_monitor()
        .or_else(|| event_loop.primary_monitor())
        .map(|m| m.size().width)
        .filter(|w| *w > 0)
        .or_else(|| {
            event_loop
                .available_monitors()
                .map(|m| m.size().width)
                .max()
        })
        .unwrap_or(0)
}

/// How far the pointer may move between press and release and still count as a
/// click, in physical pixels.
const CLICK_SLOP: f64 = 5.0;

/// Whether a pointer position belongs to the 3D view rather than to chrome.
///
/// This has to be decided here rather than taken from egui. `egui_wants_pointer_input`
/// is true whenever the pointer is merely *over* an egui area, and the viewport
/// is itself a `CentralPanel` — so honouring egui's `consumed` flag for pointer
/// events swallows every click and drag in the 3D view.
fn viewport_owns_pointer(x: f32, y: f32, viewport: [f32; 4], overlays: &[[f32; 4]]) -> bool {
    let inside = |r: [f32; 4]| x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3];
    inside(viewport) && !overlays.iter().copied().any(inside)
}

/// Uploads egui's textures and buffers, then draws its output over the frame.
///
/// Split out of the frame loop purely for size; it is one linear sequence with
/// no decisions in it.
#[allow(clippy::too_many_arguments)]
fn composite_chrome(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    jobs: &[egui::ClippedPrimitive],
    descriptor: &egui_wgpu::ScreenDescriptor,
    textures: &egui::TexturesDelta,
) {
    // A texture can arrive as several partial updates in one frame.
    for (id, deltas) in &textures.set {
        for delta in deltas {
            renderer.update_texture(device, queue, *id, delta);
        }
    }
    renderer.update_buffers(device, queue, encoder, jobs, descriptor);

    {
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
        renderer.render(&mut pass.forget_lifetime(), jobs, descriptor);
    }

    for id in &textures.free {
        renderer.free_texture(id);
    }
}

/// What a drag is currently doing. Orbit and pan are alternatives, not two
/// independent flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Gesture {
    #[default]
    None,
    Orbit,
    Pan,
}

/// Pointer state.
#[derive(Clone, Copy, Debug, Default)]
struct Pointer {
    gesture: Gesture,
    /// Whether egui claimed the pointer on the previous frame. Camera gestures
    /// are ignored while a panel has it, so dragging a slider does not also
    /// spin the model.
    egui_owns: bool,
    /// Shift turns a left drag into a pan, the convention most tools share.
    shift: bool,
}

struct App {
    gpu: Option<Gpu>,
    state: AppState,
    /// The region left for the 3D view, in physical pixels.
    viewport: [f32; 4],
    /// Interactive chrome floating over the viewport, in physical pixels.
    overlays: Vec<[f32; 4]>,
    /// Pointer state, grouped so the gestures read together.
    input: Pointer,
    cursor: Option<PhysicalPosition<f64>>,
    /// Where the left button went down, so a click can be told from a drag.
    ///
    /// In sketch mode the left button both places points and orbits; the
    /// difference is whether the pointer moved.
    press_at: Option<PhysicalPosition<f64>>,
    /// When the last frame was drawn, for frame-rate independent smoothing.
    last_frame: std::time::Instant,
    /// When and where the last click landed, for double-click detection.
    last_click: Option<(std::time::Instant, PhysicalPosition<f64>)>,
    /// The display could not be measured at startup; retry once it is mapped.
    scale_undetermined: bool,
}

impl App {
    fn new() -> Self {
        Self {
            gpu: None,
            state: AppState::new(),
            viewport: [0.0, 0.0, 1.0, 1.0],
            overlays: Vec::new(),
            input: Pointer::default(),
            cursor: None,
            press_at: None,
            last_frame: std::time::Instant::now(),
            last_click: None,
            scale_undetermined: false,
        }
    }

    fn pointer_in_viewport(&self) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        viewport_owns_pointer(
            cursor.x as f32,
            cursor.y as f32,
            self.viewport,
            &self.overlays,
        )
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(gpu) = &mut self.gpu {
            if width == 0 || height == 0 {
                return;
            }
            gpu.config.width = width;
            gpu.config.height = height;
            gpu.surface.configure(&gpu.device, &gpu.config);
        }
    }

    fn redraw(&mut self) {
        let Some(gpu) = &mut self.gpu else { return };

        // Ease the camera toward wherever input sent it, and keep requesting
        // frames until it has settled.
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        let still_moving = self.state.rig.advance(dt);

        // 1. Build the UI and learn how much room is left for the model.
        let raw_input = gpu.egui_state.take_egui_input(&gpu.window);
        let ctx = gpu.egui_state.egui_ctx().clone();
        // Zoom multiplies the display's own scale factor, so this composes with
        // a compositor that already reports HiDPI rather than fighting it.
        if (ctx.zoom_factor() - self.state.ui_scale).abs() > f32::EPSILON {
            ctx.set_zoom_factor(self.state.ui_scale);
        }
        let mut chrome = ui::Chrome::default();
        let output = ctx.run_ui(raw_input, |ui| {
            chrome = ui::draw(ui, &mut self.state);
        });
        gpu.egui_state
            .handle_platform_output(&gpu.window, output.platform_output);
        // Actively dragging a widget, as opposed to merely hovering one.
        self.input.egui_owns = ctx.egui_is_using_pointer();

        let ppp = output.pixels_per_point;
        let to_pixels = |r: egui::Rect| {
            [
                r.min.x * ppp,
                r.min.y * ppp,
                r.width() * ppp,
                r.height() * ppp,
            ]
        };
        self.viewport = to_pixels(chrome.viewport);
        self.overlays = chrome.overlays.iter().map(|r| to_pixels(*r)).collect();

        // 2. Push the model to the GPU if it changed. Values live in a buffer,
        //    so most edits are an upload; only a change to the generated source
        //    costs a pipeline rebuild.
        if self.state.field_dirty {
            let started = std::time::Instant::now();
            let generated = self.state.wgsl();
            let rebuilt = gpu.field.update(&gpu.device, &gpu.queue, &generated);
            self.state.last_edit_ms = started.elapsed().as_secs_f32() * 1000.0;
            self.state.last_edit_rebuilt = rebuilt;
            self.state.field_dirty = false;
        }

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        // 3. The field, confined to the central rectangle.
        let clamped = [
            self.viewport[0],
            self.viewport[1],
            self.viewport[2].min(gpu.config.width as f32 - self.viewport[0]),
            self.viewport[3].min(gpu.config.height as f32 - self.viewport[1]),
        ];
        gpu.field.draw_in(
            &gpu.queue,
            &mut encoder,
            &view,
            &self.state.camera(),
            clamped,
        );

        // 4. The chrome, composited over it.
        let jobs = ctx.tessellate(output.shapes, ppp);
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: ppp,
        };
        composite_chrome(
            &gpu.device,
            &gpu.queue,
            &mut gpu.egui_renderer,
            &mut encoder,
            &view,
            &jobs,
            &descriptor,
            &output.textures_delta,
        );

        gpu.queue.submit(Some(encoder.finish()));
        gpu.queue.present(frame);

        if still_moving
            || output
                .viewport_output
                .values()
                .any(|v| v.repaint_delay.is_zero())
        {
            gpu.window.request_redraw();
        }
    }

    /// Second attempt at sizing the interface for the display.
    ///
    /// Wayland frequently reports no monitor until the surface has been mapped,
    /// which is after the window is created but before the first resize.
    fn retry_scale_detection(&mut self, event_loop: &ActiveEventLoop) {
        if !self.scale_undetermined {
            return;
        }
        let Some(gpu) = &self.gpu else { return };
        let width = monitor_width(event_loop, &gpu.window);
        if width == 0 {
            return;
        }
        self.scale_undetermined = false;
        let native = gpu.window.scale_factor() as f32;
        let auto = settings::auto_scale(width, native);
        if (auto - self.state.ui_scale).abs() > f32::EPSILON {
            println!("display: {width}px wide, scale factor {native}, using zoom {auto}");
            self.state.ui_scale = auto;
        }
    }

    /// The pointer in normalised device coordinates, with the viewport aspect.
    fn pointer_ndc(&self) -> Option<(Vec2, f32)> {
        let cursor = self.cursor?;
        let [left, top, width, height] = self.viewport;
        if width <= 1.0 || height <= 1.0 {
            return None;
        }
        Some((
            Vec2::new(
                ((cursor.x as f32 - left) / width) * 2.0 - 1.0,
                1.0 - ((cursor.y as f32 - top) / height) * 2.0,
            ),
            width / height,
        ))
    }

    /// Centres the view on whatever is under the pointer.
    fn focus_under_pointer(&mut self) {
        let Some((ndc, aspect)) = self.pointer_ndc() else {
            return;
        };
        self.state.rig.goal.target = self.state.pick_world(ndc, aspect);
        self.state.status = "Focused".to_string();
    }

    /// How far the pointer travelled since the button went down.
    ///
    /// A press and release in the same place is a click; anything further was a
    /// drag, and the camera has already acted on it.
    fn drag_distance(&self) -> f64 {
        match (self.press_at, self.cursor) {
            (Some(a), Some(b)) => ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt(),
            _ => f64::MAX,
        }
    }

    /// Selects whatever is under the pointer, or clears the selection.
    fn select_under_pointer(&mut self) {
        let Some((ndc, aspect)) = self.pointer_ndc() else {
            return;
        };
        let [_, _, _, height] = self.viewport;
        // A few pixels' worth of world units, so a click near an edge reads as
        // the seam rather than demanding sub-millimetre accuracy.
        let tolerance = (self.state.camera().world_per_pixel(height) * 4.0).max(0.01);
        self.state.select_at(ndc, aspect, tolerance);
    }

    /// Places a sketch point where the pointer meets the build plate.
    fn place_sketch_point(&mut self) {
        let Some(cursor) = self.cursor else { return };
        let [left, top, width, height] = self.viewport;
        let ndc = Vec2::new(
            ((cursor.x as f32 - left) / width) * 2.0 - 1.0,
            1.0 - ((cursor.y as f32 - top) / height) * 2.0,
        );
        let aspect = width / height.max(1.0);
        let plane = self.state.plane;
        match self
            .state
            .camera()
            .plane_hit(ndc, aspect, sc_geom::glam::Vec3::ZERO, plane.normal())
        {
            Some(hit) => {
                let snapped = self.state.snap(plane.to_plane(hit));
                self.state.add_sketch_point(snapped);
            }
            None => self.state.status = format!("That is not on the {} plane", plane.name()),
        }
    }

    /// Press and release of a mouse button: starts a gesture, places a sketch
    /// point, or focuses on a double click.
    fn handle_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let down = state == ElementState::Pressed;
        if down && (self.input.egui_owns || !self.pointer_in_viewport()) {
            return;
        }
        match button {
            MouseButton::Left => {
                if down {
                    self.press_at = self.cursor;
                } else if self.state.sketch.is_some() {
                    // A click places a point; a drag orbited instead.
                    if self.drag_distance() < CLICK_SLOP {
                        self.place_sketch_point();
                        self.request_redraw();
                    }
                } else if !down {
                    // A click rather than a drag, with the pointer tool armed:
                    // select whatever is under it.
                    if self.drag_distance() < CLICK_SLOP && self.pointer_in_viewport() {
                        self.select_under_pointer();
                        self.request_redraw();
                    }
                }

                // A quick second click in the same spot frames what is
                // under the pointer.
                if !down && self.state.sketch.is_none() {
                    let now = std::time::Instant::now();
                    let is_double = self.last_click.is_some_and(|(at, pos)| {
                        let close = self.cursor.is_some_and(|c| {
                            ((c.x - pos.x).powi(2) + (c.y - pos.y).powi(2)).sqrt() < 6.0
                        });
                        close && now.duration_since(at).as_millis() < 350
                    });
                    if is_double {
                        self.focus_under_pointer();
                        self.last_click = None;
                        self.request_redraw();
                    } else {
                        self.last_click = self.cursor.map(|c| (now, c));
                    }
                }
                self.input.gesture = if down { Gesture::Orbit } else { Gesture::None };
            }
            MouseButton::Right | MouseButton::Middle => {
                self.input.gesture = if down { Gesture::Pan } else { Gesture::None };
            }
            _ => {}
        }
    }

    fn request_redraw(&self) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("ShapeCAD")
            .with_inner_size(winit::dpi::LogicalSize::new(1500.0, 940.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("could not open a window"),
        );

        let instance = gpu::instance();
        let surface = instance
            .create_surface(window.clone())
            .expect("could not create a surface");
        let adapter = gpu::adapter(&instance, Some(&surface), gpu::Preference::Hardware);
        println!(
            "gpu: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().device_type
        );
        let (device, queue) = gpu::device(&adapter);

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface is incompatible with this adapter");
        let caps = surface.get_capabilities(&adapter);
        if let Some(srgb) = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
        {
            config.format = srgb;
        }
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);

        let field = Renderer::new(&device, &queue, config.format, &self.state.wgsl());
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        // Many Linux compositors report 1.0 on a 4K panel. Measure the display
        // and pick a sensible zoom rather than rendering everything half-size.
        let native = window.scale_factor() as f32;
        let monitor_width = monitor_width(event_loop, &window);
        self.state.ui_scale = self.state.settings.ui_scale.unwrap_or_else(|| {
            let auto = settings::auto_scale(monitor_width, native);
            println!("display: {monitor_width}px wide, scale factor {native}, using zoom {auto}");
            auto
        });
        // Wayland often cannot name a monitor until the window is mapped. If it
        // could not, try again on the first resize rather than leaving someone
        // on a 4K panel with half-size controls.
        self.scale_undetermined = monitor_width == 0 && self.state.settings.ui_scale.is_none();

        let egui_state = egui_winit::State::new(
            ctx,
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        self.gpu = Some(Gpu {
            window,
            surface,
            device,
            queue,
            config,
            field,
            egui_renderer,
            egui_state,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // The chrome sees every event, but only gets to *claim* keyboard ones.
        //
        // For the pointer, egui reports "consumed" whenever the cursor is over
        // any of its areas — and the viewport is a panel — so deferring to it
        // would mean the 3D view never receives a click. Pointer ownership is
        // decided by `viewport_owns_pointer` at each use instead.
        if let Some(gpu) = &mut self.gpu {
            let response = gpu.egui_state.on_window_event(&gpu.window, &event);
            if response.repaint {
                gpu.window.request_redraw();
            }
            let pointer_event = matches!(
                event,
                WindowEvent::MouseInput { .. }
                    | WindowEvent::CursorMoved { .. }
                    | WindowEvent::MouseWheel { .. }
            );
            if response.consumed && !pointer_event {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::ModifiersChanged(mods) => {
                self.input.shift = mods.state().shift_key();
            }

            WindowEvent::Resized(size) => {
                self.resize(size.width, size.height);
                self.retry_scale_detection(event_loop);
                self.request_redraw();
            }

            // Moving the window to a display with a different density.
            WindowEvent::ScaleFactorChanged { .. } => {
                self.request_redraw();
            }

            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(state, button);
            }

            WindowEvent::CursorMoved { position, .. } => {
                if let Some(prev) = self.cursor {
                    let dx = (position.x - prev.x) as f32;
                    let dy = (position.y - prev.y) as f32;
                    let [_, _, width, height] = self.viewport;
                    // Shift turns an orbit drag into a pan, as most tools do.
                    let pan = self.input.gesture == Gesture::Pan
                        || (self.input.gesture == Gesture::Orbit && self.input.shift);
                    if pan {
                        self.state.rig.goal.pan_pixels(dx, dy, height);
                        self.request_redraw();
                    } else if self.input.gesture == Gesture::Orbit {
                        self.state
                            .rig
                            .goal
                            .orbit_pixels(dx, dy, Vec2::new(width, height));
                        self.request_redraw();
                    }
                }
                self.cursor = Some(position);
                if self.state.sketch.is_some() {
                    self.request_redraw();
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if self.input.egui_owns || !self.pointer_in_viewport() {
                    return;
                }
                let ticks = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                let factor = 0.85f32.powf(ticks);
                // Zoom toward whatever is under the pointer rather than toward
                // the orbit target, so the wheel pulls you into what you are
                // looking at.
                if let Some((ndc, aspect)) = self.pointer_ndc() {
                    let anchor = self.state.pick_world(ndc, aspect);
                    self.state.rig.goal.zoom_towards(factor, anchor);
                } else {
                    self.state.rig.goal.zoom(factor);
                }
                self.request_redraw();
            }

            WindowEvent::RedrawRequested => self.redraw(),

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::viewport_owns_pointer;

    const VIEWPORT: [f32; 4] = [300.0, 60.0, 1000.0, 800.0];

    #[test]
    fn a_point_in_the_open_viewport_belongs_to_the_model() {
        assert!(viewport_owns_pointer(700.0, 400.0, VIEWPORT, &[]));
    }

    #[test]
    fn a_point_over_a_side_panel_does_not() {
        assert!(!viewport_owns_pointer(100.0, 400.0, VIEWPORT, &[]));
        assert!(!viewport_owns_pointer(1500.0, 400.0, VIEWPORT, &[]));
        assert!(!viewport_owns_pointer(700.0, 20.0, VIEWPORT, &[]));
    }

    #[test]
    fn a_point_over_floating_chrome_does_not() {
        // The tool bar sits inside the viewport but is still interface.
        let toolbar = [600.0, 780.0, 400.0, 50.0];
        assert!(!viewport_owns_pointer(700.0, 800.0, VIEWPORT, &[toolbar]));
        assert!(viewport_owns_pointer(700.0, 700.0, VIEWPORT, &[toolbar]));
    }

    #[test]
    fn an_open_modal_claims_everything() {
        let everything = [f32::MIN / 2.0, f32::MIN / 2.0, f32::MAX, f32::MAX];
        assert!(!viewport_owns_pointer(
            700.0,
            400.0,
            VIEWPORT,
            &[everything]
        ));
    }

    #[test]
    fn the_viewport_edges_are_inclusive() {
        assert!(viewport_owns_pointer(300.0, 60.0, VIEWPORT, &[]));
        assert!(viewport_owns_pointer(1300.0, 860.0, VIEWPORT, &[]));
        assert!(!viewport_owns_pointer(1300.1, 860.0, VIEWPORT, &[]));
    }
}
