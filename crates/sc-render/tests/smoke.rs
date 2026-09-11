//! End-to-end check that the GPU path produces an image.
//!
//! Skips rather than fails where there is no GPU, so a headless build machine
//! without even a software rasteriser does not turn the suite red. Where a GPU
//! does exist — including llvmpipe — this exercises shader compilation, the
//! pipeline, the render pass and the readback in one go.

use sc_doc::samples::bracket;
use sc_render::gpu::Preference;
use sc_render::{snapshot, OrbitCamera};

#[test]
fn the_reference_part_renders() {
    let doc = bracket();
    let camera = OrbitCamera::framing(doc.bounds().expect("sample is rooted"));
    let Some(image) = snapshot::try_render(&doc.wgsl(), &camera, 320, 200, Preference::Software)
    else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };

    assert_eq!(image.pixels.len(), 320 * 200 * 4);

    // A corner is background; the centre is the part. If those match, either
    // nothing was drawn or the camera is not looking at the model.
    let px = |x: usize, y: usize| {
        let i = (y * 320 + x) * 4;
        [image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]]
    };
    let corner = px(4, 4);
    let centre = px(160, 100);
    assert_ne!(corner, centre, "nothing was rendered");

    // Deliberately not asserting which of the two is brighter. That depends on
    // the palette, and an earlier version of this test hard-coded "the part is
    // pale, the background is dark" and then silently failed for several
    // sessions once the viewport moved to a light theme.
    //
    // What actually matters is that shading ran: the part is clearly separated
    // from the background, and is neither black nor the background colour.
    let brightness = |c: [u8; 3]| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]);
    let separation = brightness(centre).abs_diff(brightness(corner));
    assert!(
        separation > 60,
        "part {centre:?} and background {corner:?} are too close to tell apart"
    );
    assert!(
        brightness(centre) > 60,
        "the part rendered essentially black: {centre:?}"
    );
}
