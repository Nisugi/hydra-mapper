//! The map, built into the binary, for a mapper handed to someone else.
//!
//! **One file to hand out.** With the `bundled-map` feature the build
//! embeds `gs.map` from the repository root -- the baked curation,
//! areas and regions included -- so a person working on the map needs
//! nothing but the executable. Their edits go in a store beside it, and
//! "Export my changes" writes just those edits to send back.
//!
//! ```text
//! cargo build --release -p cena-mapper --features bundled-map
//! ```
//!
//! Build it from a freshly baked map (`cena-mapper --export-curation`,
//! then `retag apply`): what is embedded is what everyone's edits sit on
//! top of.

use std::path::PathBuf;

/// The embedded map, when this build has one.
#[cfg(feature = "bundled-map")]
pub const MAP: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../gs.map"
)));

/// The embedded map, when this build has one.
#[cfg(not(feature = "bundled-map"))]
pub const MAP: Option<&[u8]> = None;

/// Where a bundled mapper's map "is": `gs.map` beside the executable.
///
/// Nothing is read from there unless someone puts a map there -- the
/// embedded one is used instead -- but the store path is derived from it,
/// so a contributor's edits land in `gs.overrides.json` next to the exe.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    MAP?;
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name("gs.map"))
}
