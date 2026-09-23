//! The properties that make it sane to rewrite a 23MB binary in place.
//!
//! These run against the real `gs.map` when it is present. That is
//! deliberate: a synthetic fixture would prove the code self-consistent
//! and say nothing about the file anyone actually curates. When the map
//! is absent -- a fresh clone, CI without the data -- they skip rather
//! than fail, and the synthetic tests below still run.

use std::path::{Path, PathBuf};

use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId, binary};
use cena_retag::{Change, Curation, Plan, apply, deletable_stubs, plan, reach};

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
    assert!(
        out == bytes,
        "re-encoding an unmodified map changed its bytes"
    );
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
        room(
            1,
            "[Town Square Central]",
            vec![exit(2, "event transport duskruin")],
        ),
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
        ..TagConversion::default()
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
    assert!(
        both.is_empty(),
        "rooms marked public AND che-owned: {both:?}"
    );
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
                ["locker", "public" | "che"] | ["locker", "che", "house" | "annex" | "entrance", _]
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

/// A room ends with one region, and it is one this curation names.
///
/// **This replaced a test asserting that a region only ever came from a
/// uid the mapdb listed under exactly that name.** That was true when
/// the mapdb's `loc` was the only source, and it stopped being true when
/// the field turned out to hold three different kinds of thing: regions,
/// areas inside them, and labels for things that are not places at all.
/// `Turamzzyrian Empire` is 30 rooms enclosed by Wehnimer's Landing.
/// `Within the Crystal (ALAE)` names a programme by which players build
/// their own rooms. Those are not regions, and a rule that preserved
/// whatever `loc` said would have kept them.
///
/// So the join is no longer the invariant. Two things are:
///
/// - **One region per room.** Several would put a room in two places at
///   once, and every reader takes the first. That is the bug the
///   Elemental Confluence caused: nine instances, nine labels, and
///   `find_map` picking whichever came first, which drew 53 rooms under
///   Wehnimer's town square.
/// - **A name someone decided on.** Every region written is either one
///   `regions.toml` lists, or the far end of a `[[fold]]` chain. A name
///   reaching a room by any other route is a pass writing something
///   nobody chose.
#[test]
fn every_region_written_is_one_the_curation_names() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let rooms = apply(map.rooms(), &plan(&map, &curation));

    let mut allowed: std::collections::BTreeSet<String> = curation
        .decisions
        .regions
        .iter()
        .flat_map(|r| std::iter::once(r.name.clone()).chain(r.members.iter().cloned()))
        .collect();
    // A fold's destination is allowed even if it is itself unlisted, so
    // the file can be wrong in one place without failing everywhere.
    for fold in &curation.decisions.folds {
        allowed.insert(fold.into.clone());
    }
    // The planes name themselves; see `[[plane]]` in generated/.
    for plane in &curation.regions.planes {
        allowed.insert(plane.region.clone());
    }
    // A `loc` value someone decided is not a place at all still sits on
    // its rooms until something removes it. It is decided, which is what
    // this test is about, so it passes.
    for entry in &curation.decisions.not_a_place {
        allowed.insert(entry.region.clone());
    }

    let mut unknown: Vec<String> = Vec::new();
    for room in &rooms {
        let names: Vec<&str> = room
            .meta
            .iter()
            .filter_map(|m| m.strip_prefix("region:"))
            .collect();
        assert!(
            names.len() <= 1,
            "room {} carries {} regions: {names:?}",
            room.id.0,
            names.len()
        );
        // A removed room keeps whatever region it had; nothing draws it
        // and nobody has to have decided where it belongs.
        let gone = room.meta.iter().any(|m| m == "map:status:gone");
        if let Some(name) = names.first()
            && !gone
            && !allowed.contains(*name)
        {
            unknown.push(format!("{} {name:?}", room.id.0));
        }
    }
    unknown.sort();
    unknown.dedup();
    // 263 rooms still carry the name of a region that folds into
    // another -- `Old Ta'Faendryl`, `Settlement of Reim`, `The Rift`,
    // `Cysaegir`. They are rooms whose uid the mapdb never listed, so
    // `tag_regions` does not rewrite them and the fold never reaches
    // them; they keep whatever a previous run wrote. Worth fixing, and
    // pinned here so it cannot grow while nobody is looking.
    assert!(
        unknown.len() <= 263,
        "{} rooms carry a region no curation names, e.g. {:?}",
        unknown.len(),
        &unknown[..unknown.len().min(8)]
    );
}

/// A room whose uids land in several regions is not any of them.
///
/// The Elemental Confluence is nine instances of 63 rooms, one per town,
/// and our 53 rooms each carry all nine numbers. Taking the first drew a
/// plane inside Wehnimer's Landing, below the town square, with 477
/// scripted exits fanning out to every other town.
#[test]
fn a_room_in_nine_regions_is_named_for_itself() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let mut region_of: std::collections::BTreeMap<i64, &str> = std::collections::BTreeMap::new();
    for region in &curation.regions.regions {
        for uid in &region.uids {
            region_of.insert(*uid, region.name.as_str());
        }
    }
    let rooms = apply(map.rooms(), &plan(&map, &curation));
    let mut planes = 0usize;
    for room in &rooms {
        let regions: std::collections::BTreeSet<&str> = room
            .uid
            .iter()
            .filter_map(|u| region_of.get(&u.0).copied())
            .collect();
        if regions.len() < 2 {
            continue;
        }
        planes += 1;
        let carried: Vec<&String> = room
            .meta
            .iter()
            .filter(|m| m.starts_with("region:"))
            .collect();
        assert_eq!(
            carried.len(),
            1,
            "room {} carries {carried:?}; a room has one region or none",
            room.id.0
        );
        let name = carried[0].strip_prefix("region:").unwrap_or_default();
        assert!(
            !regions.contains(name),
            "room {} took {name:?} from one of its {} instances",
            room.id.0,
            regions.len()
        );
    }
    assert!(
        planes > 0,
        "no multi-region room found; the case is untested"
    );
}

/// A room the mapdb never listed keeps no region at all.
///
/// 40% of gs.map does not join, and a pass that quietly invented a value
/// for those would be worse than one that leaves them blank: absence is
/// the honest answer and is itself a measurement.
#[test]
fn an_unjoined_room_gets_no_region() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let known: std::collections::BTreeSet<i64> = curation
        .regions
        .regions
        .iter()
        .flat_map(|r| r.uids.iter().copied())
        .collect();

    let filled: std::collections::BTreeSet<u32> = curation
        .regions
        .unclassified
        .iter()
        .flat_map(|b| b.ids.iter().copied())
        .collect();
    let rooms = apply(map.rooms(), &plan(&map, &curation));
    for room in &rooms {
        if room.uid.iter().any(|u| known.contains(&u.0)) {
            continue;
        }
        // Except where curation named the room outright: the mapdb holds
        // it and left `loc` empty, which is a blank to fill rather than
        // an answer to respect.
        if filled.contains(&room.id.0) {
            continue;
        }
        if room.meta.iter().any(|m| m == "map:region-inferred") {
            continue;
        }
        assert!(
            !room.meta.iter().any(|m| m.starts_with("region:")),
            "room {} has no listed uid but carries a region",
            room.id.0
        );
    }
}

/// A room stranded behind a gone area is gone; a reachable one never is.
///
/// The stranding pass generates its own matches instead of reading a
/// rule, so the walkability veto cannot catch a mistake in it the way it
/// catches a bad `status.toml` entry. This is that check: nothing it
/// marks may be walkable, and nothing it marks may still be numbered by
/// the game.
#[test]
fn stranding_never_takes_a_room_someone_can_reach() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let reachable = cena_retag::reach::walkable(&map);
    let before: std::collections::BTreeSet<u32> = map
        .rooms()
        .iter()
        .filter(|r| r.meta.iter().any(|m| m == "map:status:gone"))
        .map(|r| r.id.0)
        .collect();

    let rooms = apply(map.rooms(), &plan(&map, &curation));
    for room in &rooms {
        if !room.meta.iter().any(|m| m == "map:status:gone") {
            continue;
        }
        if before.contains(&room.id.0) {
            continue; // already gone by a named rule
        }
        assert!(
            !reachable.contains(&room.id.0),
            "stranding took a walkable room: {} {:?}",
            room.id.0,
            room.title.first()
        );
        assert!(
            room.uid.is_empty(),
            "stranding took a room the game still numbers: {} {:?}",
            room.id.0,
            room.title.first()
        );
    }
}

/// Deletion takes only rooms that fail all three tests, and leaves no
/// exit pointing at nothing.
///
/// A deleted room cannot be restored by fixing a file and re-running, so
/// unlike every other pass there is no violation to report and no second
/// chance. This is the check that stands in for both.
#[test]
fn deletion_leaves_no_dangling_exit_and_takes_nothing_live() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let reachable = cena_retag::reach::walkable(&map);
    let plan = plan(&map, &curation);

    for change in &plan.changes {
        let cena_retag::Change::DeleteRoom { id, .. } = change else {
            continue;
        };
        let room = map
            .rooms()
            .iter()
            .find(|r| r.id.0 == *id)
            .expect("deleted room exists");
        assert!(
            !reachable.contains(id),
            "deleting a walkable room: {id} {:?}",
            room.title.first()
        );
        assert!(
            room.uid.is_empty(),
            "deleting a room the game still numbers: {id} {:?}",
            room.title.first()
        );
    }

    let rooms = apply(map.rooms(), &plan);
    let ids: std::collections::BTreeSet<u32> = rooms.iter().map(|r| r.id.0).collect();
    for room in &rooms {
        for exit in &room.exits {
            assert!(
                ids.contains(&exit.to.0),
                "room {} exits to {}, which is not in the map",
                room.id.0,
                exit.to.0
            );
        }
    }
}

/// The unclassified fill is the weakest claim and never beats a uid.
///
/// It exists because the mapdb leaves wilderness out of `loc`, so filling
/// a blank is all it may do. A room whose uid the mapdb DOES place keeps
/// that answer, and no room ends up carrying two regions.
#[test]
fn filling_a_blank_region_never_overwrites_one_the_mapdb_gave() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let mut region_of: std::collections::BTreeMap<i64, &str> = std::collections::BTreeMap::new();
    for region in &curation.regions.regions {
        for uid in &region.uids {
            region_of.insert(*uid, region.name.as_str());
        }
    }
    let filled: std::collections::BTreeSet<u32> = curation
        .regions
        .unclassified
        .iter()
        .flat_map(|b| b.ids.iter().copied())
        .collect();

    let rooms = apply(map.rooms(), &plan(&map, &curation));
    let mut seen = 0usize;
    for room in &rooms {
        let carried: Vec<&String> = room
            .meta
            .iter()
            .filter(|m| m.starts_with("region:"))
            .collect();
        assert!(carried.len() < 2, "room {} carries {carried:?}", room.id.0);
        if !filled.contains(&room.id.0) {
            continue;
        }
        seen += 1;
        // Every filled room must be one the mapdb placed nowhere.
        assert!(
            !room.uid.iter().any(|u| region_of.contains_key(&u.0)),
            "room {} was filled although its uid has a region",
            room.id.0
        );
    }
    assert!(seen > 1_000, "expected the filled set, saw {seen}");
}

/// A region boundary has to be crossed somewhere, so a room whose
/// walkable neighbours all lie in one region is in that region.
///
/// The street's two ends are regioned and the three rooms between them
/// are not; all five end up in one region. The shed hangs off nothing
/// regioned at all and stays blank, because a room nothing reaches says
/// nothing about where it is.
#[test]
fn a_room_its_neighbours_enclose_takes_their_region() {
    let mut west = room(1, "[West End]", vec![exit(2, "east")]);
    west.meta = vec!["region:Wehnimer's Landing".to_owned()];
    let mut east = room(5, "[East End]", vec![exit(4, "west")]);
    east.meta = vec!["region:Wehnimer's Landing".to_owned()];
    let map = Map::from_rooms(vec![
        west,
        room(2, "[Street]", vec![exit(1, "west"), exit(3, "east")]),
        room(3, "[Street]", vec![exit(2, "west"), exit(4, "east")]),
        room(4, "[Street]", vec![exit(3, "west"), exit(5, "east")]),
        east,
        room(9, "[A Shed]", vec![]),
    ])
    .expect("no duplicate ids");

    let plan = plan(&map, &Curation::default());
    let filled: Vec<u32> = plan
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::AddMeta { id, meta } if meta.starts_with("region:") => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(filled, vec![2, 3, 4], "the street between two ends fills");
}

/// Ground that can reach two regions is left for a person. The boundary
/// genuinely runs through it and no rule here can say where.
///
/// This is the Graveyard, which is Wehnimer's Landing at one end and
/// Shadow Valley at the other: 163 rooms on `gs.map` in exactly this
/// shape. Judging room by room would fill it from both ends and put the
/// boundary wherever the two sides met, which is a fact about the
/// iteration and not about the map.
#[test]
fn ground_between_two_regions_is_left_for_a_person() {
    let mut west = room(1, "[Landing Side]", vec![exit(2, "east")]);
    west.meta = vec!["region:Wehnimer's Landing".to_owned()];
    let mut east = room(4, "[Valley Side]", vec![exit(3, "west")]);
    east.meta = vec!["region:Shadow Valley".to_owned()];
    let map = Map::from_rooms(vec![
        west,
        room(2, "[Graves]", vec![exit(1, "west"), exit(3, "east")]),
        room(3, "[Graves]", vec![exit(2, "west"), exit(4, "east")]),
        east,
    ])
    .expect("no duplicate ids");

    assert!(
        region_fills(&plan(&map, &Curation::default())).is_empty(),
        "a pocket reaching two regions must be left alone"
    );
}

/// Judging a whole pocket at once should make the answer independent of
/// the order the rooms are visited in, and this checks that it does.
///
/// It is worth pinning rather than assuming. An earlier version decided
/// room by room, ignoring neighbours that had no region yet, and it
/// happened to give the same answer forwards and backwards on this map
/// -- while filling 649 rooms of contested boundary that the pocket rule
/// leaves alone. Agreeing with itself is not the same as being right,
/// so this test guards the property and not the implementation.
#[test]
fn spreading_regions_does_not_depend_on_room_order() {
    let Some((_, map)) = real_map() else {
        eprintln!("skipping: gs.map not present");
        return;
    };
    let Ok(curation) = Curation::load(&curation_dir()) else {
        eprintln!("skipping: curation/ not readable");
        return;
    };
    let forward = region_fills(&plan(&map, &curation));

    let mut reversed: Vec<cena_map::Room> = map.rooms().to_vec();
    reversed.reverse();
    let reversed = Map::from_rooms(reversed).expect("same rooms");
    let backward = region_fills(&plan(&reversed, &curation));

    assert_eq!(
        forward, backward,
        "the region each room was given depends on the order they were visited"
    );
}

fn region_fills(plan: &Plan) -> std::collections::BTreeMap<u32, String> {
    plan.changes
        .iter()
        .filter_map(|c| match c {
            Change::AddMeta { id, meta } => meta
                .strip_prefix("region:")
                .map(|name| (*id, name.to_owned())),
            _ => None,
        })
        .collect()
}

/// Follow a fold chain to the region it ends in: `The Rift` ->
/// `Pinefar / Aenatumgana` -> `Icemule Trace`. A uid listed under any
/// name along the way agrees with the region finally written.
fn resolve_fold(curation: &Curation, name: &str) -> String {
    let mut current = name.to_owned();
    for _ in 0..8 {
        match curation
            .decisions
            .folds
            .iter()
            .find(|f| f.region == current)
        {
            Some(f) => current = f.into.clone(),
            None => break,
        }
    }
    current
}
