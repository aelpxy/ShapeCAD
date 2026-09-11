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

A single neutral ramp, one accent, one red. Nothing else.

| Constant | Used for |
|---|---|
| `CANVAS` | The space between cards |
| `SURFACE` | A card, a popup, a floating bar |
| `SURFACE_ALT` | A hovered or active row, an input field |
| `BORDER` | The hairline that separates a card from the canvas |
| `TEXT` / `TEXT_DIM` | Primary and secondary text |
| `ACCENT` / `ACCENT_SOFT` | Selection, and the tint behind it |
| `DANGER` | Destructive actions only |
| `INK` | The single primary button, and the application mark |

Depth comes from the border, not from shadows. Only things that genuinely float
over the 3D view get one, and even then it is faint.

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
