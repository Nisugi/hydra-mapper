//! Map layout engine -- ported from `reference/VellumFE/src/core/
//! layout_engine/` (`plan/26`).
//!
//! Generates a 2D grid layout for one *location* of the map: direction
//! analysis -> BFS component placement with grid rips -> per-component
//! hill-climb + compaction -> interior classification -> cluster packing
//! (outdoor sheet) + interior shelf. The pipeline itself is `pipeline`
//! (`plan/05` Rule 4.4: this file re-exports and wires, it does not
//! implement).
//!
//! Pure: no file I/O, no rendering toolkit, no window (`plan/26` §2) -- a
//! future Hydra GUI panel depends on this crate directly, with no coupling
//! to whichever app draws it first. `cena-mapper` is that first app; this
//! crate knows nothing about it.
//!
//! **World elevation is out of scope.** `plan/26` §0 records why: render
//! placement (which plate a room draws on) is a room-graph packing problem
//! decided by cell collision, edge direction and crossings, not a floor
//! number. Up/down borrow the north/south offsets in the plane, same as
//! Vellum's own engine (spec §10, "out of scope: up/down layering").
//!
//! **v1 renders; it does not edit.** The override system (position pins,
//! edge overrides, classification flips) that lets a person hand-correct a
//! bad layout is not ported yet; see `overrides` and `plan/26` §0.

// `implicit_hasher` asks internal `HashMap`/`HashSet` parameters to
// generalise over `BuildHasher`. Every one it flags in this crate is fed
// straight from another function's own `std`-hasher output earlier in this
// same pipeline (`interior_clusters`'s map, `classify`'s set) -- there is no
// caller anywhere that could or would supply a different hasher, so the
// bound would be dead flexibility, not real API surface. Scoped here rather
// than per-site because the reasoning is the same at every occurrence.
#![allow(clippy::implicit_hasher)]

pub mod classifier;
pub mod direction;
pub mod interior_shelf;
pub mod outdoor_packing;
pub mod overrides;
pub mod packer;
pub mod pipeline;
pub mod positioner;
pub mod scene;
pub mod stats;

pub use classifier::Classification;
pub use packer::PackInfo;
pub use pipeline::{Layout, generate_layout, generate_layout_reference};
pub use positioner::{Cell, Group, PackMethod, Violation};
pub use scene::{MapScene, Sheet, SheetScene, build_scene};
pub use stats::LayoutStats;
