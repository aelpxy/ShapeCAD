# The interface

Everything the user sees lives in `sc-app`. It is built from four files:

| File | Holds |
|---|---|
| `theme.rs` | The palette, the metrics, and every reusable widget |
| `icon.rs` | The icon set, drawn rather than shipped |
| `ui.rs` | The layout: what goes in which panel, and what each control does |
| `state.rs` | Everything the interface reads and writes. No widget touches the kernel directly |

`ui.rs` never picks a colour, a radius or a font size. If a screen needs one it
goes in `theme.rs` first. That is what keeps a hundred widgets looking like one
application.

## The palette

A single neutral ramp, one accent, one red. Nothing else. Read `theme::palette()`
rather than naming a colour at the call site; it returns whichever of the two
schemes is in force.

| Field | Used for |
|---|---|
| `canvas` | The space between cards |
| `surface` | A card, a popup, a floating bar |
| `surface_alt` | A hovered or active row, an input field |
| `border` | The hairline that separates a card from the canvas |
| `text` / `text_dim` | Primary and secondary text |
| `accent` / `accent_soft` | Selection, and the tint behind it |
| `danger` | Destructive actions only |
| `ink` / `on_ink` | The single primary button and the application mark, and what is written on them |
| `sky` / `haze` / `plate` / `grid` | The viewport: background sweep, build plate, grid lines |

Depth comes from the border, not from shadows. Only things that genuinely float
over the 3D view get one, and even then it is faint.

## Grouped lists

The tool panel is a grouped list, which is the shape this kind of list takes on
Apple's platforms. Rows carry no chrome of their own: the group is a raised
surface, the rows inside it are divided by hairlines inset to where the label
starts, and the section header sits outside the group and above it.

A group only reads as raised against the canvas, so the sections deliberately sit
directly on the panel background rather than inside a card. Nested in one they
would be surface on surface and the separators would be doing all the work.

Use `theme::grouped`, which hands you a `Rows` and puts the hairlines in. The
separator belongs between rows and nowhere else, which is fiddly to get right by
hand in a list whose length depends on what is selected.

## Light and dark

Two palettes, chosen by `Settings::appearance`: `Light`, `Dark`, or `System`.
`System` is a request, not a result. It becomes one of the other two by asking
winit what the desktop is set to, and `None` is a normal answer there: several
Wayland compositors never report a scheme, in which case there is nothing to
follow and it resolves to light.

The scheme is a process-wide `AtomicU8` rather than a value threaded through
every function that draws. A desktop application has exactly one appearance at a
time, and passing a palette into every label would be ceremony with no reader.

Three things need doing together when it changes, and missing any one shows:

- `theme::set_scheme` swaps the palette.
- `theme::apply` rebuilds egui's style, which caches colours rather than reading
  them per frame. Calling it again is safe; it is written to be idempotent.
- `field_dirty` is set, so the viewport rebuilds. Its colours live in the
  shader's uniform, which nothing repaints on its own.

egui stays pinned to its light style in both schemes. Every colour that shows is
set from the palette, and letting egui swap its own base underneath would change
the handful that are not named here, giving a scheme assembled from two sources.

**The viewport colours are linear, the chrome colours are sRGB.** The shader
writes into a linear target and the swapchain encodes on the way out, so a value
picked to look right as a hex colour comes out about two and a half times too
light. `#1F1F23` is `0.0137`, not `0.12`. The first dark viewport was written as
if it were sRGB and came out mid grey, which read as a dimmed light mode rather
than a dark one. `the_dark_scene_is_actually_dark` pins it.

## Motion

Animation is driven by springs, in `motion.rs`, not by a duration and an easing
curve. The difference shows when something changes target mid-flight, which in a
tool that responds to every click is most of the time: a tween restarts from
where it was and loses its velocity, so an interrupted animation stutters. A
spring carries the velocity across and bends toward the new target.

Tunings are a response time and a damping fraction, the two numbers a person can
reason about. Stiffness and mass are not.

| Tuning | For |
|---|---|
| `SMOOTH` | Hover fills, colour changes, anything a pointer is driving. No overshoot |
| `BOUNCY` | Things that appear: a menu opening, a selection moving to a new segment |
| `SNAPPY` | Press feedback. Slower than this reads as lag, because the finger has already gone |

`animate` keeps its state in egui's memory under an id, so a caller owns nothing,
and requests a repaint while the spring is moving. `animate_from` is for things
that appear and therefore have no previous position to carry; whoever uses it
must `forget` the id when the thing goes away, or the next appearance starts
where the last one finished.

Two traps, both found by the tests rather than by looking:

**Integrate in fixed slices, not once per frame.** One step per frame makes the
motion depend on the frame rate: at a response of 0.3 seconds a 60Hz frame is a
third of a radian, far enough into the integration error to see. The same motion
reached 0.68 at 60Hz and 0.63 at 144Hz. `MAX_SUBSTEP` fixes it at four slices
per frame at 60Hz, which costs nothing.

**Clamp the frame time.** A frame longer than `MAX_STEP` is a stall, not a slow
frame: the window was occluded, or a shader was compiling. Integrating it
honestly flings every spring in the interface at once.

## Widgets

`theme.rs` exposes the whole vocabulary. Use one of these rather than building a
widget inline:

- `card()` is the unit the layout is made of. A panel is a column of cards.
- `section(ui, "ADD")` labels a group inside a card.
- `row(ui, icon, label, active, enabled)` is a full-width ghost row: the tool
  list, and anything else that reads as a list rather than a wall of buttons.
- `danger_row(ui, icon, label)` is the same in red.
- `tree_row(..)` adds indent guides and a muted kind note, for the design tree.
- `tool_button(ui, icon, label, active, enabled)` is compact, for bars.
- `icon_button(ui, icon, enabled)` is square and wordless.
- `primary_button(..)` and `primary_button_with_icon(..)` are the dark
  call-to-action. There is at most one visible at a time.
- `segmented(..)` is the workspace switch.
- `empty_state(ui, icon, title, hint)` fills a panel that has nothing in it.

## Icons

The set in `icon.rs` is drawn on a 24 by 24 grid with a 2px stroke, which is what
Lucide uses, so an icon borrowed from there sits with the rest without being
redrawn. Drawing them rather than shipping a font means they stay crisp at any
interface scale and take the current text colour, which the ghost rows depend on:
a disabled row dims its glyph and its label by the same amount.

Two rules keep the set legible at the 15px it is actually rendered at:

- Detail inside an outline merges with it. Concentric shapes need a gap of at
  least five grid units, and a feature smaller than four units should be a
  filled dot instead of an outline.
- `for_kind(kind)` maps a node kind to its glyph. Adding a node kind to the
  kernel means adding an arm there, and the match is exhaustive, so the compiler
  will say so.

`draw` dispatches to `profile_glyph`, `solid_glyph`, `action_glyph` and
`view_glyph`. The split is only to keep each match readable; which group an icon
lands in does not matter to a caller.

## Every control explains itself

A line icon does not teach anyone what a tool does, so every interactive thing in
the interface carries a tooltip, built with `theme::hint`:

```rust
let row = theme::row(ui, Icon::Shell, "Shell", false, has_selection);
if theme::hint(row, "Shell", help, None).clicked() {
    // ...
}
```

The tooltip is a bold title, a muted sentence or two, and an optional keyboard
shortcut. Three conventions:

- Say what it does to the model, in millimetres where a number is involved. Not
  "creates an extrude node".
- A disabled control explains *why* it is disabled and what to do about it. That
  is the tooltip a user needs most, and it is the reason disabled rows are
  allocated with `Sense::hover()` rather than skipped.
- Parameter fields are described by `describe_param`, keyed on the kernel's own
  parameter names. `every_parameter_is_described` walks one node of every kind
  and fails if any parameter has no description, so a new parameter cannot ship
  as a bare number.

## The context menu

Right click opens a menu on whatever is under the pointer. There are three, and
which one appears is decided by `MenuTarget`:

| Target | When | Offers |
|---|---|---|
| `Sketch` | A sketch is in progress | Finish and pad, remove the last point, cancel |
| `Node(id)` | The pick found a node | Sketch on its face, cut, shell, offset, move, frame, make root, delete |
| `Empty` | The pick found nothing | Sketch, add a body, fit, the standard views |

Three things make it behave:

- Opening a node menu **selects** that node first. The items act on the
  selection, so the menu and the property panel can never be acting on two
  different nodes.
- Right **drag** still pans. The two are told apart exactly as the left button
  tells a click from an orbit: by whether the pointer moved further than
  `CLICK_SLOP` between press and release.
- The press that dismisses a menu does nothing else. Without that, closing a
  menu also selects whatever was behind it.

The menu is state, not a widget. It lives on `AppState` because the click that
opens it arrives from winit, one layer below egui, and it has to survive until
the next frame draws it. Its rect is pushed into `Chrome::overlays` so a click
inside it is never also handled as a click on the model.

## Pointer ownership

egui reports its pointer as consumed whenever the pointer is merely *over* one of
its areas, and the viewport is itself a panel, so that flag cannot be used to
decide whether a click belongs to the 3D view. `ui::draw` returns a `Chrome` with
the viewport rect and every floating overlay rect, and `viewport_owns_pointer`
answers the question geometrically. Keyboard events still honour egui's flag.

Getting this wrong is not subtle: every click on the model is swallowed and the
application appears completely inert.

## Why a tooltip needs the event loop to cooperate

The viewport is static most of the time, so the event loop waits for input
rather than spinning at the refresh rate. That interacts badly with tooltips,
and it is worth understanding before touching `next_frame`.

egui will not show a tooltip until the pointer has rested on a widget for
`tooltip_delay`, and it only learns that the pointer is still by running frames
in which it did not move. Those frames are the ones it asks for with
`request_repaint_after`, so a delayed repaint request is not a hint that can be
dropped. Treating "repaint in 280ms" as "wait for input" deadlocks the thing:
the input never comes, because the pointer is deliberately not moving.

`next_frame` turns egui's request into a deadline and `about_to_wait` arms the
timer with `ControlFlow::WaitUntil`. `a_hinted_row_shows_its_tooltip` drives a
real `egui::Context` through that same rule and fails if a tooltip never
appears, which is what a regression here looks like.

## Resizing

The swapchain is reconciled with the window inside `redraw`, not when
`WindowEvent::Resized` arrives. Two reasons, both of which have bitten:

- Wayland does not guarantee that a resize is announced before the frame that
  needs it. A frame drawn at the old size is still presented at the new one,
  which leaves a smaller copy of the previous frame sitting in the window.
- egui reads its `screen_rect` straight from the window, so taking the surface
  size from an event instead means the chrome and the 3D view can be laid out
  at two different sizes in the same frame.

Reading both from `window.inner_size()` at the top of the frame removes both.
`surface_state` is the rule, and it is a plain function so it can be tested
without a window.

The other half is that a dropped frame has to ask for another one. The event
loop waits for input, so if `get_current_texture` fails and nothing requests a
redraw, the window keeps showing whatever it last had, which right after a
resize is the stale frame. `acquire` rebuilds the swapchain and requests the
next frame. The one exception is an occluded window, where nothing can be
presented and the request would spin.

## Checking your work

There is no display in CI, and often none in a development container either, so
the interface is reviewed by capture. See
[building.md](building.md#headless-capture) for the flags, including `--hover`,
which catches a tooltip in the frame.
