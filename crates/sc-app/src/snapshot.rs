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
/// points.
///
/// The panel is laid out from the top down, so this does not depend on the size
/// of the window, only on what is above the row. That changes: it was on the row
/// until a card was added above it, after which the capture rested the pointer
/// on the ADD heading and no tooltip appeared, which looks exactly like a
/// tooltip that stopped working. `the_hover_scene_shows_a_tooltip` is what
/// notices next time.
const HOVER_POINT: egui::Pos2 = egui::pos2(100.0, 358.0);

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
    /// The engine example, framed.
    Engine,
    /// A circular pattern, to show the count doing its job.
    Pattern,
    /// The guided tour on its first card.
    Tutorial,
    /// A selected box with its dimension grips showing.
    Grips,
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
    // The capture sets the palette directly, so the switch has to be told what
    // it is showing or it would report the wrong one.
    state.settings.appearance = match crate::theme::scheme() {
        crate::theme::Scheme::Dark => crate::settings::Appearance::Dark,
        crate::theme::Scheme::Light => crate::settings::Appearance::Light,
    };
    match scene {
        // Hover changes where the pointer is, not what the document holds.
        Scene::Empty | Scene::Hover => {}
        Scene::Dialog => state.browse(crate::dialog::Purpose::Open),
        Scene::Sample => {
            state.load_sample();
            let root = state.doc.root();
            state.select(root);
        }
        Scene::Tutorial => {
            state.start_tutorial();
        }
        Scene::Pattern => {
            state.new_document();
            state.add_body(
                sc_geom::Node::Cylinder {
                    radius: 4.0,
                    half_height: 3.0,
                    round: 0.5,
                },
                "Boss",
            );
            // Off the axis, or every instance of a circular pattern lands in
            // one place and the capture shows one boss.
            state.move_selection(sc_geom::glam::Vec3::new(22.0, 0.0, 0.0));
            state.repeat_selection(sc_geom::node::Repeat::Circular {
                sweep: std::f32::consts::TAU,
            });
            if let Some(id) = state.selected {
                state.apply(sc_doc::Command::SetParam {
                    id,
                    name: "count".into(),
                    value: 6.0,
                });
            }
            state.frame_model();
            state.rig.snap_to(state.rig.goal);
            state.status = "Ready".to_string();
        }
        Scene::Grips => {
            state.new_document();
            state.add_body(
                sc_geom::Node::Box {
                    half: sc_geom::glam::Vec3::new(24.0, 16.0, 10.0),
                    round: 2.0,
                },
                "Block",
            );
            state.frame_model();
            state.rig.goal.yaw = -0.9;
            state.rig.goal.pitch = 0.5;
            state.rig.snap_to(state.rig.goal);
            state.status = "Ready".to_string();
        }
        Scene::Engine => {
            state.load_engine();
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

/// How many passes a capture runs before the frame it keeps.
///
/// Several are needed whatever the scene: egui gives a newly created Area a
/// sizing pass before it can place itself, so anything that has just appeared,
/// the file browser say, is still invisible after one.
///
/// The number is set by the animation. The interface moves on springs, and the
/// integrator clamps a pass to a thirtieth of a second however much virtual time
/// it claims, so the slowest tuning needs fifteen passes to come to rest and the
/// margin above that covers a tooltip, which spends its first passes waiting for
/// the pointer to be still. Three passes caught the context menu a third of the
/// way through growing: half transparent and slightly small.
/// `there_are_enough_passes_for_the_slowest_animation` keeps this honest.
const PASSES: u32 = 24;

/// The pass the pointer is moved on.
///
/// Not the first, because where a grip is depends on the viewport rect, and the
/// rect the first pass reports is not the real one: `set_zoom_factor` is applied
/// on the next pass to run, and applying it replaces `screen_rect` with the
/// previous one scaled by the change in zoom. At a zoom of 1.5 over a default
/// rect that came out as a 6346 by 6636 viewport, four times the window, and a
/// grip projected through it lands far from where it is drawn. The second pass
/// has the rect that was actually passed in.
const POINTER_PASS: u32 = 2;

/// Virtual seconds between passes.
///
/// Longer than the tooltip delay divided by the passes it can afford to spend
/// waiting, and longer than the integrator's own clamp, so each pass advances
/// every animation by as much as it ever will.
const PASS_SECONDS: f64 = 0.25;

/// Runs the interface for [`PASSES`] passes and reports the last one.
///
/// Shared with the tests, which drive the same passes without a GPU: a scene is
/// only worth capturing if it poses what its name says, and that is a question
/// about the frame rather than about the picture.
fn run(
    ctx: &egui::Context,
    state: &mut AppState,
    input: &egui::RawInput,
    scene: Scene,
    mut on_pass: impl FnMut(&mut egui::FullOutput),
) -> (egui::FullOutput, ui::Chrome) {
    let mut chrome = ui::Chrome::default();
    let mut output = None;
    for pass in 0..PASSES {
        // Time has to advance between passes or egui's animations never run:
        // a modal would be captured mid fade-in, half transparent.
        let mut frame_input = input.clone();
        frame_input.time = Some(f64::from(pass) * PASS_SECONDS);
        // The pointer is moved once and then left alone: egui measures the
        // tooltip delay from the last movement, so repeating the event on every
        // pass would keep resetting it and no tooltip would ever appear.
        if pass == POINTER_PASS {
            if let Some(at) = pointer_for(scene, state, chrome.viewport) {
                frame_input.events.push(egui::Event::PointerMoved(at));
            }
        }
        let mut frame = ctx.run_ui(frame_input, |ui| {
            chrome = ui::draw(ui, state);
        });
        on_pass(&mut frame);
        output = Some(frame);
    }
    (output.expect("frames were run"), chrome)
}

/// Where a scene rests the pointer, in points.
///
/// `Scene::Grips` works its answer out from the pose rather than having it
/// written down. A grip's place on screen follows the camera, the viewport rect
/// and the size of the window, so a position read off one capture is wrong in
/// the next one, and the failure is silent: the grips are all still there, just
/// none of them hovered, which is exactly what the scene exists to show.
fn pointer_for(scene: Scene, state: &AppState, viewport: egui::Rect) -> Option<egui::Pos2> {
    match scene {
        Scene::Hover => Some(HOVER_POINT),
        Scene::Grips => {
            let rect = [
                viewport.min.x,
                viewport.min.y,
                viewport.width(),
                viewport.height(),
            ];
            let grip = state.grips().into_iter().find(|g| g.param == "half_x")?;
            let (at, _, _) = state.grip_on_screen(&grip, rect)?;
            Some(egui::pos2(at.x, at.y))
        }
        _ => None,
    }
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
    let mut field = Renderer::new(&device, &queue, capture::FORMAT, &generated);
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
    let (output, chrome) = run(&ctx, &mut state, &input, scene, |frame| {
        for (id, deltas) in &frame.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(&device, &queue, *id, delta);
            }
        }
    });

    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let descriptor = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: output.pixels_per_point,
    };

    let image = capture::capture(&device, &queue, width, height, |encoder, view| {
        field.set_scene(crate::theme::scene());
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

#[cfg(test)]
mod tests {
    use super::{pointer_for, pose, run, Scene, PASSES, PASS_SECONDS};
    use crate::motion::{Spring, Tuning};

    /// The size the documented capture command lays the interface out at:
    /// 2400 by 1500 pixels at a scale of 1.5.
    const POINTS: egui::Vec2 = egui::vec2(1600.0, 1000.0);

    /// Drives the passes a capture drives, without a GPU.
    fn capture(scene: Scene) -> (egui::Context, crate::state::AppState, crate::ui::Chrome) {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut state = pose(scene);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, POINTS)),
            ..Default::default()
        };
        let (mut output, chrome) = run(&ctx, &mut state, &input, scene, |frame| {
            // Nothing here uploads textures, and epaint refuses to let a delta
            // be dropped unhandled.
            frame.textures_delta.clear();
        });
        output.textures_delta.clear();
        (ctx, state, chrome)
    }

    /// The scene exists to put a tooltip in the frame. Resting the pointer a few
    /// points off the row shows no tooltip at all, and the capture still looks
    /// like a perfectly good picture of the application, so nothing says the
    /// flag has stopped doing what it is for.
    #[test]
    fn the_hover_scene_shows_a_tooltip() {
        let (ctx, _, _) = capture(Scene::Hover);
        let tooltip = ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .iter()
                .any(|layer| layer.order == egui::Order::Tooltip)
        });
        assert!(
            tooltip,
            "the pointer rested where the scene puts it and nothing explained itself"
        );
    }

    /// And this one exists to show a grip ready to be grabbed. A pointer that
    /// lands anywhere else captures three grips at rest, which is what the
    /// picture would look like anyway.
    #[test]
    fn the_grips_scene_rests_the_pointer_on_a_grip() {
        let (_, state, chrome) = capture(Scene::Grips);
        let at = pointer_for(Scene::Grips, &state, chrome.viewport)
            .expect("the block's half_x grip is on screen");
        assert!(
            chrome.viewport.contains(at),
            "the pointer landed at {at:?}, outside the viewport {:?}",
            chrome.viewport
        );

        let rect = [
            chrome.viewport.min.x,
            chrome.viewport.min.y,
            chrome.viewport.width(),
            chrome.viewport.height(),
        ];
        let near = state.grips().iter().any(|grip| {
            state.grip_on_screen(grip, rect).is_some_and(|(p, _, _)| {
                (egui::pos2(p.x, p.y) - at).length() < crate::ui::GRIP_REACH
            })
        });
        assert!(near, "the pointer at {at:?} is not within reach of a grip");
        let (ctx2, _, _) = capture(Scene::Grips);
        println!("pointer after the passes: {:?}", ctx2.pointer_latest_pos());
        println!("wanted: {at:?} viewport {:?}", chrome.viewport);
    }

    /// A capture is one frame, so every animation in it has to have finished.
    /// The integrator clamps a pass to a thirtieth of a second however much
    /// virtual time it claims, which is what makes this a fixed number of passes
    /// rather than a duration.
    #[test]
    fn there_are_enough_passes_for_the_slowest_animation() {
        for tuning in [Tuning::SMOOTH, Tuning::BOUNCY, Tuning::SNAPPY] {
            let mut spring = Spring::at(0.0);
            let mut passes = 0;
            while !spring.settled(1.0) {
                spring = spring.step(1.0, PASS_SECONDS as f32, tuning);
                passes += 1;
                assert!(passes < 1000, "{tuning:?} never settled at all");
            }
            assert!(
                passes < PASSES,
                "{tuning:?} needs {passes} passes and a capture runs {PASSES}"
            );
        }
    }

    /// Every scene has to hold up what its name promises, and most of that is
    /// in the document it poses.
    #[test]
    fn every_scene_poses_what_it_is_called() {
        assert!(pose(Scene::Empty).doc.arena().is_empty());
        assert!(pose(Scene::Hover).doc.arena().is_empty());

        let sample = pose(Scene::Sample);
        assert_eq!(sample.selected, sample.doc.root(), "the root is selected");
        assert!(!sample.doc.arena().is_empty());

        let engine = pose(Scene::Engine);
        assert!(
            engine.doc.arena().len() > sample.doc.arena().len() * 3,
            "the engine scene is not posing the engine"
        );

        assert!(
            pose(Scene::Dialog).browser.is_some(),
            "the dialog scene has no file browser open"
        );
        assert!(
            pose(Scene::Menu).menu.is_some(),
            "the menu scene has no menu open"
        );
        assert_eq!(
            pose(Scene::Tutorial).tutorial.map(|t| t.at),
            Some(0),
            "the tutorial scene is not on the first card"
        );

        let grips = pose(Scene::Grips);
        assert_eq!(grips.grips().len(), 3, "a rounded block has three grips");

        // The showcase selects the transform that stands the wall up, not the
        // wall, so the highlight is where the wall is rather than at the origin.
        let showcase = pose(Scene::Showcase);
        let selected = showcase.selected.expect("something is selected");
        assert!(
            matches!(
                showcase.doc.arena().get(selected),
                Some(sc_geom::Node::Transform { .. })
            ),
            "the showcase selected {:?}",
            showcase.doc.arena().get(selected)
        );
    }
}
