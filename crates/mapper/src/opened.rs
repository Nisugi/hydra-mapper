//! `--open-places [map]`: every place left off an area's sheet, opened on
//! it where its dot is ([`cena_map_layout::open`]), and how many fit
//! without breaking a rule, by size, with those that do not listed.
//! Hydra opens a place this way when a character walks in (its
//! `plan/53` §8a).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use cena_map::Map;

/// Open every place of the map at `path` and say how it went.
///
/// # Errors
///
/// When the map will not load.
pub fn open_places_headless(path: Option<&Path>) -> Result<String, String> {
    let map = crate::app::load_map(path).map_err(|p| p.to_string())?;
    let placeable = cena_map_layout::regions::placeable_rooms(&map);
    let mut sizes: BTreeMap<usize, (usize, usize, &str)> = BTreeMap::new();
    let (mut apart, mut unfitted) = (0, Vec::new());
    for (name, rooms) in cena_map_layout::areas::baked(&map) {
        let rooms = cena_map_layout::areas::layout_rooms(&rooms, &map, &placeable);
        let Ok(subset) = Map::from_rooms(rooms) else {
            continue;
        };
        let layout = cena_map_layout::generate_layout(&subset);
        let area = cena_map_layout::build_scene(&name, &layout, &subset);
        let hidden = cena_map_layout::hidden::hidden_rooms(&subset);
        for place in cena_map_layout::hidden::place_rooms(&subset, &hidden) {
            let rooms = place
                .iter()
                .filter_map(|id| subset.room(*id).cloned())
                .collect();
            let Ok(alone) = Map::from_rooms(rooms) else {
                continue;
            };
            let laid = cena_map_layout::generate_layout(&alone);
            let own = cena_map_layout::build_scene(&name, &laid, &alone);
            let (bucket, label) = match place.len() {
                1 => (0, "1 room"),
                2..=3 => (1, "2-3 rooms"),
                4..=10 => (2, "4-10 rooms"),
                _ => (3, "11+ rooms"),
            };
            let row = sizes.entry(bucket).or_insert((0, 0, label));
            match cena_map_layout::open::open_place(&area, &own, layout.town_scale) {
                Some(opened) if opened.fitted => row.0 += 1,
                Some(_) => {
                    row.1 += 1;
                    unfitted.push(format!(
                        "{name}: {} rooms, room {} first",
                        place.len(),
                        place[0].0
                    ));
                }
                None => apart += 1,
            }
        }
    }
    let mut out = String::from("size\tfits\tbreaks a rule\n");
    for (fits, breaks, label) in sizes.values() {
        let _ = writeln!(out, "{label}\t{fits}\t{breaks}");
    }
    let _ = writeln!(
        out,
        "{apart} places have no dot on their area's sheet and keep their own"
    );
    for line in &unfitted {
        let _ = writeln!(out, "  {line}");
    }
    Ok(out)
}
