# hydra-mapper

The map layout engine and standalone map explorer for **Hydra** (working
name **Cena**, the game client this feeds): computes a 2D room-graph
layout from a `.map` file and renders it, for the (large) share of the
GemStone IV map that has no hand-drawn artwork to fall back on.

This is a separate repo from Cena/Hydra itself, the same way
[`Nisugi/hydra-mapdb`](https://github.com/Nisugi/hydra-mapdb) is: it
depends on `cena-map` — the room/exit record and the client's binary map
format — as a **git dependency on the client's own repo**
(`Nisugi/cena`), not a vendored copy, so there is one definition of the
format instead of two that can drift. Nothing here needs a local Cena
checkout to build, test, or run.

The relationship runs both ways: this repo pulls `cena-map` from Cena to
read a `.map` file, and Cena is expected to pull `cena-map-layout` from
here once its own GUI (M4+) needs to draw a map inside the client itself —
the same shape `cena-map` already has with `hydra-mapdb`'s two converter
crates.

## Why a layout engine at all

Simutronics ships hand-drawn artwork for some of the map, positioned by a
official layout file. But that covers a minority of walkable rooms
(measured in Cena's own research: 48% of walkable rooms, and unevenly —
71% of outdoor rooms but only 29% of indoor ones). Nearly a third of
Cena's own hunting-ground exploration and most interiors have no art to
show at all.

`cena-map-layout` is the algorithmic fallback: given a location's rooms
and their exits, it derives a 2D grid layout automatically — placing
rooms by their stated compass directions, packing separate buildings and
areas together by their connecting passages, and splitting indoor
buildings onto their own shelf sheet — so a reasonable diagram exists even
where no artist ever drew one.

## The two crates

- **`cena-map-layout`** — pure: no file I/O, no rendering toolkit, no
  window. Rooms of one location in, a `Layout` (and from that, a drawable
  `MapScene`) out. Ported from VellumFE's own layout engine
  (`src/core/layout_engine/`), restructured to Cena's own code standard
  rather than kept as a straight port. Pipeline: direction analysis → BFS
  placement with grid rips → per-component hill-climb and compaction →
  indoor/outdoor classification → cluster packing onto a shared outdoor
  sheet, plus a shelf for whatever interior doesn't seat cleanly beside
  its own doorway.
- **`cena-mapper`** — the standalone window (`egui`/`eframe`, the same
  fork Cena's future GUI is expected to use). Loads a `.map` file
  (`CENA_MAP` env var, or a path as the first argument) and draws the
  layout for whichever area you pick, with hand corrections saved beside
  the map.

  **Two area lists, and no room is in both.** The **Official** tab holds
  Simutronics' own layout areas (117, the unit their artwork is positioned
  in); the **Mapdb** tab holds what the game's `location` verb answers,
  for everything the official layout does not cover (239). Either is
  filterable by name.

  **Official layout is truth**: where it covers a room, that room is not
  listed again under its mapdb location. The official areas are curated
  splits, whereas mapdb `location` routinely cuts a building into areas of
  one or two rooms, so browsing by it alone buries a town under its own
  shopfronts. The two groupings genuinely cross-cut —
  `research/jev-trial/areas.py` measured it: *"66 official areas span
  several locations and 53 locations span several official areas"* — so
  the split is applied per room, not per name. A location partly inside an
  official area keeps its unclaimed rooms; one wholly inside it drops out.
  Against the real map that is 13,689 official + 23,149 mapdb rooms: zero
  overlap, and all 36,838 still reachable. The official list
  ships as `crates/mapper/data/areas.tsv`, a copy of that research
  output — `.map` files cannot supply it, since their `image.file` entries
  are raw artwork names, inconsistently spelled (`JourneysEnd.jpg` beside
  `Journeys_End.jpg`) and not area names at all. It is a frozen snapshot;
  refreshing it is a file copy.

  **The canvas is a camera**, not a scroll pane: drag to pan, wheel to
  zoom about the pointer, and a newly picked area starts centred and
  fitted rather than in the top-left corner. **Outdoor** and **Interiors**
  sheets both draw, toggled in the header — the interiors shelf is often
  the larger half (Wehnimer's Landing: 827 outdoor rooms against 2,402
  interior ones).

  **Hover names a room; clicking one opens the inspector**, which reads
  all three sources at once:

  - the **room record** — title (and its day/night and seasonal variants),
    id, uids, location, terrain, climate, tags, description, paths;
  - its **exits** — where each goes, the command, and *how* it is crossed.
    7,400 of the map's 84,867 exits are scripted rather than plain
    commands, and unported ones are impassable; the canvas draws them all
    identically, so the panel says which is which;
  - the **layout** — the room's cell, its group and building name, how
    that group was packed, and **any direction violations naming it**:
    what the exit claimed against where the room actually landed.

  That last part is the reason the panel reads the layout at all. There
  are 955 violations across the real map, in 108 of 356 areas, and nothing
  showed them before — a layout that had gone wrong looked exactly like
  one that had not. They are genuine data conflicts, not layout bugs: two
  rooms in Old Ta'Faendryl are joined by exits claiming *both* east and
  west, which no 2D placement can satisfy.

  **Editing.** The `Edit` toggle turns dragging from panning into moving:
  drag a group to shift it, hold Alt to move one room. A ghost outline
  previews where it lands, and the whole drag commits as one correction on
  release. `Reset (n)` forgets an area's corrections; `Unpin room` returns
  one room to where the solver put it.

  **Plates** are the answer to a crowded town. Wehnimer's Town Square
  Central has a well and a treehouse hanging off it; moving those onto
  `landing.well` and `landing.treehouse` takes them out of the town's own
  list, so they stop competing for space on its sheet while staying
  reachable as plates of their own. The inspector's Plate section moves
  the selected room, or its whole group, onto a new or existing plate; a
  third picker tab lists them.

  Corrections save to `<map>.overrides.json` beside the map file, written
  atomically on every edit, and are a diff applied *after* generation — so
  the solver's own output stays the thing being corrected, and a
  correction whose anchor no longer resolves is skipped rather than
  guessed at. Rooms are keyed by game uid where they have one, which
  survives a map rebuild, falling back to the map's room id for the 21%
  that do not.

  This ports two of VellumFE's override kinds (`plan/26` §0 named its
  `overrides.rs` as the reference). Its edge restyling, forced sheets and
  room-data edits are not ported; every field is `#[serde(default)]`, so
  they can arrive without a format change.

  One deliberate divergence: Vellum keys overrides by uid with a room-id
  fallback, on the stated grounds that *"uids are >= 7 digits, so the key
  spaces never collide"*. That does not hold here — 9,542 uids in `gs.map`
  are below 1,000,000 and 228 are negative, giving 160 keys two rooms both
  claim (room 1207 has uid 17127; room 17127 has no uid). Keys here name
  their space (`uid:7122`, `id:4242`) instead.

## Running it

```powershell
$env:CENA_MAP = "path\to\hydra.map"
cargo run --release -p cena-mapper
```

or pass the path directly:

```powershell
cargo run --release -p cena-mapper -- path\to\hydra.map
```

## Status

Verified against a hand-built fixture town (20 tests: BFS placement,
indoor/outdoor classification, cluster packing, image-anchor seating, the
scene the window draws, and the camera's transform and fit).

**Layout cost is measured**, against the real 36,838-room map: the worst
case, Wehnimer's Landing at 3,229 rooms, lays out and builds its scene in
~102ms (release). Recomputing on every selection change is therefore
fine, which the code previously only assumed.

Still open: reproducing VellumFE's own statistical *quality* targets
(zone-by-zone violation counts, connector lengths) against the real map.
Timing says the pipeline is fast enough; it does not say the layouts it
produces are good.
