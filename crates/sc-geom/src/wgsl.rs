//! WGSL code generation for GPU evaluation of the field.
//!
//! The viewport sphere-traces this directly; there is no tessellation in the
//! display path at all. Meshing only happens on export.
//!
//! Every number from the model is bound into a storage buffer rather than
//! written into the source, so editing a radius or dragging a sketch point is an
//! upload and only a change of topology recompiles. What remains in the source
//! is structure: which nodes there are, the loop bound of a profile, and the
//! peepholes that decide whether a term exists at all.

use crate::arena::Arena;
use crate::node::{Node, NodeId};
use std::collections::HashMap;
use std::fmt::Write as _;

const PRELUDE: &str = r"
@group(0) @binding(1) var<storage, read> sc_params: array<f32>;

// Empty space. Named rather than written as a bare literal so that a node
// emitted as nothing is greppable in the output and countable in a test: the
// one failure this backend must never have is dropping a node quietly. Large
// enough that a trace step from it leaves any scene, finite so that a `max`
// against it cannot produce a NaN.
const SC_EMPTY: f32 = 1e30;

// WGSL has no isinf or isnan, so the empty-space sentinel doubles as the
// threshold: an infinity fails this, and so does a NaN, because every
// comparison against one is false. No real distance in a printable part comes
// near 1e30.
fn sc_finite(x: f32) -> bool {
    return abs(x) < SC_EMPTY;
}

fn sc_smin(a: f32, b: f32, k: f32) -> f32 {
    // Mirrors the guard in `eval::smin`. The interpolation multiplies an
    // operand by a weight that saturates at zero, so an infinite operand gives
    // `inf * 0`, which is NaN rather than the other operand. Empty space is
    // exactly such an operand, so without this one missing feature blanks the
    // whole model on the GPU while the CPU renders it correctly.
    if (k <= 0.0 || !sc_finite(a) || !sc_finite(b)) {
        return min(a, b);
    }
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

fn sc_smax(a: f32, b: f32, k: f32) -> f32 {
    return -sc_smin(-a, -b, k);
}

fn sc_qrot(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}
";

/// A generated shader and the values it reads.
///
/// No value from the model appears in `source`; every one is an index into
/// `params`. That gives callers a free rebuild test: if `source` is unchanged,
/// only values moved and the buffer can simply be re-uploaded. If it changed,
/// the topology did (or a peephole stopped applying), and the pipeline has to be
/// rebuilt.
///
/// The one number that is genuinely structure is a profile's vertex count,
/// which is the loop bound over the buffer and so cannot itself live in it.
/// Adding a point to a sketch rebuilds; moving one does not.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    /// WGSL exposing `sc_sdf` and `sc_selected`.
    pub source: String,
    /// Values for the `sc_params` storage buffer, in binding order.
    pub params: Vec<f32>,
    /// Nodes the shader does not represent, in the order they were reached.
    ///
    /// **A non-empty list means `source` is not the model.** Those nodes emit
    /// empty space, so anything that displays the shader without checking this
    /// shows a part with a piece missing and no indication that it is missing.
    /// Check it and refuse to display, or fall back to the CPU field.
    ///
    /// Today this is only [`Node::Mesh`]: a voxel grid is megabytes of samples
    /// and belongs in a texture the shader reads, not in generated source text,
    /// and that GPU path is not built yet. [`generate`] cannot return an error
    /// because the viewport regenerates on every structural edit and has nowhere
    /// to put one, so the fact is reported alongside the source instead of being
    /// swallowed. [`try_generate`] is the same thing as a `Result` for callers
    /// that can refuse.
    pub unsupported: Vec<NodeId>,
}

impl Generated {
    /// Whether the shader faithfully represents the whole model.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unsupported.is_empty()
    }
}

/// Emits a module exposing `sc_sdf` and `sc_selected`, plus the values they read.
///
/// An absent or dead root yields a field that is empty everywhere, so the
/// viewport shows nothing rather than failing to compile.
#[must_use]
pub fn generate(arena: &Arena, root: Option<NodeId>) -> Generated {
    // `sc_selected` is emitted even with nothing selected: the renderer's shader
    // always calls it, and one empty function is cheaper than two shader
    // variants.
    compose(arena, &[("sc_sdf", root), ("sc_selected", None)])
}

/// As [`generate`], but refuses a model the shader cannot represent.
///
/// # Errors
/// [`GeomError::NotInShader`] naming the first offending node, so a caller that
/// has somewhere to report an error is not left inspecting
/// [`Generated::unsupported`] by hand.
pub fn try_generate(arena: &Arena, root: Option<NodeId>) -> crate::Result<Generated> {
    let generated = generate(arena, root);
    match generated.unsupported.first() {
        None => Ok(generated),
        Some(&node) => Err(crate::GeomError::NotInShader {
            node,
            kind: arena.get(node).map_or("dead", Node::kind),
        }),
    }
}

/// As [`generate`], plus a second field covering only the selected subtree.
///
/// The viewport uses it to tint whatever is selected: at a surface hit it asks
/// whether that point also lies on the selection's own surface. One extra field
/// evaluation per visible pixel, and no separate pass or picking buffer.
///
/// `sc_selected` is always emitted, returning empty space when nothing is
/// selected, so the shader never has to be regenerated with a different shape.
#[must_use]
pub fn generate_with_selection(
    arena: &Arena,
    root: Option<NodeId>,
    selected: Option<NodeId>,
) -> Generated {
    compose(arena, &[("sc_sdf", root), ("sc_selected", selected)])
}

/// Emits a module containing one function per requested field.
///
/// Helper functions are shared across them, so two fields referring to the same
/// extruded profile emit its polygon once.
fn compose(arena: &Arena, fields: &[(&str, Option<NodeId>)]) -> Generated {
    let mut e = Emitter {
        arena,
        body: String::new(),
        helpers: String::new(),
        counter: 0,
        memo: HashMap::new(),
        helper_names: HashMap::new(),
        params: Vec::new(),
        unsupported: Vec::new(),
    };

    let mut functions = String::new();
    for (name, root) in fields {
        e.body.clear();
        // Variable names are scoped to the function body being written, so the
        // memo cannot carry across; the helper counter deliberately does, to
        // keep generated function names unique.
        e.memo.clear();

        let result = match root {
            Some(id) if arena.is_alive(*id) => e.emit(*id, "p"),
            _ => {
                e.line("let d_empty = SC_EMPTY;");
                "d_empty".to_string()
            }
        };
        let _ = write!(
            functions,
            "\nfn {name}(p: vec3<f32>) -> f32 {{\n{}    return {result};\n}}\n",
            e.body
        );
    }

    let source = format!(
        "// generated by sc-geom; do not edit\n{PRELUDE}\n{}\n{functions}",
        e.helpers
    );
    Generated {
        source,
        params: e.params,
        unsupported: e.unsupported,
    }
}

struct Emitter<'a> {
    arena: &'a Arena,
    body: String,
    /// Standalone functions emitted alongside `sc_sdf`.
    ///
    /// An extruded profile needs a loop over an array, which cannot be expressed
    /// as a single SSA statement in the main body.
    helpers: String,
    counter: u32,
    /// Keyed by (node, point expression): a shared subtree evaluated under two
    /// different transforms is genuinely two different computations.
    memo: HashMap<(NodeId, String), String>,
    /// Names of the helper functions already written, keyed by node.
    ///
    /// Unlike [`Emitter::memo`] this survives from one field to the next: a
    /// helper takes its point as an argument, so it is the same function
    /// wherever it is called from, and a selected extrusion would otherwise
    /// have its whole polygon written out twice.
    helper_names: HashMap<NodeId, String>,
    /// Values hoisted out of the source, in binding order.
    params: Vec<f32>,
    /// Nodes that had to be emitted as empty space.
    unsupported: Vec<NodeId>,
}

impl Emitter<'_> {
    /// Binds a model value and returns the expression that reads it.
    ///
    /// Everything that can change without altering the shape of the model goes
    /// through here, so editing a radius re-uploads a buffer rather than
    /// recompiling a shader.
    fn p(&mut self, value: f32) -> String {
        self.params.push(value);
        format!("sc_params[{}]", self.params.len() - 1)
    }

    /// Reserves a contiguous run of values and returns the first index.
    fn p_run(&mut self, values: impl IntoIterator<Item = f32>) -> usize {
        let base = self.params.len();
        self.params.extend(values);
        base
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}{}", self.counter)
    }

    fn line(&mut self, s: &str) {
        self.body.push_str("    ");
        self.body.push_str(s);
        self.body.push('\n');
    }

    /// Emits the distance of `id` evaluated at point expression `p`, returning
    /// the name of the variable holding it.
    fn emit(&mut self, id: NodeId, p: &str) -> String {
        let key = (id, p.to_string());
        if let Some(v) = self.memo.get(&key) {
            return v.clone();
        }

        let Some(node) = self.arena.get(id).cloned() else {
            let d = self.fresh("d");
            self.line(&format!("let {d} = SC_EMPTY;"));
            return d;
        };

        let d = match node {
            Node::Mesh { asset, .. } => self.emit_unsupported(id, asset),
            Node::Sphere { .. }
            | Node::Box { .. }
            | Node::Cylinder { .. }
            | Node::Torus { .. }
            | Node::Plane { .. } => self.emit_primitive(&node, p),
            _ => self.emit_composite(id, &node, p),
        };

        self.memo.insert(key, d.clone());
        d
    }

    /// Emits empty space for a node the shader cannot express, and records it.
    ///
    /// Empty space is the only thing that can be emitted here: the function has
    /// to return a distance and the module has to compile, since a shader that
    /// fails to build reaches the user as an opaque driver error. What must not
    /// happen is that it does so quietly, hence [`Generated::unsupported`] and a
    /// comment naming the node in the source itself.
    fn emit_unsupported(&mut self, id: NodeId, asset: crate::sdf::AssetId) -> String {
        if !self.unsupported.contains(&id) {
            self.unsupported.push(id);
        }
        let d = self.fresh("d");
        self.line(&format!(
            "let {d} = SC_EMPTY; // sc-geom: {id} is a mesh ({asset}); no GPU path yet"
        ));
        d
    }

    /// Closed-form distance functions. Each is exact, not merely a bound.
    fn emit_primitive(&mut self, node: &Node, p: &str) -> String {
        match *node {
            Node::Sphere { radius } => {
                let d = self.fresh("d");
                let radius = self.p(radius);
                self.line(&format!("let {d} = length({p}) - {radius};"));
                d
            }

            Node::Box { half, round } => {
                let q = self.fresh("q");
                let d = self.fresh("d");
                let (hx, hy, hz) = (
                    self.p(half.x - round),
                    self.p(half.y - round),
                    self.p(half.z - round),
                );
                self.line(&format!(
                    "let {q} = abs({p}) - vec3<f32>({hx}, {hy}, {hz});"
                ));
                let tail = self.minus(round);
                self.line(&format!(
                    "let {d} = length(max({q}, vec3<f32>(0.0))) + min(max({q}.x, max({q}.y, {q}.z)), 0.0){tail};"
                ));
                d
            }

            Node::Cylinder {
                radius,
                half_height,
                round,
            } => {
                let q = self.fresh("q");
                let d = self.fresh("d");
                let (r, hh) = (self.p(radius - round), self.p(half_height - round));
                self.line(&format!(
                    "let {q} = vec2<f32>(length({p}.xy) - {r}, abs({p}.z) - {hh});"
                ));
                let tail = self.minus(round);
                self.line(&format!(
                    "let {d} = length(max({q}, vec2<f32>(0.0))) + min(max({q}.x, {q}.y), 0.0){tail};"
                ));
                d
            }

            Node::Torus { major, minor } => {
                let q = self.fresh("q");
                let d = self.fresh("d");
                let major = self.p(major);
                self.line(&format!(
                    "let {q} = vec2<f32>(length({p}.xy) - {major}, {p}.z);"
                ));
                let minor = self.p(minor);
                self.line(&format!("let {d} = length({q}) - {minor};"));
                d
            }

            Node::Plane { normal, offset } => {
                // Normalised here rather than in the shader: it is a property of
                // the value, and normalising per pixel would be wasteful.
                let n = normal.normalize();
                let d = self.fresh("d");
                let (nx, ny, nz) = (self.p(n.x), self.p(n.y), self.p(n.z));
                let offset = self.p(offset);
                self.line(&format!(
                    "let {d} = dot({p}, vec3<f32>({nx}, {ny}, {nz})) - {offset};"
                ));
                d
            }

            _ => unreachable!("emit_primitive called with a composite node"),
        }
    }

    /// Emits `" - x"`, or nothing when x is zero.
    ///
    /// The zero case is a structural decision, not a value one: if a rounding
    /// radius becomes non-zero the source changes and the pipeline is rebuilt,
    /// which is exactly right.
    fn minus(&mut self, value: f32) -> String {
        if value == 0.0 {
            String::new()
        } else {
            format!(" - {}", self.p(value))
        }
    }

    /// Emits `body` as a helper function the first time `id` is reached, and
    /// returns the function's name on every reach after that.
    ///
    /// Keyed by node rather than by the profile's contents: two extrusions of
    /// the same rectangle stay two functions, so re-dimensioning one of them
    /// moves a value in the buffer instead of changing the shape of the source
    /// and rebuilding the pipeline.
    /// Emits a subtree as a standalone function taking its point as an argument.
    ///
    /// A pattern evaluates its child once per instance, at a different point
    /// each time. Inlining it the way every other parent does would write the
    /// whole subtree out `count` times, which at two hundred instances is a
    /// shader nobody wants to compile. A function is written once and called in
    /// a loop.
    ///
    /// The child's statements belong inside the function, and the memo entries
    /// it produces name variables that exist only there, so both are swapped out
    /// for the duration and put back afterwards. Sharing `helper_names` with the
    /// profile helpers is deliberate rather than accidental: they have the same
    /// signature and mean the same thing, so a prism used as a pattern's child
    /// is emitted once either way.
    fn emit_as_function(&mut self, id: NodeId) -> String {
        if let Some(name) = self.helper_names.get(&id) {
            return name.clone();
        }
        let name = self.fresh("sc_node_");
        let outer_body = std::mem::take(&mut self.body);
        let outer_memo = std::mem::take(&mut self.memo);

        let result = self.emit(id, "p");

        let inner = std::mem::replace(&mut self.body, outer_body);
        self.memo = outer_memo;
        let _ = write!(
            self.helpers,
            "\nfn {name}(p: vec3<f32>) -> f32 {{\n{inner}    return {result};\n}}\n"
        );
        self.helper_names.insert(id, name.clone());
        name
    }

    fn helper_for(
        &mut self,
        id: NodeId,
        prefix: &str,
        body: impl FnOnce(&mut Self, &str),
    ) -> String {
        if let Some(name) = self.helper_names.get(&id) {
            return name.clone();
        }
        let name = self.fresh(prefix);
        body(self, &name);
        self.helper_names.insert(id, name.clone());
        name
    }

    /// Booleans and modifiers, which recurse into their children.
    fn emit_composite(&mut self, id: NodeId, node: &Node, p: &str) -> String {
        match *node {
            Node::Union { a, b, smooth } => {
                let (da, db) = (self.emit(a, p), self.emit(b, p));
                self.combine("min", "sc_smin", &da, &db, smooth)
            }

            Node::Difference { a, b, smooth } => {
                let (da, db) = (self.emit(a, p), self.emit(b, p));
                let neg = self.fresh("n");
                self.line(&format!("let {neg} = -{db};"));
                self.combine("max", "sc_smax", &da, &neg, smooth)
            }

            Node::Intersection { a, b, smooth } => {
                let (da, db) = (self.emit(a, p), self.emit(b, p));
                self.combine("max", "sc_smax", &da, &db, smooth)
            }

            Node::Transform { child, xform, .. } => self.emit_transform(child, &xform, p),

            Node::Offset { child, distance } => {
                let dc = self.emit(child, p);
                if distance == 0.0 {
                    dc
                } else {
                    let d = self.fresh("d");
                    let distance = self.p(distance);
                    self.line(&format!("let {d} = {dc} - {distance};"));
                    d
                }
            }

            Node::Extrude { ref profile, depth } => {
                let name = self.helper_for(id, "sc_extrude_", |e, name| {
                    e.emit_extrude_fn(name, profile, depth);
                });
                let d = self.fresh("d");
                self.line(&format!("let {d} = {name}({p});"));
                d
            }

            Node::Pattern { child, kind, count } => {
                let f = self.emit_as_function(child);
                let d = self.fresh("d");
                // The count is the loop bound, so it is baked into the source
                // and changing it rebuilds the pipeline. Everything else about a
                // pattern is a value in the buffer.
                // Seeded from instance zero rather than from empty space,
                // which it is: `Repeat::placement(0, _)` is the identity for
                // both kinds. The empty sentinel would work and would also
                // make this node indistinguishable from one the shader had
                // to drop, which is what `SC_EMPTY` is counted to detect.
                self.line(&format!("var {d} = {f}({p});"));
                self.line(&format!("for (var i = 1u; i < {count}u; i = i + 1u) {{"));
                let point = self.fresh("q");
                match kind {
                    crate::node::Repeat::Linear { step } => {
                        let (sx, sy, sz) = (self.p(step.x), self.p(step.y), self.p(step.z));
                        self.line(&format!(
                            "    let {point} = {p} - vec3<f32>({sx}, {sy}, {sz}) * f32(i);"
                        ));
                    }
                    crate::node::Repeat::Circular { sweep } => {
                        // Sampling moves the point rather than the shape, so the
                        // rotation is the inverse of the instance's own.
                        let spans = crate::node::Repeat::Circular { sweep }.spans(count);
                        let a = self.p(-sweep / spans as f32);
                        self.line(&format!("    let a = {a} * f32(i);"));
                        self.line("    let ca = cos(a);");
                        self.line("    let sa = sin(a);");
                        self.line(&format!(
                            "    let {point} = vec3<f32>({p}.x * ca - {p}.y * sa, \
                             {p}.x * sa + {p}.y * ca, {p}.z);"
                        ));
                    }
                }
                self.line(&format!("    {d} = min({d}, {f}({point}));"));
                self.line("}");
                d
            }

            Node::Prism { ref profile } => {
                let name = self.helper_for(id, "sc_prism_", |e, name| {
                    e.emit_prism_fn(name, profile);
                });
                let d = self.fresh("d");
                self.line(&format!("let {d} = {name}({p});"));
                d
            }

            Node::Shell { child, thickness } => {
                let dc = self.emit(child, p);
                let d = self.fresh("d");
                let thickness = self.p(thickness);
                self.line(&format!("let {d} = max({dc}, -({dc} + {thickness}));"));
                d
            }

            _ => unreachable!("emit_composite called with a primitive node"),
        }
    }

    /// Emits a function computing the exact distance to an extruded profile.
    ///
    /// A rectangle and a circle get closed forms; anything else walks its
    /// polygon. The vertex count is structural and appears as a literal loop
    /// bound, while the vertices come from the parameter buffer, so dragging a
    /// dimension does not recompile anything.
    fn emit_extrude_fn(&mut self, name: &str, profile: &crate::Profile, depth: f32) {
        let plane = self.emit_profile_block(profile);
        let h = self.p(depth);
        let _ = write!(
            self.helpers,
            "\nfn {name}(p: vec3<f32>) -> f32 {{
{plane}    let slab = max(-p.z, p.z - {h});
    return min(max(plane, slab), 0.0) + length(max(vec2<f32>(plane, slab), vec2<f32>(0.0)));
}}\n"
        );
    }

    /// A prism is the profile distance and nothing else: no slab term, because
    /// the sweep has no end to be inside or outside of.
    fn emit_prism_fn(&mut self, name: &str, profile: &crate::Profile) {
        let plane = self.emit_profile_block(profile);
        let _ = write!(
            self.helpers,
            "\nfn {name}(p: vec3<f32>) -> f32 {{
{plane}    return plane;
}}\n"
        );
    }

    /// WGSL binding `plane` to the signed distance from `p.xy` to the profile.
    ///
    /// Shared by the two swept nodes, so a fix to the polygon crossing rule or
    /// to either closed form lands in both.
    fn emit_profile_block(&mut self, profile: &crate::Profile) -> String {
        match profile {
            crate::Profile::Rect { width, height } => {
                let (hw, hh) = (self.p(width * 0.5), self.p(height * 0.5));
                format!(
                    "    let q = abs(p.xy) - vec2<f32>({hw}, {hh});
    let plane = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
"
                )
            }
            crate::Profile::Circle { radius } => {
                let r = self.p(*radius);
                format!("    let plane = length(p.xy) - {r};\n")
            }
            other => {
                let poly = other.polygon();
                let n = poly.len();
                let base = self.p_run(poly.iter().flat_map(|v| [v.x, v.y]));
                let last = base + n.saturating_sub(1) * 2;
                format!(
                    "    let q = p.xy;
    var d = 1e30;
    var s = 1.0;
    // The previous vertex is carried rather than indexed. The wrapping
    // `(i + n - 1) % n` and the second pair of buffer reads it fed were per
    // vertex of every profile, inside a loop the tracer runs on the order of a
    // hundred times per pixel.
    var vj = vec2<f32>(sc_params[{last}u], sc_params[{last}u + 1u]);
    for (var i = 0u; i < {n}u; i = i + 1u) {{
        let vi = vec2<f32>(sc_params[{base}u + i * 2u], sc_params[{base}u + i * 2u + 1u]);
        let e = vj - vi;
        let w = q - vi;
        // Guarded the way `sd_polygon` guards it. A sketch with a repeated
        // point gives a zero-length edge, and an unguarded 0/0 here is a NaN
        // that empties the viewport while the mesher still exports the part.
        let b = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-20), 0.0, 1.0);
        d = min(d, dot(b, b));
        // Three scalars rather than a bool vector: naga rejects a
        // `vec3<bool>` constructor here.
        let c0 = q.y >= vi.y;
        let c1 = q.y < vj.y;
        let c2 = e.x * w.y > e.y * w.x;
        if ((c0 && c1 && c2) || (!c0 && !c1 && !c2)) {{ s = -s; }}
        vj = vi;
    }}
    let plane = s * sqrt(d);
"
                )
            }
        }
    }

    /// Peephole: a placement is usually a pure translation, and the identity
    /// parts of it are dead arithmetic inside a loop that runs on the order of
    /// a hundred times per pixel. Emit only what actually does something.
    ///
    /// These decisions are structural. Moving a scale off 1.0 changes the source
    /// and rebuilds the pipeline, which is correct and rare.
    fn emit_transform(&mut self, child: NodeId, xform: &crate::math::Transform, p: &str) -> String {
        let q = xform.rotation;
        let unit_scale = (xform.scale - 1.0).abs() < 1e-9;
        // Tested on the vector part, not on `w`. Near the identity `w` is flat:
        // every rotation up to about a twentieth of a degree lands within two
        // f32 ulp of 1.0, so a threshold on `w` drops rotations the CPU applies,
        // which is a tenth of a millimetre at a hundred millimetres out. The
        // vector part is exactly zero for the identity and grows with the angle,
        // so this reads the same way as the translation test above it.
        let no_rotation = q.xyz().length_squared() < 1e-18;
        let no_translation = xform.translation.length_squared() < 1e-18;

        let mut expr = p.to_string();
        if !no_translation {
            let (tx, ty, tz) = (
                self.p(xform.translation.x),
                self.p(xform.translation.y),
                self.p(xform.translation.z),
            );
            expr = format!("({expr} - vec3<f32>({tx}, {ty}, {tz}))");
        }
        if !no_rotation {
            let (qx, qy, qz, qw) = (self.p(-q.x), self.p(-q.y), self.p(-q.z), self.p(q.w));
            expr = format!("sc_qrot(vec4<f32>({qx}, {qy}, {qz}, {qw}), {expr})");
        }
        if !unit_scale {
            let inv = self.p(1.0 / xform.scale);
            expr = format!("({expr} * {inv})");
        }

        let pn = if expr == p {
            p.to_string()
        } else {
            let v = self.fresh("p");
            self.line(&format!("let {v} = {expr};"));
            v
        };

        let dc = self.emit(child, &pn);
        if unit_scale {
            dc
        } else {
            let d = self.fresh("d");
            let scale = self.p(xform.scale);
            self.line(&format!("let {d} = {dc} * {scale};"));
            d
        }
    }

    /// Hard combine when the blend radius is zero, smooth otherwise. Emitting
    /// `min` directly rather than always calling the smooth helper keeps the
    /// common case free of a division and a clamp.
    fn combine(&mut self, hard: &str, soft: &str, a: &str, b: &str, k: f32) -> String {
        let d = self.fresh("d");
        if k <= 0.0 {
            self.line(&format!("let {d} = {hard}({a}, {b});"));
        } else {
            let k = self.p(k);
            self.line(&format!("let {d} = {soft}({a}, {b}, {k});"));
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::{generate, generate_with_selection, Generated};
    use crate::math::Transform;
    use crate::node::Node;
    use crate::profile::Profile;
    use crate::sdf::{AssetId, Grid};
    use crate::{eval, Builder, NodeId};
    use glam::{Quat, Vec2, Vec3};
    use std::sync::Arc;

    /// Parse and type-check the way wgpu will at runtime.
    ///
    /// Kept here as well as in `lib.rs` so that a test written next to the
    /// emitter never has to reach for a neighbour's private helper, which is how
    /// codegen tests end up only ever comparing strings.
    fn validate(src: &str) {
        let module = match naga::front::wgsl::parse_str(src) {
            Ok(m) => m,
            Err(e) => panic!("generated WGSL does not parse: {e:?}\n{src}"),
        };
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        if let Err(e) = validator.validate(&module) {
            panic!("generated WGSL does not validate: {e:?}\n{src}");
        }
    }

    /// The generated code with its comments removed, so a claim about what the
    /// GPU executes is not answered by something the emitter merely wrote down.
    fn code_of(src: &str) -> String {
        src.lines()
            .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The body of one emitted function, so a claim about `sc_sdf` is not
    /// accidentally satisfied by something in a helper or in `sc_selected`.
    fn body_of<'a>(generated: &'a Generated, name: &str) -> &'a str {
        let from = generated
            .source
            .split_once(&format!("fn {name}(p: vec3<f32>) -> f32 {{"))
            .unwrap_or_else(|| panic!("no {name} in\n{}", generated.source))
            .1;
        from.split_once("\n}").unwrap().0
    }

    #[test]
    fn a_rotation_the_cpu_applies_is_not_dropped_by_the_peephole() {
        // Under a twentieth of a degree. Small, and well inside the range a
        // placement derived from a face can land on, but not nothing: at fifty
        // millimetres out it moves the surface by more than twice the half-pixel
        // tolerance the tracer works to.
        let angle = 9.0e-4_f32;
        let half = Vec3::new(100.0, 1.0, 1.0);
        let probe = Vec3::new(50.0, 1.0, 0.0);

        let mut b = Builder::new();
        let slab = b.cuboid(half).unwrap();
        let straight = b.transform(slab, Transform::IDENTITY).unwrap();
        let turned = b
            .rotate(slab, Quat::from_rotation_z(angle))
            .expect("a small rotation is a valid transform");

        let moved = (eval(&b.arena, straight, probe) - eval(&b.arena, turned, probe)).abs();
        assert!(
            moved > 0.02,
            "the fixture rotation is too small to matter: {moved}"
        );

        let generated = generate(&b.arena, Some(turned));
        assert!(
            body_of(&generated, "sc_sdf").contains("sc_qrot"),
            "the shader dropped a rotation the CPU applies:\n{}",
            generated.source
        );
        validate(&generated.source);
    }

    #[test]
    fn an_identity_rotation_still_emits_nothing() {
        // The other side of the same peephole: a placement that is a pure
        // translation must not pay for a quaternion in the inner loop.
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let t = b.translate(s, Vec3::new(4.0, 0.0, 0.0)).unwrap();
        let generated = generate(&b.arena, Some(t));
        assert!(
            !body_of(&generated, "sc_sdf").contains("sc_qrot"),
            "identity rotation survived:\n{}",
            generated.source
        );
    }

    #[test]
    fn a_repeated_sketch_point_cannot_divide_by_zero_in_the_shader() {
        // A double click while sketching repeats a point, which is a zero-length
        // edge. `sd_polygon` falls back to the vertex; an unguarded division in
        // the shader would return NaN instead, and one NaN poisons every `min`
        // and `max` above it. The part would still export correctly, which is
        // what makes this class of disagreement so expensive to find.
        let profile = Profile::Path {
            points: vec![
                Vec2::new(-5.0, -5.0),
                Vec2::new(5.0, -5.0),
                Vec2::new(5.0, -5.0),
                Vec2::new(5.0, 5.0),
                Vec2::new(-5.0, 5.0),
            ],
        };
        assert!(profile.is_valid(), "the arena would refuse this fixture");
        assert!(
            profile.distance(Vec2::new(9.0, 0.0)).is_finite(),
            "the CPU already disagrees with itself"
        );

        let mut b = Builder::new();
        let e = b.extrude(profile, 3.0).unwrap();
        let src = generate(&b.arena, Some(e)).source;
        assert!(
            !src.contains("/ dot(e, e)"),
            "a zero-length edge divides by zero on the GPU:\n{src}"
        );
        assert!(src.contains("max(dot(e, e)"), "{src}");
        validate(&src);
    }

    #[test]
    fn the_polygon_loop_carries_its_previous_vertex() {
        // Dead arithmetic in the hottest loop in the program: the wrapping index
        // and the reads it fed ran once per vertex per trace step, and the
        // previous vertex is already in hand from the iteration before.
        let mut b = Builder::new();
        let e = b
            .extrude(
                Profile::RegularPolygon {
                    sides: 8,
                    radius: 4.0,
                },
                2.0,
            )
            .unwrap();
        let src = generate(&b.arena, Some(e)).source;
        assert!(
            !code_of(&src).contains('%'),
            "the polygon loop still recomputes a wrapping index:\n{src}"
        );
        validate(&src);
    }

    #[test]
    fn selecting_a_profile_does_not_emit_its_polygon_twice() {
        // `sc_sdf` and `sc_selected` are two fields over one arena. A helper
        // takes its point as an argument, so it is the same function in both,
        // and a 256 point path emitted twice is two loops and twice the buffer.
        let mut b = Builder::new();
        let e = b
            .extrude(
                Profile::RegularPolygon {
                    sides: 7,
                    radius: 3.0,
                },
                2.0,
            )
            .unwrap();
        let s = b.sphere(4.0).unwrap();
        let root = b.union(e, s).unwrap();

        let generated = generate_with_selection(&b.arena, Some(root), Some(e));
        assert_eq!(
            generated.source.matches("fn sc_extrude_").count(),
            1,
            "the profile was written out once per field:\n{}",
            generated.source
        );
        let unselected = generate_with_selection(&b.arena, Some(root), None);
        assert_eq!(
            generated.params.len(),
            unselected.params.len(),
            "selecting a node changed how many values the buffer holds"
        );
        validate(&generated.source);
    }

    /// Every node kind at once, including the two the hand written examples in
    /// `lib.rs` leave out: a prism, which has no slab term, and a mesh, which
    /// has no shader form at all.
    fn every_kind() -> (Builder, NodeId, NodeId) {
        let mut b = Builder::new();
        let sphere = b.sphere(2.0).unwrap();
        let cuboid = b.rounded_cuboid(Vec3::splat(2.0), 0.3).unwrap();
        let cylinder = b
            .arena
            .insert(Node::Cylinder {
                radius: 1.0,
                half_height: 3.0,
                round: 0.2,
            })
            .unwrap();
        let torus = b.torus(3.0, 0.5).unwrap();
        let plane = b.plane(Vec3::new(0.0, 0.0, 1.0), -4.0).unwrap();

        let grid = Grid::from_fn([25; 3], Vec3::splat(-6.0), 0.5, |p| p.length() - 3.0);
        assert!(
            grid.boundary_clearance() >= Grid::REQUIRED_CLEARANCE as f32 * grid.spacing,
            "fixture grid does not meet the padding precondition"
        );
        let mesh = b
            .arena
            .insert(Node::mesh(AssetId(1), Arc::new(grid)))
            .unwrap();

        let blended = b.smooth_union(sphere, cuboid, 0.4).unwrap();
        let cut = b.smooth_difference(blended, cylinder, 0.1).unwrap();
        let kept = b
            .arena
            .insert(Node::Intersection {
                a: cut,
                b: torus,
                smooth: 0.2,
            })
            .unwrap();
        let placed = b
            .transform(
                kept,
                Transform {
                    translation: Vec3::new(1.0, 2.0, 3.0),
                    rotation: Quat::from_rotation_y(0.6),
                    scale: 1.5,
                },
            )
            .unwrap();
        let grown = b.offset(placed, 0.1).unwrap();
        let hollow = b.shell(grown, 0.8).unwrap();

        let pad = b
            .extrude(
                Profile::Rect {
                    width: 8.0,
                    height: 5.0,
                },
                4.0,
            )
            .unwrap();
        let through = b
            .arena
            .insert(Node::Prism {
                profile: Profile::RegularPolygon {
                    sides: 6,
                    radius: 1.5,
                },
            })
            .unwrap();
        let bored = b.difference(pad, through).unwrap();

        let joined = b.union(hollow, bored).unwrap();
        let with_mesh = b.union(joined, mesh).unwrap();
        let trimmed = b.difference(with_mesh, plane).unwrap();
        (b, trimmed, mesh)
    }

    /// The blend guard has to exist on both sides or they disagree about the
    /// same model.
    ///
    /// `eval::smin` degrades to a hard `min` when an operand is not finite,
    /// because the interpolation would otherwise give `inf * 0`, which is NaN.
    /// Empty space is exactly such an operand. Without the same guard in the
    /// shader, one missing feature blanks the whole model on the GPU while the
    /// CPU renders it correctly: the render looks wrong and the export is
    /// right, which is the worst failure this kernel has.
    #[test]
    fn the_shader_guards_the_blend_the_same_way_the_cpu_does() {
        let src = generate(&crate::Arena::new(), None).source;
        assert!(
            src.contains("fn sc_finite"),
            "the shader has no finiteness test, so a blend against empty space \
             will return NaN where the CPU returns the other operand"
        );
        let blend = src
            .split("fn sc_smin")
            .nth(1)
            .expect("the blend helper is always emitted");
        let body = &blend[..blend.find("\n}").expect("the helper closes")];
        assert!(
            body.contains("sc_finite(a)") && body.contains("sc_finite(b)"),
            "sc_smin does not check both operands:\n{body}"
        );
        // `sc_smax` is `-sc_smin(-a, -b, k)`, so it inherits the guard, and
        // negating an infinity keeps it infinite.
        assert!(src.contains("fn sc_smax"), "the max blend went missing");
    }

    #[test]
    fn every_node_kind_emits_and_the_module_validates() {
        let (b, root, mesh) = every_kind();

        // Every kind the arena can hold is in the fixture, so this is coverage
        // of the emitter rather than of one convenient corner of it.
        let kinds: std::collections::BTreeSet<&str> = b
            .arena
            .live_ids()
            .map(|id| b.arena.get(id).unwrap().kind())
            .collect();
        assert_eq!(
            kinds.len(),
            14,
            "the fixture no longer covers every node kind: {kinds:?}"
        );

        let generated = generate(&b.arena, Some(root));
        validate(&generated.source);
        assert!(
            generated.params.iter().all(|v| v.is_finite()),
            "non-finite parameter: {:?}",
            generated.params
        );

        // The mesh is the one gap, and it is reported rather than silently left
        // as a hole in the part.
        assert_eq!(generated.unsupported, vec![mesh]);
        assert!(!generated.is_complete());
        assert_eq!(
            generated.source.matches("= SC_EMPTY;").count(),
            2,
            "something other than the mesh and the empty selection field was \
             emitted as empty space:\n{}",
            generated.source
        );
    }

    #[test]
    fn a_prism_emits_no_slab_term() {
        // The difference between a prism and an extrusion is exactly the slab,
        // and getting that wrong turns a through cut into a blind one on the GPU
        // while the mesher goes on cutting all the way through.
        let mut b = Builder::new();
        let prism = b
            .arena
            .insert(Node::Prism {
                profile: Profile::Circle { radius: 2.0 },
            })
            .unwrap();
        let src = generate(&b.arena, Some(prism)).source;
        assert!(!src.contains("slab"), "a prism grew an end:\n{src}");

        let extrude = b.extrude(Profile::Circle { radius: 2.0 }, 5.0).unwrap();
        let src = generate(&b.arena, Some(extrude)).source;
        assert!(src.contains("slab"), "an extrusion lost its end:\n{src}");
    }
}
