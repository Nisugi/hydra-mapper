//! The seam for hand-curated corrections — **not built in v1** (`plan/26`
//! §0: explorer first, no write-back). Vellum's own `overrides.rs` (spec §8)
//! is the reference for what this becomes: position pins, edge overrides,
//! classification flips, names — all uid-keyed, applied after generation so
//! a cached layout stays reusable across a pure presentation edit.
//!
//! [`EdgeOverride`] exists only so [`crate::direction::DirectionMap::
//! apply_edge_overrides`] has a real type to take; nothing constructs one
//! yet.

/// Placeholder for a curated edge correction. Uninhabited until the override
/// system is designed; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeOverride {}
