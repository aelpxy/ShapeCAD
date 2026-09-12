//! `ShapeCAD` desktop application.
//!
//! The sphere-traced viewport and the chrome share one wgpu device: the field is
//! drawn straight into the central rectangle and egui composites its panels on
//! top in a second pass. No intermediate texture, no copy.

mod dialog;
mod entry;
mod handle;
mod icon;
mod motion;
mod plane;
mod settings;
mod snap;
mod snapshot;
mod state;
mod theme;
mod tutorial;
mod ui;

use std::sync::Arc;

use sc_geom::glam::{Vec2, Vec3};
use sc_render::{gpu, Renderer};
use state::{AppState, MenuTarget};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        let path = args.get(i + 1).map_or("shapecad.png", String::as_str);
        let scene = snapshot::Scene::from_args(&args);
        // Both palettes have to be capturable, or the dark one can only be
        // checked by running the application and looking at it.
        if args.iter().any(|a| a == "--dark") {
            crate::theme::set_scheme(crate::theme::Scheme::Dark);
        }
        let value = |flag: &str| -> Option<f32> {
            let i = args.iter().position(|a| a == flag)?;
            args.get(i + 1)?.parse().ok()
        };
        let width = value("--width").unwrap_or(1500.0) as u32;
        let height = value("--height").unwrap_or(940.0) as u32;
        let scale = value("--scale").unwrap_or(1.0);
        snapshot::write(std::path::Path::new(path), width, height, scene, scale);
        return;
    }

    // `--display` pins the window to one screen and is remembered, so it only
    // has to be passed once. `--display auto` gives the choice back to the
    // window system.
    let requested = args
        .iter()
        .position(|a| a == "--display")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let event_loop = EventLoop::new().expect("could not create an event loop");

    // A CAD viewport is static most of the time; redrawing only on input keeps
    // the GPU idle instead of spinning at the refresh rate.
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new();
    if args.iter().any(|a| a == "--displays") {
        app.mode = Mode::ListDisplays;
    }
    if let Some(name) = requested {
        app.state
            .set_display(if name == "auto" { None } else { Some(name) });
    }
    event_loop.run_app(&mut app).expect("event loop failed");
}

/// What this run is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Run,
    /// Print the displays and quit. The monitor list is only reachable from
    /// inside the event loop, so this is a mode rather than a plain function.
    ListDisplays,
}

/// Prints the displays the window system is offering, one per line.
///
/// Exists because on Wayland the application cannot tell which of them it is
/// on, so when a window opens somewhere unexpected this is the only way to see
/// what the names are and pass one to `--display`.
fn list_displays(event_loop: &ActiveEventLoop) {
    let primary = event_loop.primary_monitor().and_then(|m| m.name());
    for monitor in event_loop.available_monitors() {
        let name = monitor.name().unwrap_or_else(|| "?".to_string());
        let size = monitor.size();
        let at = monitor.position();
        let mark = if Some(&name) == primary.as_ref() {
            " (primary)"
        } else {
            ""
        };
        println!(
            "{name}{mark}: {}x{} at {},{} scale {}",
            size.width,
            size.height,
            at.x,
            at.y,
            monitor.scale_factor()
        );
    }
    if primary.is_none() {
        println!();
        println!("The window system did not say which display is primary.");
        println!("On Wayland it never does, and it also decides where a window opens.");
    }
    println!();
    println!("Pass one of these names to --display, or `auto` to let the window system choose.");
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

/// Translates winit's report of the desktop appearance into ours.
fn scheme_of(theme: winit::window::Theme) -> crate::theme::Scheme {
    match theme {
        winit::window::Theme::Dark => crate::theme::Scheme::Dark,
        winit::window::Theme::Light => crate::theme::Scheme::Light,
    }
}

/// What the desktop says its colour scheme is, if it says anything.
///
/// `None` is a normal answer, not a failure. Several Wayland compositors do not
/// expose the setting at all, in which case "follow the system" has nothing to
/// follow and resolves to light.
fn window_scheme(window: &Window) -> Option<crate::theme::Scheme> {
    window.theme().map(scheme_of)
}

/// Width of the display in physical pixels, or zero if it cannot be determined.
///
/// Tries the window's own monitor first, then the primary, then infers one from
/// the window's width.
///
/// Wayland tells a client neither which output it is on nor where it is, so
/// both of the direct answers come back empty there, every time and not just
/// before the surface is mapped. The window's own width is the remaining clue:
/// a maximised window is about as wide as the display holding it.
fn monitor_width(event_loop: &ActiveEventLoop, window: &Window) -> u32 {
    if let Some(width) = window
        .current_monitor()
        .or_else(|| event_loop.primary_monitor())
        .map(|m| m.size().width)
        .filter(|w| *w > 0)
    {
        return width;
    }

    let widths: Vec<u32> = event_loop
        .available_monitors()
        .map(|m| m.size().width)
        .filter(|w| *w > 0)
        .collect();
    nearest_monitor_width(&widths, window.inner_size().width)
}

/// Picks the monitor whose width best explains a window of `window_width`.
///
/// Split out from the lookup above so the inference can be tested without a
/// compositor, which is the only place it is ever exercised.
///
/// Guessing the widest monitor instead puts a 4K zoom on a 1080 wide screen
/// whenever the compositor opens the window on the smaller of two displays.
fn nearest_monitor_width(widths: &[u32], window_width: u32) -> u32 {
    if window_width == 0 {
        return widths.iter().copied().max().unwrap_or(0);
    }
    widths
        .iter()
        .copied()
        // A window is never wider than the display holding it, so a narrower
        // monitor cannot be the one it is on. Of those left, the narrowest is
        // the closest fit.
        .filter(|w| *w >= window_width)
        .min()
        .or_else(|| widths.iter().copied().max())
        .unwrap_or(0)
}

/// The pointer in normalised device coordinates, with the viewport aspect.
///
/// Both arguments are in physical pixels, and deliberately so. This is a ratio
/// of one length to another, so the display's scale factor and the interface
/// zoom divide out of both and the answer is the same at any of them. Dividing
/// by `pixels_per_point` here would be harmless; dividing one of the two and
/// not the other is how a click lands somewhere the pointer is not.
///
/// `grab_grip` does convert to interface points, and it is right to. It
/// compares a distance against `ui::GRIP_REACH`, which is a length in points,
/// and against grips put on screen by the very function `ui::grips` draws them
/// with. A ratio needs no unit; a length does.
///
/// `None` for a viewport too small to divide by, which is what a window reports
/// on its way back from being minimised.
fn viewport_ndc(viewport: [f32; 4], cursor: PhysicalPosition<f64>) -> Option<(Vec2, f32)> {
    let [left, top, width, height] = viewport;
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

/// How far the pointer may move between press and release and still count as a
/// click, in physical pixels.
const CLICK_SLOP: f64 = 5.0;

/// How close the pointer has to be to an outline corner to take hold of it, in
/// interface points.
///
/// Wider than a dimension grip, because a corner is drawn small and there is
/// nothing else nearby to grab by mistake: the only alternative reading of the
/// press is adding a point, which the rest of the plane is for.
const POINT_REACH: f32 = 14.0;

/// How long after a click a second one still pairs with it.
const DOUBLE_CLICK_MS: u128 = 350;

/// How far apart in physical pixels the two may land and still be one gesture.
///
/// A hair wider than `CLICK_SLOP`, because the hand has to travel back to the
/// button and settle between the two.
const DOUBLE_CLICK_SLOP: f64 = 6.0;

/// Whether a release pairs with the click before it.
///
/// A double click frames what is under the pointer, which moves the camera, so
/// it has to be sure: near in time and near in space, both.
fn double_click(
    last: (std::time::Instant, PhysicalPosition<f64>),
    now: std::time::Instant,
    at: Option<PhysicalPosition<f64>>,
) -> bool {
    let (when, where_) = last;
    let Some(at) = at else {
        return false;
    };
    let moved = ((at.x - where_.x).powi(2) + (at.y - where_.y).powi(2)).sqrt();
    moved < DOUBLE_CLICK_SLOP && now.duration_since(when).as_millis() < DOUBLE_CLICK_MS
}

/// When the next frame should be drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NextFrame {
    /// Straight away: the camera is still easing, or egui asked for it.
    Now,
    /// At this instant. egui asked to be repainted after a delay.
    At(std::time::Instant),
    /// Not until something happens. Nothing is animating.
    Wait,
}

/// Decides when to draw again from egui's repaint request.
///
/// A delayed request is not a hint that can be dropped. A tooltip appears by
/// asking to be repainted once the pointer has rested long enough, and egui
/// only judges the pointer to be still by running frames in which it did not
/// move. Treating "repaint in 280ms" as "wait for input" means neither ever
/// happens, and nothing appears on hover.
fn next_frame(
    still_moving: bool,
    delay: std::time::Duration,
    now: std::time::Instant,
) -> NextFrame {
    if still_moving || delay.is_zero() {
        NextFrame::Now
    } else if delay == std::time::Duration::MAX {
        NextFrame::Wait
    } else {
        NextFrame::At(now + delay)
    }
}

/// What has to happen to the swapchain before a frame can be drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceState {
    /// The window has no area, which is what a minimised window reports.
    /// Nothing can be drawn and nothing should be requested, or the loop spins.
    Skip,
    /// The swapchain no longer matches the window and has to be rebuilt.
    Rebuild,
    /// The two agree. Draw.
    Ready,
}

/// Compares the swapchain's size with the window's.
///
/// Split out from the frame loop because it is the rule that decides whether a
/// frame is drawn at the right size, and that is worth being able to test
/// without a window.
fn surface_state(config: (u32, u32), window: (u32, u32)) -> SurfaceState {
    if window.0 == 0 || window.1 == 0 {
        SurfaceState::Skip
    } else if config == window {
        SurfaceState::Ready
    } else {
        SurfaceState::Rebuild
    }
}

/// Whether a pointer position belongs to the 3D view rather than to chrome.
///
/// This has to be decided here rather than taken from egui. `egui_wants_pointer_input`
/// is true whenever the pointer is merely *over* an egui area, and the viewport
/// is itself a `CentralPanel`, so honouring egui's `consumed` flag for pointer
/// events swallows every click and drag in the 3D view.
fn viewport_owns_pointer(x: f32, y: f32, viewport: [f32; 4], overlays: &[[f32; 4]]) -> bool {
    let inside = |r: [f32; 4]| x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3];
    inside(viewport) && !overlays.iter().copied().any(inside)
}

/// Takes the next surface texture, recovering from the ways that can fail.
///
/// When it fails, nothing is drawn and the window keeps showing the last image
/// it was given. The event loop waits for input, so another frame has to be
/// asked for here or that stale image stays on screen. Right after a resize
/// that image is the previous frame at the previous size, which is exactly the
/// smaller copy of the interface people see in the corner of the window.
fn acquire(gpu: &Gpu) -> Option<wgpu::SurfaceTexture> {
    match gpu.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
            Some(t)
        }
        other => {
            if matches!(
                other,
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost
            ) {
                gpu.surface.configure(&gpu.device, &gpu.config);
            }
            // An occluded window is the one case where retrying is wrong:
            // nothing can be presented and the request would spin.
            if !matches!(other, wgpu::CurrentSurfaceTexture::Occluded) {
                gpu.window.request_redraw();
            }
            None
        }
    }
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

/// A gesture the press already started, which the release has to end.
///
/// One value rather than two flags, because they are alternatives: a dimension
/// and a whole feature cannot both be following the same pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Started {
    #[default]
    Nothing,
    /// A dimension is being pushed or pulled.
    Drag,
    /// The selection is being dragged around the drag plane.
    Move,
}

/// What the next click was promised to before the button went down.
///
/// Also alternatives, and enforced as such: `AppState::arm` cancels a sketch
/// and `AppState::start_sketch` disarms, because two things waiting for one
/// click means the one checked first always wins and the other never fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pending {
    #[default]
    Nothing,
    /// A feature is armed, waiting to be told where it goes.
    Armed,
    /// A profile is being drawn.
    Sketch,
}

/// What was in flight when a left release arrived.
///
/// A press already decided which of five things it meant, and the release has
/// to reach the same answer. Written down as a value so the combinations can be
/// checked as a table rather than reasoned about across a winit match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct InFlight {
    started: Started,
    pending: Pending,
    /// The pointer stayed within the click slop, so this was a click rather
    /// than a camera gesture the orbit has already had.
    clicked: bool,
    /// The release landed in the 3D view rather than on chrome.
    in_viewport: bool,
}

/// What a left release means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Release {
    /// Nothing: the pointer travelled, so the camera has already had it, or the
    /// release landed somewhere that names nothing in the model.
    Nothing,
    FinishDrag,
    FinishMove,
    PlaceArmed,
    SketchPoint,
    Select,
}

/// Decides what a left release does.
///
/// A press that only dismissed the context menu never reaches here; it is
/// answered where it is recognised, so that closing a menu cannot also select
/// whatever was behind it.
///
/// Two rules, and the reason they are worth writing down separately is that
/// they pull in opposite directions:
///
/// - **Finishing outranks starting, wherever the pointer is.** A push/pull and
///   a free drag each hold an undo step open. A release over the property
///   panel, or past the edge of the window, is still the release that has to
///   close it. Making either conditional on the viewport leaves the step open,
///   and everything the user does afterwards merges into one undo entry.
/// - **Starting something needs the pointer in the 3D view.** Placing a
///   feature, placing a sketch point and selecting all act on a spot in the
///   model, and a release over the tree or a panel names no such spot.
fn left_release(f: InFlight) -> Release {
    match f.started {
        Started::Drag => return Release::FinishDrag,
        Started::Move => return Release::FinishMove,
        Started::Nothing => {}
    }
    if !f.clicked || !f.in_viewport {
        return Release::Nothing;
    }
    match f.pending {
        // An armed feature takes the click: it was armed precisely so that the
        // next one would say where it goes.
        Pending::Armed => Release::PlaceArmed,
        Pending::Sketch => Release::SketchPoint,
        Pending::Nothing => Release::Select,
    }
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
    /// Where the right button went down, so a click can be told from a pan.
    ///
    /// Right drag pans and right click opens the context menu, so the two are
    /// separated the same way the left button separates click from orbit.
    right_press_at: Option<PhysicalPosition<f64>>,
    /// The press currently in flight only closed the context menu, so its
    /// release must not be read as a click on the model.
    dismissing_press: bool,
    /// When the last frame was drawn, for frame-rate independent smoothing.
    last_frame: std::time::Instant,
    /// When and where the last click landed, for double-click detection.
    last_click: Option<(std::time::Instant, PhysicalPosition<f64>)>,
    /// When egui next wants to be repainted, if it asked for a delayed frame.
    repaint_at: Option<std::time::Instant>,
    /// Whether this run draws anything at all.
    mode: Mode,
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
            right_press_at: None,
            dismissing_press: false,
            last_frame: std::time::Instant::now(),
            last_click: None,
            repaint_at: None,
            mode: Mode::Run,
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

    /// Reconciles the swapchain with the window. False if there is nothing to
    /// draw into.
    ///
    /// This happens at draw time rather than when a resize is announced,
    /// because Wayland does not guarantee that a resize arrives before the
    /// frame that needs it. A frame drawn at the old size is still presented
    /// at the new one, which is what leaves a smaller copy of the previous
    /// frame sitting inside the window.
    fn reconcile_surface(&mut self) -> bool {
        let Some(gpu) = &mut self.gpu else {
            return false;
        };
        let size = gpu.window.inner_size();
        match surface_state(
            (gpu.config.width, gpu.config.height),
            (size.width, size.height),
        ) {
            SurfaceState::Skip => return false,
            SurfaceState::Rebuild => {
                gpu.config.width = size.width;
                gpu.config.height = size.height;
                gpu.surface.configure(&gpu.device, &gpu.config);
            }
            SurfaceState::Ready => {}
        }
        true
    }

    fn redraw(&mut self) {
        if !self.reconcile_surface() {
            return;
        }
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

        let Some(frame) = acquire(gpu) else { return };
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
        // Cheap enough to restate every frame, and it means the viewport can
        // never be a frame behind the chrome when the palette changes.
        gpu.field.set_scene(crate::theme::scene());
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

        let delay = output
            .viewport_output
            .values()
            .map(|v| v.repaint_delay)
            .min()
            .unwrap_or(std::time::Duration::MAX);

        match next_frame(still_moving, delay, std::time::Instant::now()) {
            NextFrame::Now => {
                self.repaint_at = None;
                gpu.window.request_redraw();
            }
            NextFrame::At(at) => self.repaint_at = Some(at),
            NextFrame::Wait => self.repaint_at = None,
        }
    }

    /// Sizes the interface for whichever display the window is on.
    ///
    /// Run on every resize rather than once at startup, for two reasons. The
    /// window's size is the only clue to which display holds it, and that size
    /// is not real until the compositor has mapped the surface. And a window
    /// can move between displays afterwards, either because the user dragged it
    /// or because a requested display was honoured a beat late.
    ///
    /// Choosing a zoom by hand switches this off, since the setting then says
    /// what the user wants rather than what the display suggests.
    fn update_auto_scale(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.settings.ui_scale.is_some() {
            return;
        }
        let Some(gpu) = &self.gpu else { return };
        // A window with no area is minimised, and says nothing about which
        // display it is on.
        if gpu.window.inner_size().width == 0 {
            return;
        }
        let width = monitor_width(event_loop, &gpu.window);
        if width == 0 {
            return;
        }
        let native = gpu.window.scale_factor() as f32;
        let auto = settings::auto_scale(width, native);
        if (auto - self.state.ui_scale).abs() > f32::EPSILON {
            println!("display: {width}px wide, scale factor {native}, using zoom {auto}");
            self.state.ui_scale = auto;
        }
    }

    /// The pointer in normalised device coordinates, with the viewport aspect.
    fn pointer_ndc(&self) -> Option<(Vec2, f32)> {
        viewport_ndc(self.viewport, self.cursor?)
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
        Self::distance(self.press_at, self.cursor)
    }

    fn right_drag_distance(&self) -> f64 {
        Self::distance(self.right_press_at, self.cursor)
    }

    fn distance(from: Option<PhysicalPosition<f64>>, to: Option<PhysicalPosition<f64>>) -> f64 {
        match (from, to) {
            (Some(a), Some(b)) => ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt(),
            _ => f64::MAX,
        }
    }

    /// Opens the context menu for whatever the pointer is over.
    ///
    /// The menu position is in interface points, which is what egui lays out
    /// in, so the physical cursor position is divided by the scale factor.
    fn open_context_menu(&mut self) {
        let Some(cursor) = self.cursor else {
            return;
        };
        let scale = self
            .gpu
            .as_ref()
            .map_or(1.0, |gpu| gpu.egui_state.egui_ctx().pixels_per_point());
        let at = (cursor.x as f32 / scale, cursor.y as f32 / scale);

        let target = if self.state.sketch.is_some() {
            MenuTarget::Sketch
        } else {
            // The same pick the left button uses, so the menu is about the
            // thing the user believes they clicked.
            self.select_under_pointer();
            match self.state.selected {
                Some(id) => MenuTarget::Node(id),
                None => MenuTarget::Empty,
            }
        };
        self.state.open_menu(at, target);
        self.request_redraw();
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

    /// Starts a push/pull drag if the pointer is on one of the selection's
    /// grips. True if it took the press.
    ///
    /// The nearest grip wins rather than the first, so that two that overlap on
    /// screen still both reachable: the one whose centre is closer is the one
    /// being aimed at.
    fn grab_grip(&mut self) -> bool {
        // Not while something is armed. That click was promised to the feature
        // waiting to be placed, and a grip that happens to lie under the
        // pointer must not quietly spend it resizing something else instead.
        if self.state.sketch.is_some()
            || self.state.armed.is_some()
            || self.state.tool != state::TOOL_SELECT
        {
            return false;
        }
        let Some(cursor) = self.cursor else {
            return false;
        };
        if self.input.egui_owns || !self.pointer_in_viewport() {
            return false;
        }
        let _ = cursor;
        let Some(pointer) = self.pointer_points() else {
            return false;
        };
        let viewport = self.viewport_points();

        let mut best: Option<(f32, state::Drag)> = None;
        for grip in self.state.grips() {
            let Some((at, axis, gain)) = self.state.grip_on_screen(&grip, viewport) else {
                continue;
            };
            let reach = (pointer - at).length();
            if reach > crate::ui::GRIP_REACH {
                continue;
            }
            if best.as_ref().is_some_and(|(closest, _)| *closest <= reach) {
                continue;
            }
            best = Some((
                reach,
                state::Drag {
                    node: self.state.selected.expect("a grip implies a selection"),
                    param: grip.param,
                    from: grip.value,
                    value: grip.value,
                    axis,
                    gain,
                    origin: pointer,
                },
            ));
        }
        let Some((_, drag)) = best else {
            return false;
        };
        self.state.begin_drag(drag);
        self.input.gesture = Gesture::None;
        true
    }

    /// Everything the pointer moving can mean: a free drag, a push/pull, a
    /// hover that lights a grip, or a camera gesture.
    fn pointer_moved(&mut self, position: PhysicalPosition<f64>) {
        // A corner being dragged owns the pointer, the same way every other
        // drag does.
        if let Some(index) = self.state.grabbed_point {
            self.cursor = Some(position);
            if let Some(at) = self.sketch_plane_hit() {
                let snapped = self.state.snap(at);
                self.state.move_sketch_point(index, snapped);
            }
            self.request_redraw();
            return;
        }
        // A drag in progress owns the pointer outright: no orbit, no
        // pan, and no selection change underneath it.
        if self.state.moving.is_some() {
            self.cursor = Some(position);
            if let Some(now) = self.drag_plane_hit() {
                self.state.move_to_plane(now);
            }
            self.request_redraw();
            return;
        }
        if self.state.drag.is_some() {
            self.cursor = Some(position);
            let ppp = self
                .gpu
                .as_ref()
                .map_or(1.0, |gpu| gpu.egui_state.egui_ctx().pixels_per_point());
            self.state
                .drag_to(Vec2::new(position.x as f32 / ppp, position.y as f32 / ppp));
            self.request_redraw();
            return;
        }
        // The grips light up as the pointer passes over them, so the
        // frame has to be redrawn while a feature is selected even when
        // nothing else is happening.
        if (self.state.selected.is_some() || self.state.armed.is_some())
            && self.pointer_in_viewport()
        {
            self.request_redraw();
        }
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

    /// Interface points the frame is measured in, as opposed to physical pixels.
    ///
    /// Handles are drawn in points and compared against reaches in points, so
    /// everything that hit-tests one has to be in points too, or a grip moves
    /// out from under the pointer as soon as the interface is zoomed.
    fn points_per_pixel(&self) -> f32 {
        self.gpu
            .as_ref()
            .map_or(1.0, |gpu| gpu.egui_state.egui_ctx().pixels_per_point())
    }

    fn pointer_points(&self) -> Option<Vec2> {
        let cursor = self.cursor?;
        let ppp = self.points_per_pixel();
        Some(Vec2::new(cursor.x as f32 / ppp, cursor.y as f32 / ppp))
    }

    fn viewport_points(&self) -> [f32; 4] {
        let ppp = self.points_per_pixel();
        [
            self.viewport[0] / ppp,
            self.viewport[1] / ppp,
            self.viewport[2] / ppp,
            self.viewport[3] / ppp,
        ]
    }

    /// Where the pointer meets the plane a free drag moves across.
    fn drag_plane_hit(&self) -> Option<Vec3> {
        self.constrained_plane_hit(self.state.moving.and_then(|d| d.axis))
    }

    /// The same, for a drag locked to `axis` before the lock is recorded.
    fn constrained_plane_hit(&self, axis: Option<Vec3>) -> Option<Vec3> {
        let normal = match axis {
            Some(a) => self.state.plane_for_axis(a),
            None => self.state.drag_plane(),
        };
        let (ndc, aspect) = self.pointer_ndc()?;
        let origin = self.state.selection_origin()?;
        self.state.camera().plane_hit(ndc, aspect, origin, normal)
    }

    /// Starts dragging the selection if the press landed on it. True if it took
    /// the press.
    fn grab_selection(&mut self) -> bool {
        if self.state.sketch.is_some()
            || self.state.armed.is_some()
            || self.state.tool != state::TOOL_SELECT
            || self.input.egui_owns
            || !self.pointer_in_viewport()
        {
            return false;
        }
        let Some(selected) = self.state.selected else {
            return false;
        };
        // Only when the press is actually on the selected feature, which is the
        // same test the click-to-select path uses.
        let Some((ndc, aspect)) = self.pointer_ndc() else {
            return false;
        };
        let [_, _, _, height] = self.viewport;
        let tolerance = (self.state.camera().world_per_pixel(height) * 4.0).max(0.01);
        if self.state.hit_at(ndc, aspect, tolerance) != Some(selected) {
            return false;
        }
        // Found before the step is opened. Nothing here may fail between
        // `begin_move` and the gesture being recorded, or the step it opened
        // has no release coming to close it.
        let Some(grabbed) = self.drag_plane_hit() else {
            return false;
        };
        if self.state.begin_move(grabbed, height).is_none() {
            return false;
        }
        self.input.gesture = Gesture::None;
        true
    }

    /// Drops the armed feature where the pointer meets the active plane.
    fn place_armed(&mut self) {
        let Some((ndc, aspect)) = self.pointer_ndc() else {
            return;
        };
        let origin = self.state.plane_origin();
        let normal = self.state.plane_normal();
        match self.state.camera().plane_hit(ndc, aspect, origin, normal) {
            Some(hit) => {
                let at = self.state.to_plane(hit);
                self.state.place_armed(at);
            }
            None => self.state.status = "That is not on the active plane".to_string(),
        }
    }

    /// Places a sketch point where the pointer meets the build plate.
    fn place_sketch_point(&mut self) {
        match self.sketch_plane_hit() {
            Some(hit) => {
                let snapped = self.state.snap(hit);
                self.state.add_sketch_point(snapped);
            }
            None => self.state.status = "That is not on the sketch plane".to_string(),
        }
    }

    /// A left press: grabs a grip, grabs the selection, or starts an orbit.
    ///
    /// The order is the order of specificity. A grip is checked first because
    /// the alternative is orbiting the camera the instant someone tries to
    /// resize a feature, and the selection before empty space because a press
    /// on unselected geometry has to stay a way to select it, or nothing could
    /// be picked without being moved by accident.
    fn left_press(&mut self) {
        self.press_at = self.cursor;
        if self.grab_sketch_point() || self.grab_grip() || self.grab_selection() {
            self.request_redraw();
            return;
        }
        self.input.gesture = Gesture::Orbit;
    }

    /// Takes hold of an outline corner, if the press landed on one.
    ///
    /// Checked before everything else while a sketch is in flight, because the
    /// alternative is that trying to move a corner adds another one on top of
    /// it, which is the opposite of what was meant and looks like the corner
    /// refusing to move.
    fn grab_sketch_point(&mut self) -> bool {
        if self.state.sketch.is_none() {
            return false;
        }
        let Some(at) = self.sketch_plane_hit() else {
            return false;
        };
        let [_, _, _, height] = self.viewport;
        // A screen distance, converted once, so a corner is as easy to grab
        // zoomed out as zoomed in.
        let reach = self.state.camera().world_per_pixel(height) * POINT_REACH;
        let Some(index) = self.state.point_near(at, reach) else {
            return false;
        };
        self.state.grabbed_point = Some(index);
        self.input.gesture = Gesture::None;
        true
    }

    /// Where the pointer meets the plane the sketch is drawn on.
    fn sketch_plane_hit(&self) -> Option<Vec2> {
        let (ndc, aspect) = self.pointer_ndc()?;
        let origin = self.state.plane_origin();
        let normal = self.state.plane_normal();
        let hit = self.state.camera().plane_hit(ndc, aspect, origin, normal)?;
        Some(self.state.to_plane(hit))
    }

    /// A left release: whatever the press started, finished.
    fn left_up(&mut self) {
        // A corner that was being dragged has arrived. Answered here rather than
        // through `left_release`, because letting the release fall through would
        // add a point on top of the one just moved.
        if self.state.grabbed_point.take().is_some() {
            self.press_at = None;
            self.input.gesture = Gesture::None;
            self.state.status =
                "Drag a corner to move it, click to add one, Enter to apply".to_string();
            self.request_redraw();
            return;
        }
        let in_flight = InFlight {
            started: if self.state.drag.is_some() {
                Started::Drag
            } else if self.state.moving.is_some() {
                Started::Move
            } else {
                Started::Nothing
            },
            pending: if self.state.armed.is_some() {
                Pending::Armed
            } else if self.state.sketch.is_some() {
                Pending::Sketch
            } else {
                Pending::Nothing
            },
            clicked: self.drag_distance() < CLICK_SLOP,
            in_viewport: self.pointer_in_viewport(),
        };
        // Spent here, so that a release with no press behind it, or one whose
        // press went to the chrome, cannot be measured against an older
        // gesture and read as a click on the model.
        self.press_at = None;
        self.input.gesture = Gesture::None;

        match left_release(in_flight) {
            Release::FinishDrag => {
                self.state.finish_drag();
                self.request_redraw();
                return;
            }
            Release::FinishMove => {
                self.state.finish_move();
                self.request_redraw();
                return;
            }
            Release::PlaceArmed => {
                self.place_armed();
                self.request_redraw();
            }
            Release::SketchPoint => {
                self.place_sketch_point();
                self.request_redraw();
            }
            Release::Select => {
                self.select_under_pointer();
                self.request_redraw();
            }
            Release::Nothing => {}
        }

        // A quick second click in the same spot frames what is under the
        // pointer. Not while sketching, where a second click is a second point,
        // and not on the chrome: a button pressed twice quickly is a button
        // pressed twice, and framing on it aims a ray through a panel at
        // whatever happens to lie behind it.
        if self.state.sketch.is_some() || !in_flight.in_viewport {
            return;
        }
        let now = std::time::Instant::now();
        let is_double = self
            .last_click
            .is_some_and(|last| double_click(last, now, self.cursor));
        if is_double {
            self.focus_under_pointer();
            self.last_click = None;
            self.request_redraw();
        } else {
            self.last_click = self.cursor.map(|c| (now, c));
        }
    }

    /// Press and release of a mouse button: starts a gesture, places a sketch
    /// point, or focuses on a double click.
    fn handle_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let down = state == ElementState::Pressed;

        // A press in the 3D view dismisses an open menu and does nothing else:
        // the click that closes a menu should not also select or orbit. A press
        // over the menu itself is left alone, since that is an item being
        // chosen, and one over a panel is dismissed by `ui::context_menu`.
        if down && self.state.menu.is_some() && self.pointer_in_viewport() {
            self.state.close_menu();
            self.dismissing_press = true;
            self.input.gesture = Gesture::None;
            self.press_at = None;
            self.request_redraw();
            return;
        }
        if !down && self.dismissing_press {
            self.dismissing_press = false;
            self.input.gesture = Gesture::None;
            self.press_at = None;
            self.right_press_at = None;
            return;
        }

        if down && (self.input.egui_owns || !self.pointer_in_viewport()) {
            // The press belongs to the chrome. Forgetting where it landed is
            // what stops the release that follows it being measured against
            // some earlier press and read as a click on the model.
            self.press_at = None;
            return;
        }
        match button {
            MouseButton::Left => {
                if down {
                    self.left_press();
                } else {
                    self.left_up();
                }
            }
            MouseButton::Right => {
                // Not while a left gesture is in flight. Opening a node menu
                // selects what is under the pointer, and changing the selection
                // halfway through a drag leaves the panels describing one
                // feature while the hand is still moving another.
                if self.state.drag.is_some() || self.state.moving.is_some() {
                    return;
                }
                if !down && self.right_drag_distance() < CLICK_SLOP && self.pointer_in_viewport() {
                    // A right click rather than a pan.
                    self.open_context_menu();
                }
                // Forgotten on release, so the next release cannot be measured
                // against a press that has already been spent.
                self.right_press_at = if down { self.cursor } else { None };
                self.input.gesture = if down { Gesture::Pan } else { Gesture::None };
            }
            MouseButton::Middle => {
                self.input.gesture = if down { Gesture::Pan } else { Gesture::None };
            }
            _ => {}
        }
    }

    /// Ends whatever gesture is in flight, as a release would have.
    ///
    /// Keeping where the drag got to rather than putting it back: the user's
    /// last intent was where they left the pointer, and a feature that snaps
    /// home because a notification stole focus is worse than one that stayed.
    /// What matters is that the undo step closes, which both do.
    fn end_gesture(&mut self) {
        self.state.finish_drag();
        self.state.finish_move();
        self.input.gesture = Gesture::None;
        self.press_at = None;
        self.right_press_at = None;
        self.dismissing_press = false;
        self.request_redraw();
    }

    fn request_redraw(&self) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.mode == Mode::ListDisplays {
            list_displays(event_loop);
            event_loop.exit();
            return;
        }
        if self.gpu.is_some() {
            return;
        }

        // A named display is only a request. Wayland ignores a position, which
        // is why the fallback below exists.
        let chosen = self.state.settings.display.as_ref().and_then(|want| {
            event_loop
                .available_monitors()
                .find(|m| m.name().as_deref() == Some(want.as_str()))
        });

        let mut attrs = Window::default_attributes()
            .with_title("ShapeCAD")
            // Opens filling the display. A CAD viewport is worth the whole
            // screen, and on a 4K panel a 1500 point window is a postage stamp.
            // The inner size is what un-maximising restores to.
            .with_maximized(true)
            .with_inner_size(winit::dpi::LogicalSize::new(1500.0, 940.0))
            // Below this the panels take the whole window and the 3D view has
            // nowhere left to go.
            .with_min_inner_size(winit::dpi::LogicalSize::new(900.0, 600.0));
        if let Some(monitor) = &chosen {
            attrs = attrs.with_position(monitor.position());
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("could not open a window"),
        );

        // Where a window goes is the window system's decision, and Wayland
        // gives a client no way to influence it: no position, no output. The
        // one primitive it does offer that names an output is fullscreen, so
        // that is how a chosen display is honoured there. Positioning worked if
        // the window can say where it is.
        if let Some(monitor) = chosen {
            if window.outer_position().is_err() {
                window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(Some(monitor))));
            }
        }

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
        // Asked before the first frame, so the window does not flash light and
        // then correct itself.
        self.state.set_system_scheme(window_scheme(&window));
        self.state.apply_appearance();
        crate::theme::apply(&ctx);
        // Unasked, on a first launch only. Putting it behind a menu item means
        // the people who most need it are the least likely to find it, and it
        // costs nothing to dismiss.
        if !self.state.settings.tutorial_seen {
            self.state.start_tutorial();
        }
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

    /// Sleeps until the next event, or until egui's delayed frame is due.
    ///
    /// The event loop otherwise waits for input, which is right for a CAD
    /// viewport that is static most of the time, but it means a frame egui
    /// asked for in half a second never arrives unless the timer is set here.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(at) = self.repaint_at else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        if at <= std::time::Instant::now() {
            self.repaint_at = None;
            self.request_redraw();
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(at));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // The chrome sees every event, but only gets to *claim* keyboard ones.
        //
        // For the pointer, egui reports "consumed" whenever the cursor is over
        // any of its areas, and the viewport is a panel, so deferring to it
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

            WindowEvent::Resized(_) => {
                // The new size is read back from the window in `redraw`, so
                // there is exactly one place that decides how big the surface
                // is and it cannot disagree with the frame being drawn.
                self.update_auto_scale(event_loop);
                self.request_redraw();
            }

            // Moving the window to a display with a different density.
            WindowEvent::ScaleFactorChanged { .. } => {
                self.update_auto_scale(event_loop);
                self.request_redraw();
            }

            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(state, button);
            }

            // The release that would have ended a gesture is not coming. Alt
            // tabbing with the button down, or a compositor taking the pointer
            // for a workspace switch, both end the drag without a button event,
            // and a drag holds an undo step open: left that way, every edit for
            // the rest of the session merges into one entry that undo takes
            // back in a single press.
            WindowEvent::Focused(false) => {
                self.end_gesture();
            }

            // The desktop switched between light and dark while we were running.
            // Only matters when the user asked to follow it, which
            // `set_system_scheme` decides.
            WindowEvent::ThemeChanged(theme) => {
                self.state.set_system_scheme(Some(scheme_of(theme)));
                if let Some(gpu) = self.gpu.as_ref() {
                    crate::theme::apply(gpu.egui_state.egui_ctx());
                }
                self.request_redraw();
            }

            WindowEvent::CursorMoved { position, .. } => self.pointer_moved(position),

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
    use super::{
        double_click, left_release, nearest_monitor_width, next_frame, surface_state, viewport_ndc,
        viewport_owns_pointer, App, InFlight, NextFrame, Pending, Release, Started, SurfaceState,
    };
    use sc_geom::glam::Vec3;
    use sc_geom::Node;
    use std::time::{Duration, Instant};
    use winit::dpi::PhysicalPosition;

    /// A release with nothing in flight, in the 3D view, that did not travel.
    fn click() -> InFlight {
        InFlight {
            started: Started::Nothing,
            pending: Pending::Nothing,
            clicked: true,
            in_viewport: true,
        }
    }

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
    /// A minimised window reports no area. Drawing is impossible, and asking
    /// for another frame would spin the event loop at full speed.
    #[test]
    fn a_window_with_no_area_is_skipped() {
        assert_eq!(surface_state((800, 600), (0, 600)), SurfaceState::Skip);
        assert_eq!(surface_state((800, 600), (800, 0)), SurfaceState::Skip);
        assert_eq!(surface_state((800, 600), (0, 0)), SurfaceState::Skip);
    }

    /// The case behind the stale frame: the window grew, the swapchain did not.
    /// Drawing without rebuilding presents the old, smaller frame in the new,
    /// larger window.
    #[test]
    fn a_resized_window_rebuilds_the_swapchain() {
        assert_eq!(
            surface_state((1500, 940), (3840, 2053)),
            SurfaceState::Rebuild
        );
        assert_eq!(
            surface_state((3840, 2053), (1500, 940)),
            SurfaceState::Rebuild
        );
        assert_eq!(
            surface_state((1500, 940), (1500, 941)),
            SurfaceState::Rebuild
        );
    }

    #[test]
    fn a_matching_swapchain_is_drawn_straight_away() {
        assert_eq!(surface_state((1500, 940), (1500, 940)), SurfaceState::Ready);
    }
    /// The regression behind tooltips never appearing. egui asks to be
    /// repainted a fraction of a second after the pointer stops, and that
    /// frame has to be scheduled: the event loop is otherwise waiting for
    /// input that is not coming, because the pointer is deliberately still.
    #[test]
    fn a_delayed_repaint_is_scheduled_rather_than_dropped() {
        let now = Instant::now();
        let delay = Duration::from_millis(280);
        assert_eq!(
            next_frame(false, delay, now),
            NextFrame::At(now + delay),
            "a finite delay must become a deadline"
        );
    }

    #[test]
    fn nothing_animating_waits_for_input() {
        let now = Instant::now();
        assert_eq!(next_frame(false, Duration::MAX, now), NextFrame::Wait);
    }

    /// An easing camera outranks any delay: it needs the next frame now.
    #[test]
    fn an_immediate_request_draws_straight_away() {
        let now = Instant::now();
        assert_eq!(next_frame(false, Duration::ZERO, now), NextFrame::Now);
        assert_eq!(next_frame(true, Duration::MAX, now), NextFrame::Now);
        assert_eq!(
            next_frame(true, Duration::from_millis(280), now),
            NextFrame::Now
        );
    }
    /// Wayland never says which display a window is on, so the interface scale
    /// is picked from the window's own width. Taking the widest monitor
    /// instead puts a 4K zoom on a 1080 wide screen.
    #[test]
    fn the_monitor_is_inferred_from_the_window_width() {
        let displays = [3840, 1080];
        assert_eq!(nearest_monitor_width(&displays, 1080), 1080);
        assert_eq!(nearest_monitor_width(&displays, 3840), 3840);
        // A window smaller than either still belongs to the smaller one more
        // plausibly than to the larger.
        assert_eq!(nearest_monitor_width(&displays, 900), 1080);
    }

    /// A window wider than every display means the guess has gone wrong, so
    /// fall back to the widest rather than reporting nothing.
    #[test]
    fn an_impossible_window_falls_back_to_the_widest() {
        assert_eq!(nearest_monitor_width(&[3840, 1080], 5000), 3840);
        assert_eq!(nearest_monitor_width(&[], 1920), 0);
    }

    /// Before the surface is mapped the window has no size to go on.
    #[test]
    fn an_unmapped_window_falls_back_to_the_widest() {
        assert_eq!(nearest_monitor_width(&[3840, 1080], 0), 3840);
    }

    /// The five things a left press can mean, in the order they outrank each
    /// other. Written as a table because the bug is never in one case, it is in
    /// two of them both believing they own the click.
    #[test]
    fn a_click_in_the_view_means_one_thing_at_a_time() {
        assert_eq!(left_release(click()), Release::Select);
        assert_eq!(
            left_release(InFlight {
                pending: Pending::Sketch,
                ..click()
            }),
            Release::SketchPoint
        );
        assert_eq!(
            left_release(InFlight {
                pending: Pending::Armed,
                ..click()
            }),
            Release::PlaceArmed
        );
        assert_eq!(
            left_release(InFlight {
                started: Started::Move,
                ..click()
            }),
            Release::FinishMove
        );
        assert_eq!(
            left_release(InFlight {
                started: Started::Drag,
                ..click()
            }),
            Release::FinishDrag
        );
    }

    /// The case that leaves an undo step open. A push/pull and a free drag each
    /// hold one from the press to the release, and a release that landed on the
    /// property panel, or past the edge of the window, is still the release
    /// that has to close it. Refusing it there means every edit for the rest of
    /// the session merges into the one entry undo takes back.
    #[test]
    fn a_release_outside_the_view_still_ends_a_drag() {
        for started in [Started::Drag, Started::Move] {
            for clicked in [true, false] {
                let out = left_release(InFlight {
                    started,
                    clicked,
                    in_viewport: false,
                    ..click()
                });
                assert_ne!(
                    out,
                    Release::Nothing,
                    "{started:?} released outside the view was dropped, leaving its step open"
                );
            }
        }
    }

    /// Starting something, unlike finishing it, needs a spot in the model.
    /// Placing a feature, placing a sketch point and selecting all name one,
    /// and a release over the tree or a panel names none.
    #[test]
    fn a_release_outside_the_view_starts_nothing() {
        for pending in [Pending::Nothing, Pending::Armed, Pending::Sketch] {
            assert_eq!(
                left_release(InFlight {
                    pending,
                    in_viewport: false,
                    ..click()
                }),
                Release::Nothing,
                "{pending:?} acted on a release that landed on the chrome"
            );
        }
    }

    /// A press that travelled was a camera gesture and the orbit has already
    /// had it. Dropping a sketch point at the end of an orbit would put a
    /// corner wherever the drag happened to stop.
    #[test]
    fn a_drag_of_the_camera_places_nothing() {
        for pending in [Pending::Nothing, Pending::Armed, Pending::Sketch] {
            assert_eq!(
                left_release(InFlight {
                    pending,
                    clicked: false,
                    ..click()
                }),
                Release::Nothing
            );
        }
    }

    /// The release that ends a gesture does not always arrive. Alt tabbing with
    /// the button down, or a compositor taking the pointer for a workspace
    /// switch, ends the drag with no button event at all.
    ///
    /// A drag holds an undo step open from the press to the release. Left that
    /// way, every edit for the rest of the session lands in the same step, and
    /// one press of undo takes back an afternoon's work rather than one action.
    /// `Focused(false)` is where that is caught, so this drives the same call.
    #[test]
    fn losing_the_window_mid_drag_closes_the_undo_step() {
        let mut app = App::new();
        app.state.new_document();
        app.state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        app.state.begin_move(Vec3::ZERO, 940.0).expect("movable");

        // No release is coming: the window lost focus with the button down.
        app.end_gesture();
        assert!(app.state.moving.is_none(), "the gesture is still in flight");

        app.state.add_body(Node::Sphere { radius: 4.0 }, "First");
        let after_first = app.state.doc.hash().expect("rooted");
        app.state.add_body(
            Node::Box {
                half: Vec3::splat(3.0),
                round: 0.0,
            },
            "Second",
        );

        app.state.undo();

        assert_eq!(
            app.state.doc.hash().expect("rooted"),
            after_first,
            "one undo took back two bodies, so the drag left its step open"
        );
    }

    /// Physical pixels against interface points, which is the unit mistake the
    /// input handling is most exposed to. Where the pointer sits in the
    /// viewport is a ratio of one length to another, so the display's scale
    /// factor cancels: the same pointer on the same pixel gives the same answer
    /// at 1x and at 2x, and nothing here needs dividing by `pixels_per_point`.
    ///
    /// The asymmetry with `grab_grip`, which does divide, is deliberate. That
    /// one compares a distance against a reach measured in points, and a length
    /// carries a unit where a ratio does not.
    #[test]
    fn the_pointer_lands_in_the_same_place_at_any_interface_scale() {
        let at_1x = viewport_ndc(
            [300.0, 60.0, 1000.0, 800.0],
            PhysicalPosition::new(800.0, 260.0),
        );
        let at_2x = viewport_ndc(
            [600.0, 120.0, 2000.0, 1600.0],
            PhysicalPosition::new(1600.0, 520.0),
        );
        assert_eq!(at_1x, at_2x, "the same click read differently at 2x");

        let (ndc, aspect) = at_1x.expect("a viewport with area");
        assert!((ndc.x - 0.0).abs() < 1.0e-6, "got {ndc:?}");
        assert!((ndc.y - 0.5).abs() < 1.0e-6, "got {ndc:?}");
        assert!((aspect - 1.25).abs() < 1.0e-6, "got {aspect}");
    }

    /// A window on its way back from being minimised reports a viewport with no
    /// area, and dividing by it gives an infinity that traces a ray to nowhere.
    #[test]
    fn a_viewport_with_no_area_has_no_pointer_position() {
        assert!(viewport_ndc([0.0, 0.0, 0.0, 800.0], PhysicalPosition::new(5.0, 5.0)).is_none());
        assert!(viewport_ndc([0.0, 0.0, 1000.0, 0.0], PhysicalPosition::new(5.0, 5.0)).is_none());
    }

    /// A double click moves the camera, so it has to be sure. Two clicks a
    /// quarter of a second apart on opposite sides of the viewport are two
    /// clicks, and so are two in the same place a second apart.
    #[test]
    fn a_double_click_is_near_in_both_time_and_space() {
        let first = Instant::now();
        let at = PhysicalPosition::new(700.0, 400.0);
        let soon = first + Duration::from_millis(100);
        let later = first + Duration::from_millis(900);

        assert!(double_click((first, at), soon, Some(at)));
        assert!(
            !double_click((first, at), soon, Some(PhysicalPosition::new(760.0, 400.0))),
            "two clicks a finger apart paired up"
        );
        assert!(
            !double_click((first, at), later, Some(at)),
            "a click most of a second later paired up"
        );
        assert!(
            !double_click((first, at), soon, None),
            "a release with no pointer position paired up"
        );
    }
}
