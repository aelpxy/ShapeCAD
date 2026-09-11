//! Voxel grids and the sidecar directory that stores them.
//!
//! An imported triangle mesh becomes a signed distance grid so that it behaves
//! like any other field in the kernel. A grid is several megabytes of binary,
//! which is exactly what a `.shapecad` file is not: the document is JSON so it
//! diffs cleanly and is directly legible to a language model. Base64 in the
//! JSON would destroy both properties, so grids live beside the document in a
//! sidecar directory and the JSON references them by [`AssetId`].
//!
//! ```text
//! part.shapecad          JSON, with `Mesh` nodes naming their asset
//! part.assets/           one file per asset
//!   00000000.scsdf
//!   00000001.scsdf
//! ```
//!
//! See [`docs/adr/0006-sidecar-assets.md`] for the trade this makes.
//!
//! [`docs/adr/0006-sidecar-assets.md`]: https://github.com/aelpxy/shapecad/blob/main/docs/adr/0006-sidecar-assets.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use sc_geom::sdf::{AssetId, Grid};
/// Magic at the head of every grid file.
pub const GRID_MAGIC: [u8; 8] = *b"SCSDF\0\0\0";

/// Version of the binary grid encoding, independent of the document format.
pub const GRID_VERSION: u32 = 1;

/// Extension of a single grid file inside the sidecar directory.
pub const GRID_EXTENSION: &str = "scsdf";

/// Bytes of fixed header before the samples: magic, version, dims, origin,
/// spacing.
const HEADER_LEN: usize = 8 + 4 + 12 + 12 + 4;

/// Why a run of bytes is not a grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridError {
    /// Fewer bytes than the fixed header needs.
    Header {
        /// How many bytes there were.
        actual: usize,
    },
    /// The magic does not match, so this is not a `ShapeCAD` grid at all.
    BadMagic,
    /// Written by a newer build of the encoder.
    UnsupportedVersion(
        /// The version the file claims.
        u32,
    ),
    /// The declared dimensions and the actual byte length disagree. Truncation
    /// is the usual cause, but a header edited by hand lands here too.
    SizeMismatch {
        /// Length the header implies.
        expected: usize,
        /// Length the file has.
        actual: usize,
    },
    /// The declared dimensions do not fit in memory on this machine, so the
    /// length check itself would overflow.
    TooLarge(
        /// The dimensions as declared.
        [u32; 3],
    ),
}

impl std::fmt::Display for GridError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GridError::Header { actual } => {
                write!(
                    f,
                    "too short to be a grid: {actual} bytes, header is {HEADER_LEN}"
                )
            }
            GridError::BadMagic => write!(f, "not a ShapeCAD grid: wrong magic"),
            GridError::UnsupportedVersion(v) => write!(
                f,
                "grid version {v} is newer than this build supports ({GRID_VERSION})"
            ),
            GridError::SizeMismatch { expected, actual } => write!(
                f,
                "grid is {actual} bytes but its dimensions need {expected}"
            ),
            GridError::TooLarge(d) => {
                write!(
                    f,
                    "grid dimensions {}x{}x{} do not fit in memory",
                    d[0], d[1], d[2]
                )
            }
        }
    }
}

impl std::error::Error for GridError {}

/// Why the sidecar directory could not be read or written.
#[derive(Debug)]
pub enum AssetError {
    /// A sidecar file could not be read or written.
    Io(
        /// The file involved.
        PathBuf,
        /// The underlying failure.
        std::io::Error,
    ),
    /// A sidecar file exists but is not a valid grid.
    Malformed {
        /// The file involved.
        path: PathBuf,
        /// What is wrong with it.
        cause: GridError,
    },
    /// The document references an asset the sidecar directory does not hold.
    /// A missing directory reports the first asset it was supposed to contain.
    Missing {
        /// The directory that was searched.
        dir: PathBuf,
        /// The asset that was not there.
        asset: AssetId,
    },
    /// The store was asked to write an asset it does not hold, which would mean
    /// a node references a grid that was never registered.
    Unregistered(
        /// The asset that is not in the store.
        AssetId,
    ),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            AssetError::Malformed { path, cause } => write!(f, "{}: {cause}", path.display()),
            AssetError::Missing { dir, asset } => {
                write!(f, "asset {} is missing from {}", asset.0, dir.display())
            }
            AssetError::Unregistered(a) => {
                write!(f, "asset {} is referenced but was never registered", a.0)
            }
        }
    }
}

impl std::error::Error for AssetError {}

/// The path a grid is stored at inside a sidecar directory.
///
/// Zero padded so a listing sorts in id order, which makes a directory of
/// assets as readable as the JSON that names them.
#[must_use]
pub fn grid_path(dir: &Path, id: AssetId) -> PathBuf {
    dir.join(format!("{:08}.{GRID_EXTENSION}", id.0))
}

/// The asset a sidecar file name refers to, or `None` if the name is not one of
/// ours. Used to spot files left behind by an earlier save.
fn id_from_name(name: &std::ffi::OsStr) -> Option<AssetId> {
    let name = name.to_str()?;
    let stem = name.strip_suffix(GRID_EXTENSION)?.strip_suffix('.')?;
    stem.parse().ok().map(AssetId)
}

/// Encodes a grid in the binary sidecar format.
///
/// Little endian throughout. Native endianness would be a portability bug
/// waiting to happen: the file would read back as noise on the other kind of
/// machine rather than failing.
#[must_use]
pub fn encode_grid(grid: &Grid) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + grid.data.len() * 4);
    out.extend_from_slice(&GRID_MAGIC);
    out.extend_from_slice(&GRID_VERSION.to_le_bytes());
    for d in grid.dims {
        out.extend_from_slice(&d.to_le_bytes());
    }
    for c in grid.origin.to_array() {
        out.extend_from_slice(&c.to_le_bytes());
    }
    out.extend_from_slice(&grid.spacing.to_le_bytes());
    for v in &grid.data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Decodes a grid written by [`encode_grid`].
///
/// Every field of the header is checked against the byte length before a single
/// sample is allocated, so a corrupt file is a typed error rather than a panic,
/// a partial grid, or a multi-gigabyte allocation taken on a stranger's word.
///
/// # Errors
/// [`GridError`] for a short file, wrong magic, unknown version, or dimensions
/// that disagree with the actual length.
pub fn decode_grid(bytes: &[u8]) -> Result<Grid, GridError> {
    if bytes.len() < HEADER_LEN {
        return Err(GridError::Header {
            actual: bytes.len(),
        });
    }
    if bytes[..8] != GRID_MAGIC {
        return Err(GridError::BadMagic);
    }
    let version = u32_at(bytes, 8);
    if version != GRID_VERSION {
        return Err(GridError::UnsupportedVersion(version));
    }

    let dims = [u32_at(bytes, 12), u32_at(bytes, 16), u32_at(bytes, 20)];
    let expected = expected_len(dims).ok_or(GridError::TooLarge(dims))?;
    if bytes.len() != expected {
        return Err(GridError::SizeMismatch {
            expected,
            actual: bytes.len(),
        });
    }

    let origin = sc_geom::glam::Vec3::new(f32_at(bytes, 24), f32_at(bytes, 28), f32_at(bytes, 32));
    let spacing = f32_at(bytes, 36);
    let data = bytes[HEADER_LEN..]
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();

    Ok(Grid {
        dims,
        origin,
        spacing,
        data,
    })
}

/// Total file length implied by a header's dimensions, or `None` on overflow.
fn expected_len(dims: [u32; 3]) -> Option<usize> {
    dims.iter()
        .try_fold(1usize, |n, &d| n.checked_mul(d as usize))?
        .checked_mul(4)?
        .checked_add(HEADER_LEN)
}

/// Reads a little-endian `u32`. The caller has already checked that the header
/// is present, and every offset used is inside it.
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Reads a little-endian `f32`, under the same precondition as [`u32_at`].
fn f32_at(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The grids a document owns, keyed by the id its nodes reference them by.
///
/// Grids are held behind an `Arc` because a `Mesh` node carries its grid inline
/// rather than looking it up: evaluation takes no asset context and must not
/// grow one. The store is therefore an index over the same allocations the
/// nodes hold, which is what makes it cheap to keep a grid alive for redo after
/// the node referencing it has been deleted.
#[derive(Clone, Debug, Default)]
pub struct AssetStore {
    grids: BTreeMap<AssetId, Arc<Grid>>,
    /// The next id to hand out. Never rewound, including by garbage collection,
    /// so an id that has been handed out is never handed out again and a stale
    /// reference cannot silently come to mean a different grid.
    next: u32,
}

impl AssetStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Issues a fresh id without storing anything against it.
    pub fn alloc(&mut self) -> AssetId {
        let id = AssetId(self.next);
        self.next += 1;
        id
    }

    /// Stores a grid under a fresh id.
    pub fn insert(&mut self, grid: Grid) -> AssetId {
        let id = self.alloc();
        self.grids.insert(id, Arc::new(grid));
        id
    }

    /// Stores a grid under an id that already exists, as on load and as when a
    /// node carrying a grid enters the arena. Advances the allocator past `id`
    /// so a later [`AssetStore::alloc`] cannot collide with it.
    pub fn insert_at(&mut self, id: AssetId, grid: Arc<Grid>) {
        self.next = self.next.max(id.0.saturating_add(1));
        self.grids.insert(id, grid);
    }

    /// The grid stored under `id`.
    #[must_use]
    pub fn get(&self, id: AssetId) -> Option<&Arc<Grid>> {
        self.grids.get(&id)
    }

    /// Whether a grid is stored under `id`.
    #[must_use]
    pub fn contains(&self, id: AssetId) -> bool {
        self.grids.contains_key(&id)
    }

    /// How many grids are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.grids.len()
    }

    /// Whether no grids are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.grids.is_empty()
    }

    /// Every id held, ascending.
    pub fn ids(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.grids.keys().copied()
    }

    /// Drops everything outside `keep`.
    ///
    /// The allocator is deliberately untouched: reissuing the id of a grid that
    /// has just been collected would make an agent-held or undo-held reference
    /// quietly resolve to different geometry.
    pub fn retain(&mut self, keep: &BTreeSet<AssetId>) {
        self.grids.retain(|id, _| keep.contains(id));
    }

    /// Writes the grids in `referenced` into `dir`, creating it if needed.
    ///
    /// Assets outside `referenced` are dead weight: nothing in the saved
    /// document can reach them, so writing them would put megabytes of
    /// unreachable binary next to every part. An empty `referenced` writes
    /// nothing and creates no directory, which is what keeps a part with no
    /// imported meshes to a single file.
    ///
    /// Removes nothing. Pairs with [`AssetStore::prune_dir`], which the caller
    /// runs *after* the document itself has been written, so that a save which
    /// fails part way through has not already deleted a grid the document still
    /// on disk refers to.
    ///
    /// # Errors
    /// [`AssetError::Unregistered`] if `referenced` names a grid the store does
    /// not hold, or [`AssetError::Io`] for any filesystem failure.
    pub fn write_dir(&self, dir: &Path, referenced: &BTreeSet<AssetId>) -> Result<(), AssetError> {
        if referenced.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(dir).map_err(|e| AssetError::Io(dir.to_path_buf(), e))?;
        for &id in referenced {
            let grid = self.grids.get(&id).ok_or(AssetError::Unregistered(id))?;
            let path = grid_path(dir, id);
            std::fs::write(&path, encode_grid(grid)).map_err(|e| AssetError::Io(path, e))?;
        }
        Ok(())
    }

    /// Deletes sidecar files for assets `referenced` does not name, then the
    /// directory itself if that leaves it empty.
    ///
    /// Without this, saving repeatedly over the same path accumulates every mesh
    /// the part ever had. The directory going away when the last asset does is
    /// what lets a part that once held a mesh become a single file again.
    ///
    /// Only files this module would have written are removed. Anything else the
    /// directory happens to contain is left alone, and its presence simply means
    /// the directory outlives the last asset. A directory that is not there is
    /// not an error: that is the ordinary case for a part with no meshes.
    ///
    /// # Errors
    /// [`AssetError::Io`] for any filesystem failure.
    pub fn prune_dir(dir: &Path, referenced: &BTreeSet<AssetId>) -> Result<(), AssetError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            // No directory is the normal case for a part with no meshes.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(AssetError::Io(dir.to_path_buf(), e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| AssetError::Io(dir.to_path_buf(), e))?;
            let Some(id) = id_from_name(&entry.file_name()) else {
                continue;
            };
            if !referenced.contains(&id) {
                let path = entry.path();
                std::fs::remove_file(&path).map_err(|e| AssetError::Io(path, e))?;
            }
        }
        if referenced.is_empty() {
            // Fails, harmlessly, if the user keeps anything else in there.
            let _ = std::fs::remove_dir(dir);
        }
        Ok(())
    }

    /// Loads exactly the grids in `required` from `dir`.
    ///
    /// Driven by what the document asks for rather than by what the directory
    /// happens to contain, so a stale file left by another tool is ignored
    /// rather than resurrected, and a grid the document needs but the directory
    /// lacks is an error rather than a silent hole.
    ///
    /// # Errors
    /// [`AssetError::Missing`] if a required grid, or the whole directory, is
    /// absent; [`AssetError::Malformed`] if a file is not a valid grid;
    /// [`AssetError::Io`] for any other filesystem failure.
    pub fn read_dir(dir: &Path, required: &BTreeSet<AssetId>) -> Result<Self, AssetError> {
        let mut store = Self::new();
        for &id in required {
            let path = grid_path(dir, id);
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(AssetError::Missing {
                        dir: dir.to_path_buf(),
                        asset: id,
                    })
                }
                Err(e) => return Err(AssetError::Io(path, e)),
            };
            let grid =
                decode_grid(&bytes).map_err(|cause| AssetError::Malformed { path, cause })?;
            store.insert_at(id, Arc::new(grid));
        }
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_geom::glam::Vec3;

    fn grid(dims: [u32; 3]) -> Grid {
        let n = (dims[0] * dims[1] * dims[2]) as usize;
        Grid {
            dims,
            origin: Vec3::new(-1.5, 2.0, 0.25),
            spacing: 0.5,
            data: (0..n).map(|i| i as f32 * 0.125 - 3.0).collect(),
        }
    }

    #[test]
    fn a_grid_round_trips_through_the_binary_format() {
        let g = grid([3, 4, 5]);
        let bytes = encode_grid(&g);
        assert_eq!(bytes.len(), HEADER_LEN + 3 * 4 * 5 * 4);
        let back = decode_grid(&bytes).unwrap();
        assert_eq!(back, g, "a grid changed passing through its own encoder");
        assert_eq!(
            encode_grid(&back),
            bytes,
            "re-encoding was not byte identical"
        );
    }

    /// Endianness is pinned by the format, not inherited from the machine, so
    /// the first bytes of the header are fixed and checkable.
    #[test]
    fn the_header_is_little_endian_and_starts_with_the_magic() {
        let bytes = encode_grid(&grid([2, 1, 1]));
        assert_eq!(&bytes[..8], b"SCSDF\0\0\0");
        assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &2u32.to_le_bytes());
    }

    #[test]
    fn a_truncated_grid_is_rejected() {
        let bytes = encode_grid(&grid([4, 4, 4]));
        let cut = &bytes[..bytes.len() - 8];
        assert_eq!(
            decode_grid(cut),
            Err(GridError::SizeMismatch {
                expected: bytes.len(),
                actual: bytes.len() - 8
            })
        );
    }

    #[test]
    fn a_grid_shorter_than_its_header_is_rejected() {
        assert_eq!(
            decode_grid(&GRID_MAGIC),
            Err(GridError::Header { actual: 8 })
        );
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut bytes = encode_grid(&grid([2, 2, 2]));
        bytes[0] = b'X';
        assert_eq!(decode_grid(&bytes), Err(GridError::BadMagic));
    }

    #[test]
    fn a_newer_grid_version_is_rejected() {
        let mut bytes = encode_grid(&grid([2, 2, 2]));
        bytes[8..12].copy_from_slice(&(GRID_VERSION + 1).to_le_bytes());
        assert_eq!(
            decode_grid(&bytes),
            Err(GridError::UnsupportedVersion(GRID_VERSION + 1))
        );
    }

    /// Dimensions are read from the file, so a corrupt header must not be
    /// allowed to drive an allocation before the length agrees with it.
    #[test]
    fn absurd_dimensions_do_not_allocate() {
        let mut bytes = encode_grid(&grid([2, 2, 2]));
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            decode_grid(&bytes),
            Err(GridError::TooLarge(_) | GridError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn ids_are_never_reissued_after_collection() {
        let mut store = AssetStore::new();
        let a = store.insert(grid([1, 1, 1]));
        let b = store.insert(grid([1, 1, 1]));
        assert_ne!(a, b);
        store.retain(&BTreeSet::new());
        assert!(store.is_empty());
        let c = store.insert(grid([1, 1, 1]));
        assert_ne!(c, a, "a collected id was handed out again");
        assert_ne!(c, b, "a collected id was handed out again");
    }

    #[test]
    fn inserting_at_an_id_advances_the_allocator_past_it() {
        let mut store = AssetStore::new();
        store.insert_at(AssetId(7), Arc::new(grid([1, 1, 1])));
        assert_eq!(store.alloc(), AssetId(8));
    }
}
