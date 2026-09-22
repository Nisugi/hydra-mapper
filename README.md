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
areas together by their connecting passages, and hanging each indoor
building beside the street it opens off — so a reasonable diagram exists
even where no artist ever drew one.

## The two crates

- **`cena-map-layout`** — pure: no file I/O, no rendering toolkit, no
  window. Rooms of one location in, a `Layout` (and from that, a drawable
  `MapScene`) out. Ported from VellumFE's own layout engine
  (`src/core/layout_engine/`), restructured to Cena's own code standard
  rather than kept as a straight port. Pipeline: direction analysis → BFS
  placement with grid rips → per-component hill-climb and compaction →
  indoor/outdoor classification → cluster packing of the outdoor groups
  → the buildings hung beside their streets, on the same sheet at four
  times the outdoor spacing.

  **One sheet, and a focus.** Every room of an area has one cell, in one
  frame; what a renderer draws as squares is a *unit* — the streets, or
  one building — and everything else is a dot on the same roads. So the
  area is one continuous map whichever part of it is being looked at,
  and changing focus lays nothing out again and moves nothing. The
  scene names the units (`MapScene::units`: the streets first, then one
  per building, each with its door rooms), which is the contract a
  minimap draws from: squares for the unit the character is in, dots for
  the rest.

  Two places it now diverges from the port, both because the new mapdb
  format carries what the old one flattened away:

  **A ported script that only moves names its direction.** Upstream
  string procs were opaque, so every scripted exit was directionless by
  necessity. The steps now carry their movement as text, and 943 edges
  turn out to name an ordinary bearing. The commonest shape is one
  movement branched on opposite conditions — `west` or `swim west`,
  whichever the tide allows — so a script resolves when *every* movement
  it contains agrees, reading each command's last word. Movements that
  disagree are a walk through several rooms (10 edges), and anything with
  no bearing at all still resolves to nothing.

  **The buildings follow the town.** They were once packed in rows on a
  shelf of their own, ordered by group index, which put two shops whose
  doors open off the *same* street room a median of 42 cells apart in
  Wehnimer's Landing. Now the outdoor sheet is spread to four times its
  spacing and each building is placed in the gap beside its own street
  room: door edges are a median of one cell long across the real map's
  towns, and a building with doors on two streets sits between them
  because the streets are where they were. The one that cannot be placed
  well is the complex with doors all over town — Ta'Illistim Keep, 123
  rooms and 23 doors — which sits by one of them and reaches the rest
  with long connectors; that is a case for a drawing of its own, not
  yet built.
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

  **An area filter never strands a room from its building.** 361 rooms
  used to lay out alone with no connection inside their own area at all,
  because the boundary cut them from the room their door opens onto —
  `[Haegan's Weaponry]` has `location` "Cysaegir" while its street is
  "the village of Cysaegir". An area is laid out from its own rooms plus
  whatever a stranded room needs to stay attached, which takes that to
  zero. Layout only: the room still belongs to its own area, so the
  lists, the counts and the export are unchanged.

  **The canvas is a camera**, not a scroll pane: drag to pan, wheel to
  zoom about the pointer, and a newly picked area starts centred and
  fitted rather than in the top-left corner. The header names what is
  **in focus** — the streets, a building, or an official hunting area —
  and **Back** returns to the previous focus. Clicking a dot enters the
  unit it belongs to, with the room in the middle of the view; a
  building wins over a hunting area that lists its rooms, because the
  building is what a person standing in it is in.

  **Hover names a room; clicking one opens the inspector**, which reads
  all three sources at once:

  - the **room record** — title (and its day/night and seasonal variants),
    id, uids, location, terrain, climate, tags, description, paths;
  - its **exits** — where each goes, the command, and *how* it is crossed.
    7,923 of the map's 84,867 exits arrive as something other than a plain
    command: 2,915 as ported scripts, 4,485 as named routines, and 523 as
    pass-throughs, the A→hub→B urchin hops that are not passages at all.
    The canvas draws them identically, so the panel says which is which;
  - the **layout** — the room's cell, its group and building name, how
    that group was packed, and **any direction violations naming it**:
    what the exit claimed against where the room actually landed.

  That last part is the reason the panel reads the layout at all. There
  are 667 violations across the real map, and nothing showed them before —
  a layout that had gone wrong looked exactly like one that had not.

  **More than half are the solver's fault, not the data's.** An earlier
  version of this README claimed they were all data conflicts, and it was
  wrong.

  `cena_map_layout::satisfiable` answers the question properly. Every
  compass direction is a pair of strict inequalities — *b northeast of a*
  is `x_a < x_b` and `y_b < y_a` — the two axes are independent, and such
  a set is satisfiable exactly when its constraint graph holds no cycle.
  Nothing there cares how far, so a stretched edge satisfies what a unit
  one does. Two cycle detections, O(V+E), no search.

  Against the real map: of 667 violations, **358 sit in components whose
  directions are entirely satisfiable** — an arrangement exists and the
  solver did not find it — and **309** are in the 25 groups that close a
  genuine contradictory loop, like the two Old Ta'Faendryl rooms joined
  by exits claiming *both* east and west.

  That is reproducible in four rooms. Given exits
  `0: 1 ne, 2 ne, 3 n / 1: 0 sw, 2 se / 2: 0 sw, 1 nw / 3: 0 s`, the
  arrangement `0=(0,2) 1=(1,0) 2=(2,1) 3=(0,0)` satisfies all eight, and
  the engine instead reports two violations. Searching every satisfiable
  arrangement on a 3×3 grid, the failure rate rises with density: 3% at
  four rooms, 8% at five, 14% at six. It needs **stretched diagonals** —
  a search that only connects adjacent cells never produces the case,
  which is why it went unnoticed here for so long. Found by ATARI, who
  supplied the fixture.

  So a violation currently means "something is wrong here", not "the data
  contradicts itself". Telling those apart is open work: the lead is to
  establish whether the directional constraints are satisfiable at all
  before compacting, which would separate *ask a human* from *the solver
  got stuck*.

  **Editing.** The `Edit` toggle turns dragging from panning into moving:
  drag a group to shift it, hold Alt to move one room. A ghost outline
  previews where it lands, and the whole drag commits as one correction on
  release. `Reset (n)` forgets an area's corrections; `Unpin room` returns
  one room to where the solver put it.

  **Edge corrections** are the tool for a layout that is wrong rather than
  untidy. The inspector's Edges section gives every exit a combo: `auto`,
  `passage`, or one of the ten bearings. `passage` un-welds two rooms the
  solver placed adjacent on bad data; a bearing forces what the exit should
  have said. Both are **inputs to the solve** — applied to the direction
  map before positioning, so the rooms are laid out *by* the corrected
  geometry rather than nudged afterwards, and packing, classification and
  violation counts all follow from it.

  This is what answers a violation of the **contradictory** kind — the
  309 above, not the 358 the solver is responsible for, which no
  correction should have to paper over. In
  `elven-nations-old-tafaendryl-west`, rooms 11988 and 11989 are joined by
  exits claiming *both* east and west; one correction takes that area from
  36 violations to 34, with overlaps still at zero. Vellum's other three
  edge actions (`Hide`, `Dash`, `Dots`) only restyle a drawn line and are
  deliberately not ported — that is a renderer's concern, not the layout
  engine's.

  **Plates** are the answer to a crowded town. Wehnimer's Town Square
  Central has a well and a treehouse hanging off it; moving those onto
  `landing.well` and `landing.treehouse` stops them competing for space on
  the town's sheet. The inspector's Plate section moves the selected room,
  or its whole group, onto a new or existing plate; a third picker tab
  lists them.

  **A plate is a grid, not a place.** A plated room is *drawn* elsewhere
  but still *belongs* to its area: the Town Well sits on `landing.well`
  and remains a room of Wehnimer's Landing, so it appears in both lists
  and the town's count is unchanged. `plan/21` §3f keeps the same pair
  apart, as `location` (the area) and `map` (the grid), and collapsing
  them would lose the area for travel and everything else that groups by
  it. An area's list marks how many of its rooms lay out on plates.

  **Export for the combiner** writes `<map>.corrections.json`: the
  corrections in terms another program can merge. Hydra and Vellum run the
  layout engine themselves, so what travels is not positions but the
  corrected *inputs* a layout is derived from.

  Edge corrections export as **`dirto`**, a field the mapdb room record
  already has and the layout engine already reads before the movement
  command — a bearing for a forced direction, and upstream's own
  `cross-group` for a passage. Entries are written on both rooms, the far
  one carrying the opposite bearing. Plates export as `map_membership`
  (the grid) plus `area` (the place). Pictures come in pairs: the area
  with the streets in focus, and with every building in focus, under the
  `<area>.interiors` name that was once a grid of its own.

  Everything in the export is keyed by **uid**: the combiner merges into a
  map whose room ids it assigns itself, so a room with no uid cannot be
  named in a way that survives that. Those corrections are reported as
  skipped rather than written under an id that would mean a different room
  next build.

  Corrections save to `<map>.overrides.json` beside the map file, written
  atomically on every edit, and are a diff applied *after* generation — so
  the solver's own output stays the thing being corrected, and a
  correction whose anchor no longer resolves is skipped rather than
  guessed at. Rooms are keyed by game uid where they have one, which
  survives a map rebuild, falling back to the map's room id for the 21%
  that do not.

  This ports three of VellumFE's override kinds (`plan/26` §0 named its
  `overrides.rs` as the reference). Its edge *restyling*, forced sheets and
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

92 tests, most against hand-built fixtures: BFS placement, indoor/outdoor
classification, cluster packing, image-anchor seating, shelf ordering, the
scene the window draws, the camera's transform and fit, direction
resolution, the override store's round trip and the correction format.

**Layout cost is measured**, against the real 36,838-room map: the worst
case, Wehnimer's Landing at 3,229 rooms, lays out and builds its scene in
~102ms (release). Recomputing on every selection change is therefore
fine, which the code previously only assumed.

**Layout quality is not.** Timing says the pipeline is fast enough; it
does not say the layouts are good, and on the evidence above they are
not as good as the violation count was taken to mean. The open work, in
the order it matters:

- **The placement and repair passes fail on satisfiable input** — 358 of
  the 667 violations, reproducible in four rooms. Both repair passes are
  local: the hill climb moves one room among its neighbours, and the
  reweld cascades outward but refuses to move the anchor, so neither can
  make the coordinated shift a fix sometimes needs. Placing by
  topological order, which the satisfiability check already computes,
  would satisfy every constraint by construction.
- Reproducing VellumFE's own statistical quality targets (zone-by-zone
  violation counts, connector lengths) against the real map.
- **35% of edges still resolve no direction at all** — 29,798 of 84,867,
  overwhelmingly `go <door>` (15,928) and `out` (5,171). That is the
  structural limit on how much of the map can be placed by geometry
  rather than packed, and no amount of solver work moves it.
