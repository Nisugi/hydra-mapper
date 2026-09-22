//! The properties that make it sane to rewrite a 23MB binary in place.
//!
//! These run against the real `gs.map` when it is present. That is
//! deliberate: a synthetic fixture would prove the code self-consistent
//! and say nothing about the file anyone actually curates. When the map
//! is absent -- a fresh clone, CI without the data -- they skip rather
//! than fail, and the synthetic tests below still run.

use std::path::{Path, PathBuf};

use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId, binary};
use cena_retag::{Curation, apply, deletable_stubs, plan, reach};

fn map_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../gs.map")
}

fn curation_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../curation")
}

fn real_map() -> Option<(Vec<u8>, Map)> {
    let bytes = std::fs::read(map_path()).ok()?;
    let map = binary::decode(&bytes).ok()?;
    Some((bytes, map))
}

/// The foundation. If re-encoding an untouched map is not byte-identical,
/// every diff this tool produces is indistinguishable from corruption and
/// none of the other tests mean anything.
#[test]
fn an_untouched_map_re_encodes_byte_for_byte() {
    let Some((bytes, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let out = binary::encode(&map).expect("re-encode");
    assert_eq!(out.len(), bytes.len(), "size changed");
    assert!(out == bytes, "re-encoding an unmodified map changed its bytes");
}

/// Applying twice must equal applying once. Without this, `apply` is not
/// safe to re-run, and a tool you cannot re-run is one you cannot trust
/// after a map rebuild.
#[test]
fn applying_is_idempotent() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };

    let first = plan(&map, &curation);
    let once = Map::from_rooms(apply(map.rooms(), &first)).expect("rebuild");

    let second = plan(&once, &curation);
    assert!(
        second.changes.is_empty(),
        "second run still wants {} changes, e.g. {}",
        second.changes.len(),
        second
            .changes
            .first()
            .map_or(String::new(), ToString::to_string)
    );

    let twice = Map::from_rooms(apply(once.rooms(), &second)).expect("rebuild");
    assert_eq!(once.rooms(), twice.rooms(), "second apply changed rooms");
}

/// The curation that ships must be clean: no rule may mark a room someone
/// can walk to as `closed` or `gone`. This is the check that caught the
/// Arena of the Abyss sitting inside a `gone` Caligos Isle.
#[test]
fn the_shipped_curation_has_no_violations() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let plan = plan(&map, &curation);
    assert!(
        plan.violations.is_empty(),
        "{} rule(s) would hide a walkable room: {}",
        plan.violations.len(),
        plan.violations
            .iter()
            .map(|v| format!("{} ({} walkable)", v.rule, v.reachable.len()))
            .collect::<Vec<_>>()
            .join("; ")
    );
}

/// A rule that names nothing is a typo or a place the map lost. Not fatal
/// at runtime, but the shipped file should have none.
#[test]
fn the_shipped_curation_has_no_dead_rules() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let plan = plan(&map, &curation);
    assert!(
        plan.empty_rules.is_empty(),
        "rules matching nothing: {}",
        plan.empty_rules.join("; ")
    );
}

fn room(id: u32, title: &str, exits: Vec<Exit>) -> Room {
    Room {
        id: RoomId(id),
        uid: vec![],
        title: vec![title.to_owned()],
        description: vec![],
        paths: vec![],
        location: None,
        location_unknowable: false,
        check_location: false,
        unique_loot: vec![],
        climate: None,
        terrain: None,
        tags: vec![],
        meta: vec![],
        image: None,
        exits,
    }
}

fn exit(to: u32, command: &str) -> Exit {
    Exit {
        to: RoomId(to),
        kind: ExitKind::Cardinal,
        crossing: Crossing::Command(command.to_owned()),
        cost: Some(Cost::Fixed(1.0)),
    }
}

/// A stub with a description is a room someone visited, not bookkeeping.
///
/// The ten Ta'Vaalor rooms titled `["duplicate of 3485", "[Ta'Vaalor,
/// Amaranth Court]"]` are day/night variants: "nearly deserted" against
/// "teems with people". Deleting on the title alone would take live
/// streets with it.
#[test]
fn a_described_duplicate_is_kept() {
    let mut stub = room(2, "duplicate of 1", vec![]);
    let mut variant = room(3, "duplicate of 1", vec![]);
    variant.description = vec!["The street is nearly deserted.".to_owned()];
    stub.description = vec![];

    let map = Map::from_rooms(vec![room(1, "[A Street]", vec![]), stub, variant])
        .expect("no duplicate ids");

    let deletable = deletable_stubs(&map);
    assert!(deletable.contains(&2), "undescribed stub should be deleted");
    assert!(
        !deletable.contains(&3),
        "a duplicate with a description is a day/night variant, not a stub"
    );
}

/// A stub pointed at by a real room is left alone. Nothing in `gs.map`
/// trips this today; the test pins the behaviour for the rebuild that
/// does.
#[test]
fn a_stub_a_live_room_points_at_is_kept() {
    let map = Map::from_rooms(vec![
        room(1, "[A Street]", vec![exit(2, "north")]),
        room(2, "duplicate of 1", vec![]),
        room(3, "duplicate of 1", vec![]),
    ])
    .expect("no duplicate ids");

    let deletable = deletable_stubs(&map);
    assert!(!deletable.contains(&2), "a live room points at 2");
    assert!(deletable.contains(&3), "nothing points at 3");
}

/// Stubs that only point at each other are still deletable: the five such
/// rooms in `gs.map` form an island nothing live touches, and requiring
/// total isolation would strand them.
#[test]
fn stubs_pointing_at_each_other_are_still_deletable() {
    let map = Map::from_rooms(vec![
        room(1, "[A Street]", vec![]),
        room(2, "duplicate of 1", vec![exit(3, "east")]),
        room(3, "duplicate of 1", vec![exit(2, "west")]),
    ])
    .expect("no duplicate ids");

    let deletable = deletable_stubs(&map);
    assert!(deletable.contains(&2) && deletable.contains(&3));
}

/// An `event transport` edge is in the map but cannot be walked out of
/// season, so it must not make an event area look live.
#[test]
fn a_seasonal_edge_is_not_walkable() {
    let map = Map::from_rooms(vec![
        room(1, "[Town Square Central]", vec![exit(2, "event transport duskruin")]),
        room(2, "[Bloodriven Village, River Bank]", vec![]),
    ])
    .expect("no duplicate ids");

    let walkable = reach::walkable(&map);
    assert!(walkable.contains(&1), "the town square is the seed");
    assert!(
        !walkable.contains(&2),
        "an event transport edge only works while the event runs"
    );
}

/// An ordinary edge from a town centre is walkable, gates and all: the
/// question is whether *someone* can get there.
#[test]
fn an_ordinary_edge_is_walkable() {
    let map = Map::from_rooms(vec![
        room(1, "[Town Square Central]", vec![exit(2, "north")]),
        room(2, "[A Shop]", vec![]),
    ])
    .expect("no duplicate ids");

    assert!(reach::walkable(&map).contains(&2));
}

/// The walkability veto is waived only where curation says so.
///
/// A dead shop you can still walk into is `closed`: the tag describes the
/// shop, and the open door is a forgotten lock. But the waiver must be
/// opt-in, or the veto stops catching the case it exists for.
#[test]
fn allow_walkable_waives_the_veto_but_only_when_asked() {
    use cena_retag::rules::TagConversion;

    let mut shop = room(2, "[Ilyra's Fashions]", vec![]);
    shop.tags = vec!["closed".to_owned()];

    let strict = TagConversion {
        from_tag: "closed".to_owned(),
        to_status: "closed".to_owned(),
        only_if_unwalkable: true,
        allow_walkable: false,
        only_if_walkable: false,
        require_uid: false,
        require_no_uid: false,
        require_exits: false,
        note: None,
    };
    let waived = TagConversion {
        allow_walkable: true,
        ..strict.clone()
    };

    assert!(
        !strict.applies(&shop, true),
        "by default a walkable room is live and takes no closed status"
    );
    assert!(
        waived.applies(&shop, true),
        "allow_walkable is the deliberate exception for dead shops"
    );
    assert!(
        strict.applies(&shop, false),
        "an unwalkable closed room converts either way"
    );
}

/// Bare `meta:locker` means "is a locker", not "is public".
///
/// 112 of its 164 rooms also carry a `che:*` key and are house vaults.
/// Mapping it to `locker:public` unguarded marks [Paupers, Vault] public.
#[test]
fn a_che_room_is_never_marked_a_public_locker() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let rooms = apply(map.rooms(), &plan(&map, &curation));
    let both: Vec<u32> = rooms
        .iter()
        .filter(|r| {
            r.meta.iter().any(|m| m == "locker:public")
                && r.meta.iter().any(|m| m.starts_with("locker:che"))
        })
        .map(|r| r.id.0)
        .collect();
    assert!(both.is_empty(), "rooms marked public AND che-owned: {both:?}");
}

/// Every `locker:*` key is one of the five shapes, with exactly one
/// segment for the house.
///
/// The guard that matters: `che:{House}` will otherwise match
/// `che:paupers:locker` and call the house "paupers:locker", producing
/// `locker:che:house:paupers:locker`.
#[test]
fn every_locker_key_is_well_formed() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let rooms = apply(map.rooms(), &plan(&map, &curation));
    let mut bad: Vec<String> = Vec::new();
    for room in &rooms {
        for meta in &room.meta {
            if !meta.starts_with("locker") {
                continue;
            }
            let segments: Vec<&str> = meta.split(':').collect();
            let ok = matches!(
                segments.as_slice(),
                ["locker", "public"]
                    | ["locker", "che"]
                    | ["locker", "che", "house", _]
                    | ["locker", "che", "annex", _]
                    | ["locker", "che", "entrance", _]
            );
            if !ok {
                bad.push(meta.clone());
            }
        }
    }
    bad.sort();
    bad.dedup();
    assert!(bad.is_empty(), "malformed locker keys: {bad:?}");
}
