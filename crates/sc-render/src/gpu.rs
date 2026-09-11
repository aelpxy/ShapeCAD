//! Adapter and device selection.
//!
//! Shared by the windowed viewport and the headless snapshot path so both make
//! the same choice; a viewer on the GPU and a snapshot on a software rasteriser
//! would not be comparable.

/// Creates an instance able to reach every backend.
///
/// Enables non-conformant adapters. On WSL the only hardware path is Mesa's
/// Dozen driver (Vulkan over D3D12), which self-reports as non-conformant, so
/// wgpu hides it by default, leaving nothing but a software rasteriser. That is
/// a 100x performance difference for a sphere-traced viewport.
#[must_use]
pub fn instance() -> wgpu::Instance {
    instance_for(Preference::Hardware)
}

/// Creates an instance that can only reach what `preference` asks for.
///
/// The restriction is the point, not an optimisation. `wgpu::Instance::new`
/// builds a driver instance for every backend it is given, there and then, so
/// an instance over all backends has already loaded the Vulkan ICD before
/// anything is enumerated. Filtering adapters afterwards is far too late: see
/// [`Preference::Software`] for what that costs under WSL.
#[must_use]
pub fn instance_for(preference: Preference) -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends(preference);
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
    /// This deliberately restricts itself to the GL backend, and the
    /// restriction has to reach the instance rather than only the adapter
    /// filter: see [`instance_for`]. On WSL, merely creating a Vulkan instance
    /// loads Mesa's Dozen driver, which then segfaults when its adapter is
    /// dropped from a thread other than main, and Rust's test harness runs
    /// every test on a spawned thread. Selecting a software adapter is not
    /// enough; the Vulkan ICD must never be loaded at all.
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

/// A device on the software rasteriser for tests, or `None` where the machine
/// has no GPU at all.
///
/// Returning `None` lets a test skip rather than fail on a build machine
/// without even llvmpipe. The preference is not negotiable here: the harness
/// runs every test on a spawned thread, which is exactly where a Vulkan driver
/// must not be.
#[cfg(test)]
pub(crate) fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = instance_for(Preference::Software);
    let adapter = try_adapter(&instance, None, Preference::Software)?;
    Some(device(&adapter))
}

#[cfg(test)]
mod tests {
    use super::{instance_for, rank, Preference};

    #[test]
    fn the_software_instance_cannot_reach_a_vulkan_driver() {
        // Filtering adapters is not enough. `Instance::new` builds a driver
        // instance for every backend it is handed, there and then, so an
        // instance over all of them has loaded the Vulkan ICD before a single
        // adapter has been looked at. Under WSL that ICD is Dozen, which
        // segfaults when its adapter is dropped from a thread other than main,
        // and this test is running on one.
        let instance = instance_for(Preference::Software);
        for adapter in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
            let info = adapter.get_info();
            assert_eq!(
                info.backend,
                wgpu::Backend::Gl,
                "the software instance reached {} over {:?}",
                info.name,
                info.backend
            );
        }
    }

    #[test]
    fn the_software_preference_reverses_the_adapter_ranking() {
        let mut ranked = [
            wgpu::DeviceType::DiscreteGpu,
            wgpu::DeviceType::IntegratedGpu,
            wgpu::DeviceType::Cpu,
        ];
        ranked.sort_by_key(|&t| rank(t, Preference::Software));
        assert_eq!(ranked[0], wgpu::DeviceType::Cpu);
        ranked.sort_by_key(|&t| rank(t, Preference::Hardware));
        assert_eq!(ranked[0], wgpu::DeviceType::DiscreteGpu);
    }
}
