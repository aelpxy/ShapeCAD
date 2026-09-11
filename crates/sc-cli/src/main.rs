//! `ShapeCAD` headless runner.
//!
//! Everything the GUI can do to a document, this can do without a window. That
//! is not a convenience feature: it is how automated tests and AI agents drive
//! the application, and how they verify that what they built is what they meant.
//!
//! All dimensions are millimetres.

use sc_doc::samples::bracket;
use sc_mesh::{contour, Settings};
use std::path::Path;

const GOLDEN: &str = "tests/corpus/bracket.hash";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map_or("demo", String::as_str);
    let flag = |f: &str| args.iter().any(|a| a == f);

    match cmd {
        "demo" => demo(flag("--wgsl")),
        "selftest" => std::process::exit(selftest()),
        "export" => export(&args),
        "write" => write(&args),
        "help" | "--help" | "-h" => usage(),
        other => {
            eprintln!("unknown command: {other}\n");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    println!(
        "shapecad <command>\n\n\
           demo [--wgsl]   build the reference part and report it\n\
           export <file.stl> [--resolution N]\n\
           \x20               mesh the reference part and write it\n\
           write <name> <file.shapecad>\n\
           \x20               write a reference model as a document\n\
           \x20               names: bracket, engine\n\
           selftest        compare the reference part against its golden hash\n\
           help            this message\n"
    );
}

fn demo(show_wgsl: bool) {
    let d = bracket();

    println!("=== outline ===");
    print!("{}", d.outline());

    let b = d.bounds().expect("rooted");
    let s = b.size();
    println!("\n=== bounds (mm) ===");
    println!("min  {:.2} {:.2} {:.2}", b.min.x, b.min.y, b.min.z);
    println!("max  {:.2} {:.2} {:.2}", b.max.x, b.max.y, b.max.z);
    println!("size {:.2} x {:.2} x {:.2}", s.x, s.y, s.z);

    println!("\n=== hash ===");
    println!("{}", d.hash().expect("rooted"));

    println!("\n=== history ===");
    println!("{} commands, undo available: {}", d.log_len(), d.can_undo());

    if show_wgsl {
        println!("\n=== wgsl ===");
        let generated = d.wgsl();
        print!("{}", generated.source);
        println!("// {} bound parameters", generated.params.len());
    }
}

/// Mesh the reference part and write it as STL.
///
/// Reports the topology rather than assuming it: a hole is invisible in a render
/// and fatal to a print, so the numbers are printed every time.
fn export(args: &[String]) {
    let path = args
        .get(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "bracket.stl".to_string());
    let resolution = args
        .iter()
        .position(|a| a == "--resolution")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);

    let doc = bracket();
    let root = doc.root().expect("sample is rooted");
    let bounds = doc.bounds().expect("sample is rooted");

    let started = std::time::Instant::now();
    let mesh = contour(
        doc.arena(),
        root,
        Settings {
            resolution,
            refinement: 2,
        },
    );
    let elapsed = started.elapsed();

    let cell = bounds.size().max_element() / resolution as f32;
    println!("resolution {resolution} ({cell:.3} mm per cell), meshed in {elapsed:.2?}");
    println!(
        "{} triangles, {} vertices",
        mesh.triangle_count(),
        mesh.positions.len()
    );
    println!("volume {:.1} mm^3", mesh.volume());

    let topology = mesh.topology();
    if topology.is_printable() {
        println!("watertight: closed, manifold, consistently wound");
    } else {
        eprintln!(
            "NOT PRINTABLE: {} holes, {} non-manifold edges, {} inconsistent",
            topology.boundary_edges, topology.non_manifold_edges, topology.inconsistent_edges
        );
        std::process::exit(1);
    }

    match sc_mesh::stl::write(&mesh, std::path::Path::new(&path)) {
        Ok(()) => println!("wrote {path}"),
        Err(e) => {
            eprintln!("could not write {path}: {e}");
            std::process::exit(1);
        }
    }
}

/// Compare the reference part against a checked-in hash.
///
/// This is the regression harness in miniature: if a refactor of the kernel
/// changes the shape of a known-good part by so much as a quantum, this fails.
fn selftest() -> i32 {
    let d = bracket();
    let hash = d.hash().expect("rooted").short();
    let path = Path::new(GOLDEN);

    if let Ok(golden) = std::fs::read_to_string(path) {
        let golden = golden.trim();
        if golden == hash {
            println!("ok: bracket matches golden {hash}");
            0
        } else {
            eprintln!("FAIL: bracket hash changed\n  golden {golden}\n  actual {hash}");
            1
        }
    } else {
        {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(path, format!("{hash}\n")) {
                eprintln!("could not write {}: {e}", path.display());
                1
            } else {
                println!("baseline written to {}: {hash}", path.display());
                0
            }
        }
    }
}

/// Writes a reference model out as a document, so it can be opened and edited.
///
/// The samples are built in code rather than shipped as files, because a sample
/// is a test fixture and a file on disk cannot be checked against the kernel it
/// was built with. This turns one into a file on demand.
fn write(args: &[String]) {
    let name = args.get(1).map_or("engine", String::as_str);
    let doc = match name {
        "bracket" => sc_doc::samples::bracket(),
        "engine" => sc_doc::samples::engine(),
        other => {
            eprintln!("unknown model: {other}\nnames: bracket, engine");
            std::process::exit(2);
        }
    };
    let Some(path) = args.get(2) else {
        eprintln!("usage: shapecad write <name> <file.shapecad>");
        std::process::exit(2);
    };
    let path = std::path::Path::new(path);
    if let Err(e) = sc_doc::file::save(&doc, path) {
        eprintln!("could not write {}: {e}", path.display());
        std::process::exit(1);
    }
    let bounds = doc.bounds().expect("a sample is always rooted");
    let size = bounds.size();
    println!(
        "wrote {} ({} nodes, {:.0} x {:.0} x {:.0} mm)",
        path.display(),
        doc.arena().len(),
        size.x,
        size.y,
        size.z
    );
}
