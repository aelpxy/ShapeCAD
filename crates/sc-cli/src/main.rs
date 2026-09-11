//! `ShapeCAD` headless runner.
//!
//! Everything the GUI can do to a document, this can do without a window. That
//! is not a convenience feature: it is how automated tests and AI agents drive
//! the application, and how they verify that what they built is what they meant.
//!
//! All dimensions are millimetres.

use sc_doc::samples::bracket;
use sc_mesh::{contour, Settings};
use std::path::{Path, PathBuf};

const GOLDEN: &str = "tests/corpus/bracket.hash";

/// What a command line that could not be understood exits with.
///
/// Kept apart from [`FAILED`], which is work that was understood and did not
/// come off. A script wrapping this can tell "I asked for the wrong thing" from
/// "the thing I asked for went wrong", which is the difference between a typo
/// and a regression.
const USAGE_ERROR: i32 = 2;

/// What a command that was understood and failed exits with.
const FAILED: i32 = 1;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map_or("demo", String::as_str);
    let rest = args.get(1..).unwrap_or_default();
    let flag = |f: &str| args.iter().any(|a| a == f);

    match cmd {
        "demo" => demo(flag("--wgsl")),
        "selftest" => std::process::exit(selftest()),
        "export" => std::process::exit(export(rest)),
        "write" => std::process::exit(write(rest)),
        "help" | "--help" | "-h" => usage(),
        other => {
            eprintln!("unknown command: {other}\n");
            usage();
            std::process::exit(USAGE_ERROR);
        }
    }
}

/// Reports why a file cannot be written, before the work to fill it is done.
///
/// Meshing the reference part at a fine resolution takes tens of seconds, and
/// finding out afterwards that the directory does not exist wastes all of it.
fn writable(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(format!("no such directory: {}", parent.display()));
    }
    Ok(())
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

/// Coarsest and finest grid a mesh will be asked for.
///
/// The mesher samples a cube of cells, so the memory it needs is this number
/// cubed: 512 is a gigabyte of field samples, and already an order finer than
/// any nozzle over a part this size. Past that the error a user sees is the
/// process being killed by the kernel, which says nothing about what they typed.
/// Under 8 there are too few cells to close a surface and the mesh comes out as
/// confetti, which is just as wrong and much harder to notice.
const MIN_RESOLUTION: u32 = 8;
const MAX_RESOLUTION: u32 = 512;

/// Grid resolution when none is asked for. Fine enough to print, quick enough
/// to wait for.
const DEFAULT_RESOLUTION: u32 = 128;

/// What `export` was asked to do.
#[derive(Debug, PartialEq, Eq)]
struct Export {
    path: PathBuf,
    resolution: u32,
}

/// Reads the arguments after `export`.
///
/// Every mistake is an error rather than a default. A resolution that does not
/// parse used to fall back to 128 and print that it had used 128, in among
/// numbers that all looked reasonable, and a file name after `--resolution N`
/// was ignored entirely: the mesh went to `bracket.stl` and the path the user
/// gave stayed empty.
fn parse_export(args: &[String]) -> Result<Export, String> {
    let mut path: Option<PathBuf> = None;
    let mut resolution = DEFAULT_RESOLUTION;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--resolution" => {
                let value = rest
                    .next()
                    .ok_or_else(|| "--resolution needs a number after it".to_string())?;
                resolution = value.parse().map_err(|_| {
                    format!("--resolution takes a whole number of cells, not {value:?}")
                })?;
                if !(MIN_RESOLUTION..=MAX_RESOLUTION).contains(&resolution) {
                    return Err(format!(
                        "--resolution {resolution} is outside {MIN_RESOLUTION} to {MAX_RESOLUTION}"
                    ));
                }
            }
            other if other.starts_with("--") => return Err(format!("unknown option: {other}")),
            other if path.is_some() => return Err(format!("only one file at a time, not {other}")),
            other => path = Some(PathBuf::from(other)),
        }
    }
    Ok(Export {
        path: path.unwrap_or_else(|| PathBuf::from("bracket.stl")),
        resolution,
    })
}

/// Mesh the reference part and write it as STL.
///
/// Reports the topology rather than assuming it: a hole is invisible in a render
/// and fatal to a print, so the numbers are printed every time.
fn export(args: &[String]) -> i32 {
    let Export { path, resolution } = match parse_export(args) {
        Ok(export) => export,
        Err(e) => {
            eprintln!("{e}");
            return USAGE_ERROR;
        }
    };
    if let Err(e) = writable(&path) {
        eprintln!("cannot write {}: {e}", path.display());
        return USAGE_ERROR;
    }

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
        return FAILED;
    }

    match sc_mesh::stl::write(&mesh, &path) {
        Ok(()) => println!("wrote {}", path.display()),
        Err(e) => {
            eprintln!("could not write {}: {e}", path.display());
            return FAILED;
        }
    }
    0
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
fn write(args: &[String]) -> i32 {
    let (Some(name), Some(path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: shapecad write <name> <file.shapecad>\nnames: bracket, engine");
        return USAGE_ERROR;
    };
    let doc = match name.as_str() {
        "bracket" => sc_doc::samples::bracket(),
        "engine" => sc_doc::samples::engine(),
        other => {
            eprintln!("unknown model: {other}\nnames: bracket, engine");
            return USAGE_ERROR;
        }
    };
    let path = Path::new(path);
    if let Err(e) = writable(path) {
        eprintln!("cannot write {}: {e}", path.display());
        return USAGE_ERROR;
    }
    if let Err(e) = sc_doc::file::save(&doc, path) {
        eprintln!("could not write {}: {e}", path.display());
        return FAILED;
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
    0
}

#[cfg(test)]
mod tests {
    use super::{
        parse_export, writable, Export, DEFAULT_RESOLUTION, MAX_RESOLUTION, MIN_RESOLUTION,
    };
    use std::path::{Path, PathBuf};

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn export_with_nothing_after_it_meshes_to_a_default_file() {
        assert_eq!(
            parse_export(&[]).expect("no arguments"),
            Export {
                path: PathBuf::from("bracket.stl"),
                resolution: DEFAULT_RESOLUTION,
            }
        );
    }

    /// A file named after the option used to be dropped on the floor: the path
    /// was read from the first argument only, which was `--resolution`, so the
    /// mesh went to `bracket.stl` and the file that was asked for stayed empty.
    #[test]
    fn a_file_named_after_an_option_is_still_the_file() {
        for line in ["part.stl --resolution 64", "--resolution 64 part.stl"] {
            assert_eq!(
                parse_export(&args(line)).unwrap_or_else(|e| panic!("{line}: {e}")),
                Export {
                    path: PathBuf::from("part.stl"),
                    resolution: 64,
                },
                "{line}"
            );
        }
    }

    /// A resolution that cannot be honoured has to say so. Falling back to the
    /// default prints a number that looks like the one that was asked for and
    /// writes a mesh at a different fineness.
    #[test]
    fn a_resolution_that_is_not_a_resolution_is_refused() {
        for line in [
            "--resolution",
            "--resolution x",
            "--resolution 12.5",
            "--resolution -4",
            "--resolution 0",
            "--resolution 1",
            "--resolution 100000",
            "--resolution 4294967296",
        ] {
            assert!(
                parse_export(&args(line)).is_err(),
                "{line} was accepted as a resolution"
            );
        }
    }

    /// The ceiling is memory: the mesher samples a cube of cells, so the grid
    /// for a resolution of a hundred thousand is `1e15` floats. Asking for it
    /// ends as a process killed by the kernel, which tells the user nothing.
    #[test]
    fn the_resolution_range_is_what_can_actually_be_meshed() {
        for good in [MIN_RESOLUTION, 64, DEFAULT_RESOLUTION, MAX_RESOLUTION] {
            let line = format!("--resolution {good}");
            assert_eq!(parse_export(&args(&line)).expect(&line).resolution, good);
        }
        let cells = u64::from(MAX_RESOLUTION).pow(3) * 4;
        assert!(
            cells < 8 << 30,
            "the largest grid allowed is {cells} bytes of field samples"
        );
    }

    #[test]
    fn an_unknown_option_is_not_a_file_name() {
        assert!(parse_export(&args("--verbose")).is_err());
        assert!(parse_export(&args("a.stl b.stl")).is_err());
    }

    /// Meshing takes tens of seconds. Finding out afterwards that the directory
    /// does not exist wastes every one of them.
    #[test]
    fn a_path_that_cannot_be_written_is_reported_before_the_work() {
        let scratch = std::env::temp_dir();
        assert!(writable(&scratch.join("bracket.stl")).is_ok());
        assert!(writable(Path::new("bracket.stl")).is_ok());
        assert!(
            writable(&scratch.join("no-such-directory-4b1f/bracket.stl")).is_err(),
            "a missing directory was accepted"
        );
        assert!(
            writable(&scratch).is_err(),
            "a directory was accepted as a file to write"
        );
    }
}
