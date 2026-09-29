//! The map Hydra ships: this repository's `gs.map`, in the bytes the
//! mapper wrote (the author, 2026-09-29: *"Hydra is going to embed the
//! gs.map from the mapper"*). Decode it with `cena_map::binary`.

/// The whole of `gs.map`, as the mapper last wrote it.
pub static GS_MAP: &[u8] = include_bytes!("../../../gs.map");

#[cfg(test)]
mod tests {
    /// The embedded file is a map, not a stub or an empty file.
    #[test]
    fn it_is_a_map() {
        assert!(super::GS_MAP.starts_with(b"HYDRAMAP"));
        assert!(super::GS_MAP.len() > 1_000_000);
    }
}
