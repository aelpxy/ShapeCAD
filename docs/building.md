# Building and running

## Toolchain

Rust 1.98.1, pinned in `rust-toolchain.toml`. `rustup` will install it on first
build.

You also need a C linker (`cc`) even though the project contains no C: Rust
invokes one to link every binary. On Arch that is `gcc`; on Debian and Ubuntu,
`build-essential`.

```sh
cargo test --workspace           # everything
cargo run --release -p sc-app    # the application
```

Use `--release` for anything that renders or meshes. The debug build is roughly
20× slower and the viewport will feel broken rather than slow.

## System dependencies for the viewport

`sc-app` and `sc-render` need a Vulkan or OpenGL driver and a window system.
Nothing else in the workspace does: `sc-geom`, `sc-doc`, `sc-mesh` and `sc-cli`
build and test on a machine with no GPU at all.

On Arch:

```sh
pacman -S vulkan-icd-loader mesa wayland libxkbcommon
pacman -S vulkan-radeon   # or vulkan-intel, nvidia-utils, vulkan-dzn on WSL
pacman -S vulkan-swrast   # llvmpipe: software fallback, and what the tests use
```

Fonts are vendored (`crates/sc-app/assets/fonts`), so no system font is required.

## Running under WSL

WSL works, with three traps worth knowing about.

**Mesa's Dozen driver is hidden by default.** It implements Vulkan over D3D12 and
is the only hardware path under WSL, but it reports itself as non-conformant and
wgpu filters such adapters out. Without
`InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER` the application silently
falls back to llvmpipe, which is about a hundred times slower for a sphere-traced
viewport. `sc_render::gpu::instance` sets the flag.

**Dozen crashes when driven from a non-main thread.** Enumerating Vulkan adapters
loads it, and dropping one of its adapters off the main thread segfaults. Rust's
test harness runs every test on a spawned thread, so
`gpu::Preference::Software` restricts itself to the GL backend and never loads a
Vulkan driver at all. This is why the GPU smoke test asks for software rendering
rather than just selecting a software adapter.

**`current_monitor()` returns `None`.** Wayland frequently cannot name a monitor
until the surface is mapped, so display-size detection falls through to
`primary_monitor()` and then to the widest of `available_monitors()`, and retries
once on the first resize. Without this the interface renders at 1× on a 4K panel,
which is half the size it should be.

Check what the machine actually offers:

```sh
cargo run -p sc-render --example adapters
```

## Interface scale

The application measures the display and picks a zoom, because many Linux
compositors report a scale factor of 1.0 on a high-density panel. Override it
with `Ctrl` `+` / `-` / `0`; the choice is saved to
`$XDG_CONFIG_HOME/shapecad/settings.json` and reused.

## Headless capture

The whole interface renders without a window, which is how it is reviewed in CI
and how changes are checked on a machine with no display:

```sh
cargo run --release -p sc-app -- --snapshot ui.png --width 2400 --height 1500 --scale 1.5
```

By default the capture shows a new, empty document. Three flags pick a different
scene:

| Flag | What it captures |
|---|---|
| `--sample` | The bundled bracket, with its root selected, so the design tree and the property panel are populated. |
| `--dialog` | The file browser open over an empty document. |
| `--hover` | The pointer resting on a tool row, so its tooltip is in the frame. |
| `--menu` | The context menu open on the sample model's root. |

Two details matter if you touch that path. `RawInput::screen_rect` is in *points*,
not pixels, so it must be divided by the scale. And several frames must be run:
egui gives a newly created area a sizing pass before it can place itself, and
fade-in animations need time to advance, so a modal captured in one frame is
invisible and in two is half transparent. Texture deltas must be applied from
*every* frame. The font atlas is created during the first, and dropping that
delta leaves the renderer with no atlas, at which point it silently skips the
entire interface.

Capturing a tooltip needs one more thing. egui measures the tooltip delay from
the last pointer movement, so the pointer is moved on the first pass and then
left alone; repeating the move event every pass resets the timer and no tooltip
ever appears.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `linker 'cc' not found` | No C toolchain. Install `gcc`. |
| Viewport is slow, `adapters` shows only llvmpipe | No hardware Vulkan driver installed. |
| Interface is tiny on a high-density display | Auto-detection failed; press `Ctrl` `+`. |
| Segfault in a GPU test under WSL | A Vulkan driver was loaded off the main thread; use `Preference::Software`. |
