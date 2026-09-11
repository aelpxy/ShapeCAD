# Rendering

The viewport sphere-traces the implicit field directly. There is no tessellation
anywhere in the display path, so what you see is the actual surface at any zoom
rather than an approximation of it. Meshing happens only on export.

## The frame

1. `wgsl::generate_with_selection` emits a module containing `sc_sdf` and
   `sc_selected`.
2. `sc_render::Renderer` compiles it with the sphere-tracing shader around it and
   draws a single full-viewport triangle.
3. The application composites its chrome over the result in a second pass, onto
   the same device, with no intermediate texture and no copy.

`sc_selected` is always emitted, returning empty space when nothing is selected,
so the shader has one contract and never needs two variants.

## Tolerances come from the pixel, not the distance

A ray is "at" the surface once it is within about half a pixel of it. Past that,
more precision cannot change the image. The pixel's angular size is passed in and
the tolerance is `t * pixel_angle * 0.5`.

Scaling a tolerance by ray distance *as well* is the obvious-looking mistake and
it is badly wrong: at a hundred millimetres out it turns a 0.02mm threshold into
several millimetres, and the normal, sampled over the same radius, averages every
edge in the scene into a curve. The symptom is a model that looks like it was
carved from soap, on faces nowhere near a fillet.

The same applies to the gradient used for shading, which samples over roughly one
pixel.

## Shadow rays

Soft shadows estimate a penumbra from how closely a ray passes the surface. Two
details keep them clean:

- The ray starts offset along the normal by a *view-relative* amount. Offsetting
  by a fraction of the far plane is far too coarse up close and too fine far
  away, which produces stippling along silhouettes.
- Surfaces already facing away from the light skip the march entirely. It can
  only return zero there, and marching from a grazing angle is exactly where
  self-intersection artefacts come from.

## Generated shaders

The emitter produces SSA statements memoised by (node, point expression). A
shared subtree evaluated under two different transforms is genuinely two
different computations, so the key cannot be the node alone.

Two things it does that are worth preserving:

**Peepholes.** An identity transform emits nothing: no `* 1.0`, no no-op
quaternion rotation, no `- 0.0` for zero rounding. That arithmetic sits inside a
loop running on the order of a hundred times per pixel.

**Helper functions.** Anything needing a loop or an array, such as an extruded profile,
is emitted as its own function rather than inlined, via `Emitter::helpers`.

### naga will not accept `vec3<bool>`

A boolean vector constructor fails to parse, so the polygon crossing test is
written as three scalar booleans. Generated WGSL is validated with `naga` in the
kernel's tests, which catches this class of problem in `cargo test` rather than as
an opaque driver error later.

## Camera

Gestures are measured in fractions of the viewport, not in pixels, so the same
physical drag does the same thing on a 4K panel and a laptop.

Two behaviours are load-bearing and have tests:

**Zoom keeps what is under the pointer under the pointer.** Scaling the target's
offset from the anchor by the same factor as the distance leaves the anchor on the
same ray from the eye. This is most of what makes a wheel feel like it is pulling
you into what you are looking at.

**Pan is exactly 1:1 at the target's depth**, so the point you grabbed does not
drift out from under the cursor.

`CameraRig` eases the visible camera toward the goal that input manipulates.
Without it every gesture lands as a hard cut, which reads as clunky even when the
mapping is right.

## GPU selection

`sc_render::gpu` picks the adapter for the whole project so the viewport and
offscreen capture cannot disagree. It enables non-conformant adapters, because under WSL
the only hardware path reports itself that way, and hiding it leaves nothing but a
software rasteriser.

`Preference::Software` deliberately restricts itself to the GL backend rather than
merely preferring a CPU adapter. See [building.md](building.md#running-under-wsl)
for why that distinction matters.

## Pointer ownership

egui reports a pointer event as consumed whenever the cursor is merely *over* one
of its areas, and the viewport is itself a panel. Honouring that flag for pointer
events means the 3D view never receives a click: no selection, no orbit, no
sketching.

So the application decides ownership itself: `ui::draw` reports where the chrome
landed, and `viewport_owns_pointer` tests the cursor against the viewport and the
floating overlays. egui's `consumed` flag is still honoured for *keyboard* events,
so typing in a field does not fire shortcuts.
