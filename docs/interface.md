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

## The guided tour

`tutorial.rs`. Five steps, shown on a card in the corner of the viewport, each
advancing when the user actually does the thing rather than when they press Next.
That difference is the whole point: a slideshow can be clicked through without
anything being learned, and it cannot tell whether it worked.

The cost is a condition per step, and conditions are what rots. A step that can
never be satisfied traps a beginner in the one part of the application they
cannot leave, and nothing else in the suite would notice, so every step is driven
through a real `AppState` in `doing_what_each_step_asks_finishes_the_tutorial`.

Steps measure **change**, not state. A `Mark` is taken when a step begins and the
condition compares against it, so a document that already has something in it
does not tick off "add a solid" before anything has been added.

That test earned its keep immediately. There was a sixth step, "select it", and
adding a solid selects it already, so the step was satisfied before the user had
clicked anything: a card that ticks itself off, teaching nothing while looking
like it taught something. Selection is explained on the cards either side instead.

It runs unasked on a first launch and sets `tutorial_seen`. Putting it behind a
menu item means the people who need it most are the least likely to find it, and
it costs one click to dismiss. The Guide button in the top bar brings it back.

## The design tree

Two rules, both learned from the engine example, which has eleven holes in it.

**A boolean chain is a list, not a staircase.** A boolean whose first operand is
another of the same kind is one more step in the same chain, so it is drawn at
the same depth rather than one further in. Without that the engine reached depth
26 and marched off the side of the panel. The operand is drawn first and the rest
of the chain after, so reading down gives the newest cut, the feature it cut
with, the one before it, and the body at the bottom.

**A node is called what it is for.** `label_for` decides, in one place, because
the tree and the property panel were each working it out and could disagree. An
unnamed boolean is titled by what it did and subtitled by the feature it did it
with, since "Difference" eleven times says nothing about a part with eleven
holes, and the row below it is that feature. A bare placement is "Position"
rather than "Transform".

That makes naming features matter, so the samples name theirs. An unnamed cut
falls back to the kind and the panel goes quiet again.

## Positioning

A selected feature gets a move gizmo: three arms along the world axes, coloured
to match the axis legend, with a grab ring on each. Dragging an arm moves along
that axis and nothing else. Dragging the feature itself still slides it freely
across a plane, and **X**, **Y** or **Z** locks that drag to an axis part way
through, with the same key again letting it go.

The arm is the point. A plane drag has two degrees of freedom and a pointer has
two, so every free move changes two coordinates whether or not that was wanted.
Locking is how you move something ten millimetres to the right and nowhere else.

Four things that matter in the implementation, each a test:

**The plane follows the constraint.** A locked drag is measured against a plane
that *contains* the axis and faces the camera as squarely as it can, not against
the view-facing plane. Projecting onto an axis that points away from the camera
is ill conditioned, and a pixel of pointer travel would be worth metres.

**Locking re-anchors.** Changing the constraint changes the plane the pointer is
measured against, so the old anchor is a point on a plane that no longer exists.
The drag marks itself for re-anchoring and takes the next sample as its new
origin, measured from where the feature is *now*. Without that, pressing X part
way through flings the feature back along the path it has already travelled.
That also keeps the key handling where the keys are, since it needs no pointer
position.

**The gizmo is sized from the camera**, so it stays the same length on screen
however far away the part is. One that scales with the model is unusable on a
large part and swallows a small one.

**Zero has a wider catchment than the grid.** A feature on an axis, or centred
on the plate, is something people deliberately want and then verify by reading
the number back, so landing on 0.4 when aiming at 0 is a worse answer than the
grid spacing alone suggests. Everything else rounds normally.

## Precision

A grid is the floor of precision, not the whole of it. Most of the numbers
somebody wants are not round ones: they are the number that puts this boss on
the same centreline as that one, or this wall flush with that face. Those are
relationships between features, and a grid cannot express any of them.

**Snapping to features.** A drag is offered coordinates by every other feature
in the model, three per axis: the two faces of its bounding box and its centre.
Any of the moving feature's own three can land on any of theirs, which is what
makes both "line these up" and "make these flush" the same gesture. The nearest
candidate within reach wins, and the grid is what happens when none is.

`snap.rs` is pure arithmetic on one axis at a time, which is what lets it be
tested without a document, a camera or a pointer, and what makes it compose with
axis locking: a locked drag simply has nothing to contribute on the two axes it
cannot move along.

Four things the implementation has to get right:

**The pull is a screen distance.** A snap you have to fight at one zoom and
cannot escape at another is worse than no snap. Eight points, converted through
the camera, and clamped against the grid at both ends so that zooming right out
does not have parts snapping to things on the far side of the plate.

**Only leaf features offer coordinates.** A boolean's bounding box is the box
around both of its operands, a number nobody drew, and the root's is the whole
model. Lining up with a hole, a pad or a boss is what people mean.

**A feature is never offered its own coordinates.** It moves with the drag, so it
would offer wherever it already is, and the part would refuse to leave the spot
it started from.

**A snap that cannot be seen reads as the part sticking.** Each latched axis
draws a line, tinted like that axis, from the feature the coordinate came from to
the feature being dragged, and the readout names what matched: `X centre to
centre`, not `snapped`. Knowing which is what lets you tell a wanted alignment
from an accident.

**Typing an exact number.** Dragging is how you find a size; typing is how you
state one. Chasing 12.00 with a pointer is a game, and losing it means dragging
until the readout happens to agree, which usually ends at 11.98. So a gesture
already in flight takes digits: Enter applies, Backspace edits, Escape drops the
number and leaves the drag alone.

What the number means depends on what is being dragged, because that is what the
word means in each case. A dimension takes it as the dimension: twelve means a
radius of twelve. A move takes it as a distance along the locked axis: twelve
means twelve millimetres that way. The readout says which reading is in force
while it is still being typed, so there is nothing to guess.

Three more decisions:

**A typed number is not snapped.** It is already the number that was meant, and
rounding 12.5 to a 1mm grid afterwards would produce 13 and say nothing.

**A typed number needs a direction, and a long drag has already given one.**
Locking an axis is the explicit way to say which way; having dragged a hand's
width to the right says it too, and demanding X as well would be pedantry.
Having not moved at all says nothing, and that is refused rather than guessed.

**A bad typed value is refused, not clamped.** A drag stops at the limit because
it is a continuous gesture passing through, and an error on every frame would be
noise. Typing is a statement, and quietly applying a different number is worse
than saying no.

## Direct manipulation

A selected feature shows a grip on each of its dimensions: a dot sitting on the
surface that dimension moves, with a stub pointing the way it grows once the
pointer is near enough to grab it. Drag one and the geometry follows, with the
value read out beside it.

Three modules, split along the only line that matters, which is what has a right
answer and what does not:

- `handle.rs` says where a dimension lives, in the node's own frame. A grip in
  the wrong place is a bug that can be written down as a failing test.
- `AppState::grips` lifts those into the world through the node's placement, and
  `grip_on_screen` projects one. **Drawing and hit testing share that one
  function on purpose.** Two projections would drift, and the symptom would be
  grips that cannot be grabbed where they appear.
- `ui::grips` draws them; `App::grab_grip` starts the drag.

Not every parameter gets one. A rounding radius, a blend radius, an offset
distance and a shell thickness have no direction: the surface moves everywhere at
once, so there is nowhere honest to put a grip. Those stay in the property panel,
which is the right place for a scalar with no axis. A transform's translation is
left out too, because moving a feature is a different gesture from resizing one
and a mis-grab should not change the wrong thing.

Four things the implementation has to get right, each of which is a test:

**Gain.** A half-extent is measured from the centre and a width across, so
dragging a rectangle's edge one millimetre makes it two millimetres wider. Get
this wrong and the geometry moves at half the speed of the pointer.

**Foreshortening.** A grip pointing nearly at the camera covers almost no screen
distance, so a pixel of travel would be worth metres. `grip_on_screen` refuses
one below a threshold; orbit slightly and it becomes grabbable. The screen scale
comes from projecting the grip and a point one unit along it, which picks up
perspective and any scale in the placement for free.

**One drag, one undo.** A drag makes hundreds of parameter changes. It opens an
undo step on the press and closes it on release, so all of them come back
together. Every path out of a drag closes it, including Escape.

**Stopping rather than erroring.** A drag that would take a radius through zero
stops at the limit. Refusing it would put an error on the status bar on every
frame, which is noise, and the geometry would stop responding with no
explanation.

## Copies and patterns

Two rows in MODIFY, answering two different questions.

**Duplicate** (**Ctrl+D**) copies the selected feature and its whole subtree,
lands the copy a few grid steps to the right and selects it, so the next gesture
is dragging it where it belongs. The copy is independent. Nothing links it back
to the original, because someone reaching for Duplicate usually wants a starting
point rather than a clone that follows its source forever.

**Repeat** and **Repeat around** wrap the selection in a pattern node. That node
holds the count, so going from four to six is one number in the property panel,
not two more gestures. The child stays a single node in the tree, which means
editing the original edits every instance: change the boss radius and all six
bosses change together.

Three choices worth writing down:

**The step is measured, not fixed.** A linear repeat spaces instances off the
selection's own bounds. A fixed step buries the copies inside a large feature and
scatters them across the room for a small one, and either way the first thing the
user has to do is fix a number that should not have been wrong.

**Instance zero is the identity.** Applying a pattern never moves what was
already there, so the gesture reads as adding copies rather than as rearranging
the model.

**A full turn divides by the count, a partial sweep by the count minus one.** Six
holes around a full circle sit every sixty degrees, with nothing doubled up at
the seam. Three holes across ninety degrees sit at nought, forty five and ninety,
because an arc of holes is meant to reach the far end.

Counts are shown as whole numbers and sweeps in degrees. A field reading `6.00
mm` for a copy count invites someone to try five and a half.

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
