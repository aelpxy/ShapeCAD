//! The sphere-tracing shader, assembled around a generated field function.
//!
//! [`sc_geom::wgsl::generate`] emits `fn sc_sdf(p: vec3<f32>) -> f32`; this
//! module supplies the camera, the ray march and the shading that turn it into
//! an image. Concatenating rather than templating keeps the generated half
//! independently testable.

/// Maximum sphere-tracing steps per pixel.
///
/// Also doubles as the denominator for the cheap ambient-occlusion term: a ray
/// that needed many steps was grazing geometry, which is exactly where contact
/// shadowing belongs.
pub const MAX_STEPS: u32 = 192;

/// Returns a complete WGSL module: the generated field plus the renderer.
#[must_use]
pub fn compose(field_wgsl: &str) -> String {
    // WGSL has no preprocessor, so the step budget is substituted here rather
    // than passed as a uniform: it bounds a loop, and a constant bound lets the
    // shader compiler unroll and register-allocate properly.
    format!(
        "{field_wgsl}\n{}",
        RENDER.replace("MAX_STEPS_CONST", &format!("{MAX_STEPS}u"))
    )
}

const RENDER: &str = r"
struct ScCamera {
    // xyz = eye position, w = tan(fov_y / 2)
    eye: vec4<f32>,
    // xyz = right vector, w = aspect ratio
    right: vec4<f32>,
    // xyz = up vector, w = far distance
    up: vec4<f32>,
    // xyz = forward vector, w = angular size of one pixel in radians
    forward: vec4<f32>,
    // Scene colours, so the viewport follows the interface palette rather than
    // keeping a second one of its own. xyz each; w unused.
    sky: vec4<f32>,
    haze: vec4<f32>,
    plate: vec4<f32>,
    grid: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: ScCamera;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

// A single oversized triangle covering the viewport. Cheaper than a quad and
// free of the diagonal seam two triangles produce under interpolation.
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let c = corners[idx];
    var out: VertexOut;
    out.clip = vec4<f32>(c, 0.0, 1.0);
    out.ndc = c;
    return out;
}

// Absolute floor on tracing tolerances, in model units (millimetres). Well below
// any printer's resolution, but large enough to keep the march from stalling on
// float noise near the camera.
const MIN_EPS: f32 = 1.0e-4;

// Direction toward the key light. Fixed in world space rather than to the
// camera, so orbiting reveals form instead of sliding a highlight around.
const KEY_DIR: vec3<f32> = vec3<f32>(0.3906, -0.5642, 0.7276);
const FILL_DIR: vec3<f32> = vec3<f32>(-0.8452, 0.4226, 0.3300);

fn surface_tolerance(t: f32, px_angle: f32) -> f32 {
    return max(t * px_angle * 0.5, MIN_EPS);
}

fn sc_normal(p: vec3<f32>, eps: f32) -> vec3<f32> {
    let k = vec2<f32>(1.0, -1.0);
    return normalize(
        k.xyy * sc_sdf(p + k.xyy * eps) +
        k.yyx * sc_sdf(p + k.yyx * eps) +
        k.yxy * sc_sdf(p + k.yxy * eps) +
        k.xxx * sc_sdf(p + k.xxx * eps)
    );
}

// Penumbra estimated from how closely the shadow ray passes the surface. Much
// cheaper than sampling an area light and, for a single key, indistinguishable.
fn soft_shadow(origin: vec3<f32>, dir: vec3<f32>, far: f32, bias: f32) -> f32 {
    var shade = 1.0;
    var t = bias;
    for (var i = 0; i < 48; i = i + 1) {
        let h = sc_sdf(origin + dir * t);
        if (h < 1.0e-3) {
            return 0.0;
        }
        shade = min(shade, 10.0 * h / t);
        t = t + clamp(h, bias, far * 0.05);
        if (t > far * 0.5) {
            break;
        }
    }
    return clamp(shade, 0.0, 1.0);
}

fn background(ndc: vec2<f32>) -> vec3<f32> {
    // A soft studio sweep: brighter toward the horizon, cooler above.
    let v = ndc.y * 0.5 + 0.5;
    return mix(cam.haze.xyz, cam.sky.xyz, smoothstep(0.0, 1.0, v));
}

// One axis of grid lines, antialiased by the screen-space derivative so the
// floor does not shimmer as the camera moves.
fn grid_line(coord: vec2<f32>, spacing: f32) -> f32 {
    let c = coord / spacing;
    let d = abs(fract(c - 0.5) - 0.5) / fwidth(c);
    return 1.0 - min(min(d.x, d.y), 1.0);
}

// The build plate: a ground plane at z = 0, which is where a printed part sits.
fn ground(p: vec3<f32>, dist: f32, far: f32) -> vec3<f32> {
    let minor = grid_line(p.xy, 5.0) * 0.35;
    let major = grid_line(p.xy, 25.0) * 0.55;
    let axis_x = 1.0 - min(abs(p.y) / max(fwidth(p.y), 1.0e-6), 1.0);
    let axis_y = 1.0 - min(abs(p.x) / max(fwidth(p.x), 1.0e-6), 1.0);

    var base = cam.plate.xyz;
    base = mix(base, cam.grid.xyz, max(minor, major));
    base = mix(base, vec3<f32>(0.78, 0.36, 0.36), axis_x * 0.8);
    base = mix(base, vec3<f32>(0.40, 0.62, 0.40), axis_y * 0.8);

    let shadow = soft_shadow(p, KEY_DIR, far, max(dist * 0.004, 0.02));
    base = base * (0.55 + 0.45 * shadow);

    // Fade into the background rather than tiling to the horizon.
    let fade = clamp(dist / (far * 0.35), 0.0, 1.0);
    return mix(base, background(vec2<f32>(0.0, -0.2)), fade * fade);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let tan_half = cam.eye.w;
    let aspect   = cam.right.w;
    let far      = cam.up.w;
    let px_angle = cam.forward.w;

    let dir = normalize(
        cam.forward.xyz
        + cam.right.xyz * in.ndc.x * tan_half * aspect
        + cam.up.xyz    * in.ndc.y * tan_half
    );
    let ro = cam.eye.xyz;

    var t = 0.0;
    var steps = 0u;
    var hit = false;

    for (var i = 0u; i < MAX_STEPS_CONST; i = i + 1u) {
        let d = sc_sdf(ro + dir * t);
        steps = i;
        // Stop once the ray is within half a pixel of the surface. The footprint
        // grows with distance, so distant geometry costs fewer steps without any
        // visible loss - but the tolerance tracks the *pixel*, not the distance.
        if (d < surface_tolerance(t, px_angle)) {
            hit = true;
            break;
        }
        t = t + d;
        if (t > far) {
            break;
        }
    }

    if (!hit) {
        // Missed the model: fall through to the build plate, then the sky.
        if (dir.z < -1.0e-4) {
            let tg = -ro.z / dir.z;
            if (tg > 0.0 && tg < far) {
                return vec4<f32>(ground(ro + dir * tg, tg, far), 1.0);
            }
        }
        return vec4<f32>(background(in.ndc), 1.0);
    }

    let p = ro + dir * t;
    // Sample the gradient over roughly one pixel. Any wider and genuine edges
    // get averaged into curves.
    let n = sc_normal(p, max(t * px_angle, MIN_EPS));
    let view = -dir;

    // Is this point also on the surface of the selected subtree? A slightly
    // looser tolerance than the primary hit, so the highlight does not break up
    // where the two fields disagree in the last decimal.
    let on_selection = sc_selected(p) < max(t * px_angle * 3.0, MIN_EPS * 4.0);

    // Skip the shadow march entirely where the surface already faces away from
    // the key; it can only return zero there, and marching from a grazing angle
    // is exactly where self-intersection artefacts come from.
    let facing = dot(n, KEY_DIR);
    var key = 0.0;
    if (facing > 0.01) {
        let bias = max(t * px_angle * 6.0, 0.02);
        key = facing * soft_shadow(p + n * bias, KEY_DIR, far, bias);
    }
    let fill = max(dot(n, FILL_DIR), 0.0) * 0.32;
    // Hemispheric ambient, brighter from above, matching the build-plate reading
    // of the scene.
    let amb  = 0.34 + 0.20 * (n.z * 0.5 + 0.5);

    // Rays that needed many steps were grazing geometry, which approximates
    // occlusion for free.
    let ao = 1.0 - (f32(steps) / f32(MAX_STEPS_CONST)) * 0.45;

    let h = normalize(KEY_DIR + view);
    let spec = pow(max(dot(n, h), 0.0), 64.0) * 0.45;
    let rim  = pow(1.0 - max(dot(n, view), 0.0), 4.0) * 0.12;

    // A neutral machined-aluminium grey, slightly cool in shadow; the selection
    // shifts toward the interface accent so the two read as the same idea.
    let neutral = vec3<f32>(0.470, 0.487, 0.515);
    let accent = vec3<f32>(0.055, 0.185, 0.720);
    let base = select(neutral, mix(neutral, accent, 0.72), on_selection);
    let lit = base * (key * 0.62 + fill + amb) * ao
            + vec3<f32>(spec)
            + vec3<f32>(0.72, 0.78, 0.88) * rim;

    // Output is linear; the surface is an sRGB format so the hardware handles
    // the transfer function.
    return vec4<f32>(lit, 1.0);
}
";
