//! Adapter and device selection.
//!
//! Shared by the windowed viewport and the headless snapshot path so both make
//! the same choice; a viewer on the GPU and a snapshot on a software rasteriser
//! would not be comparable.

/// Creates an instance configured for the platforms we care about.
///
/// Enables non-conformant adapters. On WSL the only hardware path is Mesa's
/// Dozen driver (Vulkan over D3D12), which self-reports as non-conformant, so
/// wgpu hides it by default — leaving nothing but a software rasteriser. That is
/// a 100x performance difference for a sphere-traced viewport.
#[must_use]
pub fn instance() -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.flags |= wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER;
    wgpu::Instance::new(descriptor)
}

/// Which kind of adapter to favour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    /// Fastest available. What the viewport wants.
    #[default]
    Hardware,
    /// Prefer a software rasteriser, and reach it without loading any Vulkan
    /// driver.
    ///
    /// Slower, but identical on every machine, which is what golden-image
    /// comparison needs.
    ///
    /// This deliberately restricts itself to the GL backend. On WSL, merely
    /// enumerating Vulkan adapters loads Mesa's Dozen driver, which then
    /// segfaults when its adapter is dropped from a thread other than main —
    /// and Rust's test harness runs every test on a spawned thread. Selecting a
    /// software adapter is not enough; the Vulkan ICD must never be loaded at
    /// all.
    Software,
}

/// Which backends to enumerate for a given preference.
fn backends(preference: Preference) -> wgpu::Backends {
    match preference {
        Preference::Hardware => wgpu::Backends::all(),
        Preference::Software => wgpu::Backends::GL,
    }
}

/// Ranks adapters, lowest first.
fn rank(device_type: wgpu::DeviceType, preference: Preference) -> u8 {
    let hardware = match device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        // Correct, and far too slow to be interactive. Last resort.
        wgpu::DeviceType::Cpu => 4,
    };
    match preference {
        Preference::Hardware => hardware,
        Preference::Software => 4 - hardware,
    }
}

/// Picks the best available adapter, optionally one able to present to `surface`.
///
/// Returns `None` when the machine has no usable GPU at all, which lets tests
/// skip rather than fail on a headless build machine.
#[must_use]
pub fn try_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
    preference: Preference,
) -> Option<wgpu::Adapter> {
    let all = pollster::block_on(instance.enumerate_adapters(backends(preference)));
    let mut usable: Vec<wgpu::Adapter> = all
        .into_iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .collect();

    if usable.is_empty() {
        return None;
    }
    usable.sort_by_key(|a| rank(a.get_info().device_type, preference));
    Some(usable.remove(0))
}

/// Picks the best available adapter.
///
/// # Panics
/// If no adapter is available at all.
#[must_use]
pub fn adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
    preference: Preference,
) -> wgpu::Adapter {
    try_adapter(instance, surface, preference).expect("no usable GPU adapter found")
}

/// Acquires a device and queue with default limits.
///
/// # Panics
/// If the adapter refuses to create a device.
#[must_use]
pub fn device(adapter: &wgpu::Adapter) -> (wgpu::Device, wgpu::Queue) {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("shapecad"),
        ..Default::default()
    }))
    .expect("could not acquire a device")
}
