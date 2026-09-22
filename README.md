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
  layout for whichever area you pick. Read-only: this is an explorer, not
  an editor — no write-back, no way to hand-correct a bad layout yet.
  Vellum's own override system (position pins, edge overrides,
  classification flips) is the reference for what that becomes later; it
  is not ported.

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
