# Room classification: what the map records now, and what it should

Written for whoever curates `gs.map` and whoever consumes it — the
walker, the mapper, and Hydra's minimap. Measured on `gs.map`, 36,838
rooms, 2026-09-22.

## The problem in one line

A room's *contents*, its *identity*, its *access rules* and its
*disposition* are all stored in the same two fields, so the ones that
matter least are maintained best.

`tags` holds 1,976 distinct values. About 950 of them are herb and
creature names — room contents, re-sensed monthly by `;tags --crawl`.
The five values that decide whether a room should exist on a map at all
(`gone`, `closed`, `missing`, `rewritten`, `urchin-hideout`) sit in the
same list, unversioned and unenforced, and are applied to roughly 1,150
rooms out of 36,838.

The result is silent inconsistency. **Caligos Isle sank in 2021 and 676
of its 677 rooms carry no disposition at all** — one room is tagged
`closed`. The Feywrot Mire, also shut, has `gone` on two of 628 rooms.
Spitfire has `closed` on 9 of 85. Nothing in the data distinguishes any
of them from Rumor Woods, which is open.

## Four independent axes

These are genuinely orthogonal and want separate fields. Conflating any
two is what produces the current ambiguity.

| axis | question | example | belongs to |
|---|---|---|---|
| **identity** | what *is* this? | urchin hub, maze, transition | the node |
| **disposition** | does it exist, is it open? | Caligos (gone), the Abbey (closed) | the **area**, usually |
| **access** | can *this character* get in? | premium halls, Voln, citizenship | the **edge**, per-walker |
| **knowledge** | do we know where it is? | 186 Bloodriven sewer rooms | derived, not stored |

The access axis is already correct and should not be touched:
`Cost::Gated` prices an exit as `None` when a flag is absent, and
`tags.lic` mutates its own ignore list from the character's profession,
gender, race, CHE, guild and society (`tags.lic:157-183`). That is the
right shape — a property of the edge, evaluated per walker — and the
reason is that a node-level "avoid this room" would answer identically
for every character, which is wrong.

The knowledge axis should stay **derived, never stored**. A room with no
exits and nothing pointing at it cannot be placed, whatever the reason;
`regions::is_connected` computes that in two lines. Storing it would
create a fourth thing to keep in sync, and `missing` (52 rooms) already
shows what happens: it means "the mapper could not find it", which is
neither gone nor closed, and no consumer knows what to do with it.

## What exists today

### `meta` — 48 namespaces, and already the right shape

`meta` is a flat `Vec<String>` of `namespace:value` strings. It is
better structured than `tags` and is where classification already
partly lives.

**Identity — what the room is**

| meta | rooms | note |
|---|---:|---|
| `map:virtual room` | 16 | **exactly the 16 urchin hideouts** — identical set to the `urchin-hideout` tag |
| `map:multi-uid` | 206 | one room, several game ids |
| `map:ignore-blanks` | 40 | parser hint |
| `map:ignore-selfpath` | 2 | parser hint |
| `map:close and open door` | 1 | walker hint |
| `maze:` | 65 | |
| `transition:` | 2 | |
| `transport:` | 12 | |
| `town:defense` | 5 | |

**Behaviour — affects walking, not drawing**

| meta | rooms |
|---|---:|
| `nomagic:` | 667 |
| `splashy:` | 235 |
| `trap:` | 128 |
| `underwater:` | 66 |
| `noteleport:fwi` | 35 |
| `morphing:` | 34 |
| `latched:` | 21 |
| `jail cell:` | 11 |
| `samepath:<dirs>` | 3 |

**Access gates — per character**

| meta | rooms | values |
|---|---:|---|
| `che:*` | 1,233 | 72 (House of the Argent Aspis, Helden Hall, …) |
| `society:*` | 136 | Order of Voln 89, Council of Light 41, Sunfist 6 |
| `premium:` | 123 | |
| `mho:*` | 140 | 17 |
| `pay-to-play:` | 44 | |
| `citizenship:*` | 43 | Ta'Vaalor 26, Solhaven 12, Icemule 3, Teras 1, WL 1 |
| `prof:*` | 9 | cleric 5, sorcerer 4 |
| `gender:*` | 2 | female 1, male 1 |
| `game:*` | 29 | GSPlat 16, GSX 7, GSF 6 |

**Feature / content**

| meta | rooms | values |
|---|---:|---|
| `playershop:` | 2,401 | bare |
| `fest:*` | 590 | grawood 211, anfelt 186, velathae 113, highman 75, fof 4, frontier 1 |
| `teleport:fwi` | 247 | |
| `locker:` / `locker annex:*` | 373 | |
| `taskroom:` | 63 | |
| `storyline:` | 62 | |
| `mentor:` | 34 | |
| `trashcan:*` | 42 | 34 values |
| `boxpool:*` | 26 | 24 values |
| `quest:nexus` | 5 | |

**mapdb legacy** — `mapname` (234 values), `mapshortname` (198),
`mapcategory` (32, incl. `Events and Festivals` on 14). Metadata about
the old map *files*, not about rooms. Covers only 233 rooms.

### `tags` — 1,976 values, ~950 of them contents

**Dispositions (the whole set):**

| tag | rooms | locations | meaning |
|---|---:|---:|---|
| `closed` | 798 | 24 | shut, not deleted — the Abbey, Revel of the Anfelt |
| `gone` | 145 | 13 | removed from the game |
| `rewritten` | 140 | 9 | replaced by new ids; Cairnfang Manor is rewritten *and live* |
| `no-auto-map` | 135 | 22 | do not map |
| `missing` | 52 | 7 | the mapper could not find it |
| `urchin-hideout` | 16 | — | duplicate of `meta:map:virtual room` |
| `duplicate` | 2 | — | |

**Other functional tags:** `no forageables` (5,184), `urchin-access`
(517), `private property` (318), `node` (212), `supernode` (91),
`table` (41), `bank` (36), plus ~30 service tags (`inn`, `gemshop`,
`furrier`, `herbalist`, `pawnshop`, …) used as `;go2` destinations.

### The same fact, spelled several ways

Two distinct problems, both worth fixing before anything is built on
top of this data.

**(a) One concept split across `tags` and `meta`.** Eighteen concepts
appear in both fields. Most are coincidence (`temple` appears in
`mapname:` strings), but three are real:

| concept | tags | meta | overlap |
|---|---:|---:|---:|
| lockers | 153 | 164 | **26** |
| premium | 24 | 123 | 23 |
| citizenship | 6 | 43 | **0** |
| teleport | 8 | 246 | **0** |
| trap | 1 | 128 | **0** |

Near-zero overlap means these are not redundant copies — each field
holds rooms the other does not — so a consumer reading only one gets a
partial answer.

**Not every split is a bug.** `jail` is the instructive counter-example:
11 rooms carry the tag, 11 carry `meta:jail cell`, and the overlap is
**zero** — because they are different rooms.

```
tag jail        -> [Wehnimer's, Constabulary]   (pay the fine)
meta jail cell  -> [Wehnimer's, Jail Cell]      (serve the time)
```

Eleven towns, one of each. That is correct modelling with an
unfortunate tag name, and it should be left alone.

**Lockers are the genuine mess: four overlapping schemes, 538 rooms.**

- `meta:locker:` — 165 rooms, bare
- `meta:locker annex:<House>` — 208 rooms, 14 houses
- `meta:che:<house>:locker` / `:entrance_locker` / `:entrance_annex` —
  ~350 rooms, per-house
- **22 distinct tags**: `public locker` (89), `publiclockers` (89),
  `houselockers`, `locker:public`, `pauperslockers`, `bhalocker`,
  `cysaegir public locker`, `brigatta locker`, `locker:twilighthall`,
  `sglocker`, `lockerentrance:twilighthall`, `paupers locker`,
  `silvergate locker`, `locker:beaconhall`, `public lockers`,
  `twilight locker`, `sylvanfair locker`, `sylvanfair annex`, …

`locker annex` and `che:*locker*` overlap on 102 rooms; the rest are
disjoint. One room carries three spellings at once:

```
RoomId(18253) [Lockers, Antechamber]
  meta = ["che:paupers:entrance_locker", "locker annex:House of Paupers"]
  tags = ["paupers locker", "pauperslockers"]
```

**(b) One concept spelled several ways within `tags`.** 44 families
differ only by spacing or case. The ones affecting five or more rooms:

| rooms | spellings |
|---:|---|
| 213 | `node` (212), `Node` (1) |
| 91 | `publiclockers` (89), `public lockers` (2) |
| 27 | `locksmith pool` (13), `locksmithpool` (13), `locksmith-pool` (1) |
| 23 | `pawnshop` (21), `pawn shop` (2) |
| 22 | `lumnisdonate` (11), `lumnis donate` (11) |
| 21 | `gemshop` (20), `gem shop` (1) |
| 20 | `bardguild` (10), `bard guild` (10) |
| 20 | `rogueguild` (10), `rogue guild` (10) |
| 19 | `sorcererguild` (9), `sorcerer guild` (9), `Sorcerer Guild` (1) |
| 18 | each of cleric/empath/warrior/wizard guild — split ~9/9 |
| 16 | `rangerguild` (8), `ranger guild` (8) |
| 13 | `clericshop` (9), `cleric shop` (4) |
| 12 | `sanctuary` (11), `Sanctuary` (1) |

The guild tags are the worst: **every guild is split almost exactly in
half** between the spaced and unspaced spelling. Any consumer filtering
on `"bardguild"` silently finds half the bard guilds.

This is the argument for the service tags becoming a closed enum rather
than free text.

**Contents:** ~950 herb and creature names. `tags.lic`'s `@herb_list`
has 611 entries and **has drifted behind the map** — `withered mushroom`
(5,597 rooms), `soft mushroom` (4,435), `wild rose` (3,465) and whole
families (teas, wisterias, oleanders, daturas) are absent from it.

### `location` — 342 values, and not a partition

- 1,608 rooms have none; 661 are `location_unknowable`; 263 are
  `check_location`.
- Only 4 near-duplicates (`Hinterwilds` / `the Hinterwilds`), so
  normalisation is nearly a non-issue.
- **103 locations encode structure in prose**: "Arborsong, inside the
  island town of Mist Harbor". `regions::region_of` parses English to
  recover a parent that should have been a field.
- Too coarse where it exists: "Mist Harbor" is 1,692 rooms covering many
  distinct places. The graph finds 427 areas where `location` names 342,
  and **208 of those areas have names invented from title prefixes**
  because no location distinguished them.

### The wiki dump — a curated source for `area`, not for `location`

`E:\Gemstone\data\wiki_clean` holds 19,618 article dumps. Two things in
it bear on this.

**`List of hunting areas.txt` is a curated area hierarchy** — 112 areas
under 8 regions, human-authored, exactly the `region → area` shape §1
proposes:

```
== Wehnimer's Landing ==
*Castle Anwyn
*Catacombs (Wehnimer's Landing)
*Upper Trollfang
...
```

Matched against `derive_areas`, **58 of the 112 already agree by name**,
with 6 more matching a `location` but not a derived area. That is
independent corroboration: the graph grouping finds the same places a
human curator named, without being told about them. It is the strongest
validation of `derive_areas` so far, and the list is a ready-made seed
for the curated `area` field. Parsed to `analysis/wiki-hunting-areas.tsv`.

The 48 that match nothing are mostly a granularity question rather than
a failure — Cavernhold, Ant Hill, Sentoph and Thanatoph are hunting
grounds *inside* larger derived areas. Deciding whether those are areas
or sub-areas is the same question as §1's parent pointer.

**Articles carry room-level data that joins to the map.** Pages include
room titles, descriptions, exits and `Room number::NNNNNN`, plus a
`realm::` property:

```
Isle Designs is a shop in realm::Mist Harbor ...
[Isle Designs, Entry]
Room: Room number::739511
```

**670 of the 697 distinct room numbers join to the map**, so the wiki is
queryable against `gs.map` — but *two numbering schemes share the
field*, because pages written before uids existed quote the old game
room number. They separate by magnitude:

- **632 numbers are ≥ 40,000** — uid-shaped, and match a `uid`.
- **65 are < 40,000** — map-id-shaped. 64 of these match a uid *and* an
  id (a small number valid in both spaces, i.e. a coincidence), and
  exactly one is id-only: `25985`, `[Journey's End, Lawn]` in Solhaven,
  whose uid is `4547201`.

So a reader should resolve small numbers as ids and large ones as uids,
rather than trying `uid` first as this analysis originally did.

The 26 that match nothing (`3002036`, `7110481`, `13010047`, …) are
uid-shaped but absent from `gs.map`: rooms never recorded, or removed
content the wiki still documents.

**But it cannot fill the location worklist: 0 of the 960 rooms are
covered.** The wiki documents rooms people wrote articles about — shops
and landmarks — which are exactly the well-recorded rooms that already
have uids and locations. The gaps are gaps because nobody has documented
them either. Walking remains the only way to close them.

Useful, then, for naming and grouping; not for gathering.

## Proposal

### 1. `landmass` → `region` → `area` — curated place, replacing prose parsing

Three tiers, not two. The third comes from the Open Sea Adventures map
(`Open Sea Adventures.png`, cartography by Arianiss Winterfox, May
2025), which supplies a fact **the room graph cannot derive**: which
places share a continent, and which are reachable only by sea.

The map shows one main continent split north-south by the DragonSpine:

- **West of the spine** — Icemule Trace, Glaoveln, Wehnimer's Landing,
  Vornavis/Solhaven, Brisker's Cove, Fairport, Ta'Nalfein, Tamzyrr,
  the Feywrot Mire
- **East of the spine** — Ta'Illistim, Ta'Loenthra, Ta'Vaalor, Atan Irith
- **Islands, reachable only by sea** — Teras Isle, Kraken's Fall,
  Caligos Isle, Isle of Ornath
- **A southern landmass** — New Ta'Faendryl, Maelshyve, Sharath,
  Idolone, Ubl

Why this matters for layout: two rooms can be adjacent in the graph — a
ship route, a portal, an urchin — while sitting on different continents.
`region_of` has no concept of this, so it cannot tell "next door" from
"across an ocean".

It also explains apparent gaps. Of the 11 OSA port towns, 8 are already
top-level regions in `derive_areas`. The three that are not turn out to
be naming mismatches rather than missing data:

| OSA map | `gs.map` | relationship |
|---|---|---|
| Teras Isle | `the town of Kharam-Dzu` (861 rooms) | the island vs the town on it |
| Glaoveln | `the Pinefar Trading Post` (302) | the shore vs the settlement |
| Sleeping Drake Harbor | `the Isle of Ornath` | a harbour on a Southern Ocean island |

**The clearest case for curation is Solhaven.** The OSA map labels
Vornavis and Solhaven as one place. `derive_areas` produces seven
regions for it:

```
1748  Solhaven                              317  the southern part of Solhaven
 120  the plains of Vornavis                 87  Vornavis
  48  Solhaven's Warrior Guild               38  Vornavis Proper
   8  between Wehnimer's Landing and Solhaven
```

No string parsing fixes that. It needs a curated field.



Two fields, or one `area` with a parent pointer. `location` stays as the
game's own answer (useful, gathered by visiting); `area` is the curated
one the mapper and minimap use.

Requirements:

- **A partition.** Every room in exactly one area. `location` is not
  one, which is why Wehnimer's Landing is 274 connected pieces under one
  name.
- **A parent.** The 103 prose locations prove a flat field is
  insufficient — "Arborsong" needs to belong to "Mist Harbor" without
  encoding it in the string.
- **A landmass at the top.** Only the OSA map has it, and only it can
  say that a graph edge crosses an ocean. 8 of the 11 port towns are
  already regions; the other three are naming mismatches, above.
- **Generated, then corrected.** `regions::derive_areas` already
  produces 427 areas as a first pass; nobody should hand-author 36,838
  rooms. This is what the mapper's corrections store is for.

Expect the curated count nearer 427 than 342: the graph is finding real
distinctions the location field does not encode.

### 2. `meta:map:status` — disposition, promoted into the namespace that already holds identity

**Recommendation: use the existing `map:` namespace rather than a new
field.** `meta:map:virtual room` is already precisely the concept, and
it is already exactly right — the same 16 rooms the `urchin-hideout` tag
marks. Extending it costs no new plumbing and no new parser.

```
meta:map:status:live      (default; need not be written)
meta:map:status:closed    draw, do not route
meta:map:status:gone      do not draw
```

**The verbs are the point.** `closed` and `gone` differ in *what they
disable*, not in flavour, and stating that in the value stops each
consumer inventing its own reading — which is how the mapper ended up
drawing teleport edges the walker would never take:

| status | draw | route | example |
|---|:--:|:--:|---|
| `live` | yes | yes | Rumor Woods, Duskruin |
| `closed` | yes | **no** | the Abbey, Duskruin between runs |
| `gone` | **no** | no | Caligos Isle, Feywrot Mire, Spitfire |

**Set it per area, override per room.** Area-level is how Caligos gets
fixed in one edit instead of 677; room-level is needed because
Bloodriven's `[Sable Quietus, Entry]` is one closed shop on a live
street. Room wins over area.

This subsumes `gone`, `closed` and `missing` as tags. `rewritten` is
**not** a disposition — Cairnfang Manor is rewritten and live — and
should stay a tag, or become `meta:map:rewritten`.

### 3. Keep the access axis exactly as it is

`Cost::Gated` on the edge, `meta:che:*` / `society:*` / `citizenship:*` /
`premium:` as the conditions. Nothing to change. Worth stating in the
schema so nobody later "simplifies" it into a room flag.

### 4. Housekeeping worth doing while in there

- **Normalise the split spellings.** 44 tag families differ only by
  spacing or case; the guild tags are split almost exactly in half
  (`bardguild` 10 / `bard guild` 10, and the same for cleric, empath,
  ranger, rogue, sorcerer, warrior, wizard). A consumer filtering on one
  spelling finds half the guilds. Service tags want a closed enum.
- **Consolidate the four locker schemes** (538 rooms, 22 tags, three
  meta namespaces) onto one. `meta:locker:<house|public>` would cover
  every case now spread across `locker:`, `locker annex:<House>`,
  `che:<house>:locker` and the tags.
- **Leave `jail` alone.** The tag marks the constabulary, the meta marks
  the cell; zero overlap is correct.
- **`urchin-hideout` is redundant** with `meta:map:virtual room`
  (identical 16 rooms). Drop the tag, keep the meta.
- **140 `duplicate of NNNN` rooms** — bookkeeping stubs with titles like
  `duplicate of 2937`, no exits, nothing pointing in. Delete rather than
  visit.
- **Two mislabelled rooms**: 26891 and 26896 carry
  `location = "the sewers of Bloodriven Village"` but their exits lead to
  Mark Alley, Pewter Road and Bailey Park. They are village rooms.
- **Update `@herb_list`** in `tags.lic` — it is missing at least 340
  forage names, which affects `;tags --sense`, not just this census.
- **Two festival schemes**: `fest:*` marks 590 rooms across six
  festivals, but Caligos uses `mapcategory:Events and Festivals`
  instead. Neither is complete. Pick one.

## What this fixes, concretely

- Caligos Isle, the Feywrot Mire and Spitfire (1,627 rooms, three
  derived areas) can be hidden by three area-level edits, with no
  hardcoded location list in the layout engine — which would rot the
  next time a festival closes.
- 208 invented area names become curated ones.
- `regions::region_of` stops parsing English.
- The mapper and Hydra's minimap read the same disposition and cannot
  drift.

## What the map cannot tell us, and needs walking

Separate from the schema: 421 reachable rooms need a `location` the game
knows and the file does not (`analysis/worklist.tsv`). 754 have no location, 206
are flagged `check_location`. Each row names a located neighbour and the
command to reach it.

Note this does **not** fix the invented area names: typing `location` in
Cobblestone Path returns "Mist Harbor", the same coarse answer that
caused the problem. Walking gathers the game's opinion; `area` needs a
curator's.
