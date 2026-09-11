//! Lists every GPU adapter wgpu can see, with the device type it reports.

fn main() {
    let instance = sc_render::gpu::instance();
    for a in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
        let i = a.get_info();
        println!(
            "{:?}  {:?}  {}  driver={}",
            i.backend, i.device_type, i.name, i.driver
        );
    }
}
