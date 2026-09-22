# The corrections format

What `hydra-mapper` exports, what it means, and what a consumer should
accept or refuse. This is the contract between the mapper and anything
that merges its output — principally the `hydra-mapdb` submission flow.

**Current version: 1.**

## What this is, and what it deliberately is not

A corrections file carries **corrected inputs to a layout**, never a
layout. Hydra and Vellum run `cena-map-layout` themselves; handing them
cell coordinates would mean two engines disagreeing about the same map.
So what travels is the facts the solver reads — which way an exit really
goes, which grid a room is drawn on, which area it belongs to — and the
positions are derived downstream, fresh, every time.

Consequences worth stating plainly:

- **No coordinates appear in this format.** Not cells, not pixels, not
  offsets. A room pin or a group offset made in the mapper is editor
  state and does not export. That is a deliberate boundary, not an
  oversight; see *Placement* below.
- **Everything is keyed by uid**, the game's own room number, because the
  combiner assigns its own room ids on every build. A correction on a
  room with no uid cannot be named in a way that survives a rebuild, so
  it is reported to the person as skipped rather than written under a
  key that would silently mean a different room next month.
- **Merging is additive.** Nothing here deletes or rewrites a source map
  record; `dirto` entries merge into the room's own `dirto` map, and the
  membership fields are new metadata.

## The file

```json
{
  "version": 1,
  "generator": "hydra-mapper 0.1.0",
  "source_map": "hydra.map",
  "dirto": { "7000001": { "7000002": "east" } },
  "map_membership": { "4124007": "icemule.south.barn.pilot" },
  "maps": {
    "icemule.south.barn.pilot": {
      "name": "Icemule south — barn, yard and ravine (pilot)",
      "area": "icemule-trace-south-gate-wilds"
    }
  },
  "area": { "4124007": "icemule-trace-south-gate-wilds" }
}
```

Every field except `version` and `generator` is optional and omitted when
empty. JSON object keys are strings because JSON has no integer keys; all
room keys parse as `i64`.

### `version` (integer, required from v1 on)

The format version. A consumer that does not know the number **should
refuse the file** rather than read the fields it recognises and ignore the
rest — a later version may change what an existing field means.

A file with **no** `version` is version 1. Exports written before the field
existed (the September 2026 pilot among them) are version 1 in every
respect but saying so.

The number bumps when a field changes meaning or a required field is
added. Adding an optional field is not a bump: every field is
`serde(default)`, so an older reader ignores what it does not know.

### `generator` (string, required)

What wrote the file, e.g. `hydra-mapper 0.1.0`. Provenance for a file
found on its own later. Not a compatibility signal — use `version`.

### `source_map` (string, optional)

The map file the corrections were made against. Provenance only. A
correction is uid-keyed and does not depend on this map, but it is what
lets a reviewer tell which build a submission was reasoned about.

### `dirto` (object, optional) — the direction corrections

`room uid → destination uid → direction string`.

This is **the field the layout engine already reads**, before the movement
command, when resolving an edge. Exporting into it means the combiner
invents nothing and the clients need no new code: a corrected map simply
lays out correctly.

Values are one of:

| Value | Meaning |
|---|---|
| `north`, `northeast`, `east`, `southeast`, `south`, `southwest`, `west`, `northwest`, `up`, `down` | Position the destination in that bearing |
| `cross-group` | The rooms connect, but do not position by this edge |

`cross-group` is upstream's own word. It is the fix for two rooms the
solver welded together on bad evidence — a `go door` it guessed a compass
bearing for, or a pair of exits that disagree. Un-welding lets each room
be placed by its better-evidenced exits.

Upstream also defines `none` and `skip`, both meaning "fall through to the
command text". That is identical to having no correction at all, so the
mapper never writes them. A consumer may encounter them in
hand-written data and should treat them as absent.

**Entries are written on both rooms.** A bearing on `a → b` writes the
opposite on `b → a`; a `cross-group` writes `cross-group` both ways. The
engine reads the entry on whichever room it happens to be resolving from,
so a one-sided correction would apply only half the time. A reverse entry
is **not** invented for an exit the map does not have: a one-way passage
stays one-way.

### `map_membership` (object, optional) — which grid

`room uid → map slug`. The grid a room is laid out on, when that is not
its area's own sheet. Two things arrive here:

- **Plates**, made by a person to stop satellites crowding a town — the
  Town Well and treehouse hanging off Wehnimer's Town Square.
- **Interiors shelves**, one per area, slug `<area>.interiors`. These are
  automatic. An area's indoor rooms are packed as an independent grid from
  its outdoor sheet, so they must not share a slug: merging them puts 632
  rooms of the real map on top of another room's cell.

A deliberate plate move wins over the interiors shelf.

### `maps` (object, optional) — what the plates are

`map slug → { name, area }`.

- `name` (string, required) — what to show the plate as.
- `area` (string, optional) — the area the plate is a sheet of, so a
  consumer knows where it attaches rather than treating it as a map
  adrift. Absent for an orphaned plate, which is legal.

Only plates the export actually references appear; a plate minted and left
empty does not travel.

> **Reading older files.** The September 2026 pilot export wrote `maps` as
> `slug → string` (the name alone), before the `area` field existed. A
> consumer that wants to accept those should read a bare string as
> `{ name: <string>, area: null }`. Files written by current mapper always
> use the object form.

### `area` (object, optional) — which place

`room uid → area name`, written for every room the export puts on a plate.

**A plate is a grid, not a place.** The Town Well is *drawn* on
`landing.well` and remains a room *of* Wehnimer's Landing. Without this
field a consumer reading only `map_membership` would conclude the room had
left its area — losing the one fact travel, hunting-ground grouping and
every area-keyed lookup depend on. `plan/21` §3f keeps the same pair apart,
as `location` (the place) and `map` (the grid).

## Placement, and why it is not here

Room pins and group offsets — dragging a room or a building in the mapper
— are **editor state**, held in `<map>.overrides.json` and not exported.

They are applied *after* the solver runs, to one particular solve of one
particular map with one particular set of edge corrections. A cell number
is only meaningful relative to that run. Change the inputs and it may name
a different place, or nowhere.

This is a real limitation, not a claim that dragging is worthless. A drag
usually encodes something true — it is just expressed as a coordinate
instead of a relationship. The intended direction is for the mapper to
capture the *relationship* at edit time (this room is east of that one;
this connector spans three cells; this component anchors northeast of
that), so it can travel as `dirto` or as fields yet to be designed. Until
that exists, **approved manual positions do not reach Hydra**, and a
submission flow should not attempt to infer them from a sidecar.

## Validating a submission

For the `hydra-mapdb` issue form, a reasonable bar:

**Refuse** when

- `version` is present and not a version the validator knows;
- a room key does not parse as an integer, or names a uid absent from the
  current map (report which — this usually means a stale submission);
- a `dirto` value is not one of the ten bearings or `cross-group`
  (`none`/`skip` are better dropped than refused);
- a `map_membership` slug has no entry in `maps` and is not an
  `<area>.interiors` slug;
- `dirto` entries are one-sided, or disagree with each other — `a → b`
  east with `b → a` anything but west.

**Accept but flag** when

- `source_map` names a build older than current, since the reviewer should
  know the correction was reasoned about against different data;
- a plate has no `area`;
- the file carries a `maps` entry no room references.

**Never** accept coordinates in a corrections file. No version of this
format has a field for them, and something offering one is not this format.

## Where accepted corrections live

Corrections are merged **as a sidecar the combiner reads**, not as edits
baked into a map file. The map is a build artifact regenerated from
upstream mapdb data; anything written into it is lost on the next refresh.
Keeping accepted corrections as their own reviewed, version-controlled
input is what makes them survive an upstream update — and what lets a bad
one be reverted by reverting a commit rather than by hand-repairing a
binary.
