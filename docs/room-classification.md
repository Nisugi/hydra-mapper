# Room classification: what the map records, and how it got that way

Written for whoever curates `gs.map` and whoever consumes it — the
walker, the mapper, and Hydra's minimap.

Measured on `gs.map` after the `retag` migrations: **36,585 rooms,
2026-09-22**. The census files in `analysis/` were taken *before* those
migrations, against 36,838 rooms and 1,976 tags, so where they disagree
with this document they are the older picture.

## The problem this started from

A room's *contents*, its *identity*, its *access rules* and its
*disposition* were all stored in the same two fields, so the ones that
mattered least were maintained best.

`tags` held 1,976 distinct values, about 950 of them herb and creature
names re-sensed monthly by `;tags --crawl`. The five values that decide
whether a room should exist on a map at all — `gone`, `closed`,
`missing`, `rewritten`, `urchin-hideout` — sat in that same list,
unversioned and unenforced.

The result was silent inconsistency. **Caligos Isle sank in 2021 and 676
of its 677 rooms carried no disposition at all.** The Feywrot Mire, also
shut, had `gone` on two of 628 rooms. Nothing in the data distinguished
either from Rumor Woods, which is open.

That is fixed. `tags` is now 1,910 values, of which ~1,276 look like
forage names; every disposition lives in `meta:map:status:*`, and `curation/`
holds the rules that put it there, each with a note saying why.

## Five independent axes

These are genuinely orthogonal and want separate fields. Conflating any
two is what produced the original ambiguity.

| axis | question | example | belongs to |
|---|---|---|---|
| **identity** | what *is* this? | urchin hub, maze, transition | the node |
| **disposition** | does it exist, is it open? | Caligos (gone), the Abbey (closed) | the **area**, usually |
| **access** | can *this character* get in? | premium halls, Voln, a locker ticket | the **edge**, per-walker |
| **place** | where is it? | `wehnimers-landing-catacombs` | the **area**, curated |
| **knowledge** | do we know where it is? | 189 Bloodriven sewer rooms | derived, never stored |

**Access is correct and should not be touched.** `Cost::Gated` prices an
exit as `None` when a flag is absent, and `tags.lic` mutates its own
ignore list from the character's profession, gender, race, CHE, guild and
society (`tags.lic:157-183`). A node-level "avoid this room" would answer
identically for every character, which is wrong.

The Chronomage transport hub is the clearest proof. The same rooms and
the same edges cost 100,000 silvers in Prime and nothing in Shattered.
Only a per-walker edge cost can say that; no room flag can.

**Knowledge stays derived.** A room with no exits and nothing pointing at
it cannot be placed, whatever the reason, and `regions::is_connected`
computes that in two lines. `missing` is what happens when you store it
instead: 52 rooms meaning "the mapper could not find it", 11 of which
were walkable, had uids and had exits. Those 11 lost the tag; the other
41 keep it until someone walks them.

## Disposition

```
meta:map:status:live      (default; never written)
meta:map:status:closed    draw, do not route
meta:map:status:gone      do not draw
```

**The verbs are the point.** `closed` and `gone` differ in *what they
disable*, not in flavour, and stating that in the value stops each
consumer inventing its own reading.

| status | draw | route | example | rooms |
|---|:--:|:--:|---|---:|
| `live` | yes | yes | Rumor Woods in April | 31,474 |
| `closed` | yes | **no** | the Abbey, Duskruin between runs | 2,848 |
| `gone` | **no** | no | Caligos Isle, the Feywrot Mire | 2,263 |

This distinction is load-bearing beyond drawing. A pass that skipped
*both* when assigning areas threw away 1,263 rooms that were only
closed — and a closed festival still needs an area to be drawn in when
it reopens.

### Events are durable; closure is not

GemStone runs paid events in even months for about 21 days: February
Duskruin, April Rumor Woods, August Duskruin, October Ebon Gate, with
Rings of Lumnis and Inquisitor in June/December. So a Duskruin room is
open in February and shut in March, and re-curating the map four times a
year is not a plan.

`meta:event:<name>` records *which* event owns an area, which is durable.
A consumer answers "open now?" from that plus a calendar. Nine values:
`anfelt`, `duskruin`, `ebon-gate`, `fof`, `frontier`, `grawood`,
`highman`, `rumor-woods`, `velathae`.

`fest:*` used to say the same thing on 590 rooms and overlapped `event:*`
on **zero** — two vocabularies split by who added each. `fest:` is gone.

### The rule that decides disposition

**A room someone can walk to is live, whatever a tag says.** `retag`
fails the run rather than writing a disposition that contradicts the
graph, and it earned that twice:

- The **Arena of the Abyss** records `location = "Caligos Isle"` but the
  game moved it to Evermore Hollow. A `gone` rule on Caligos would have
  hidden eight live rooms. Its uids (`8225001`–`8225009`) are unchanged,
  so it is the same Arena relocated, not a new copy.
- **Briarmoon Cove**'s rooms carry `location = "the Pinefar Trading
  Post"`, a live 301-room town. A location rule would have hidden Pinefar
  too; a title rule was needed.

Reachability ignores gates — the question is whether *someone* can get
there — and does **not** follow `event transport` edges. Those exist in
the map but only work while the event runs; following them would make
1,378 Duskruin, Ebon Gate and Evermore Hollow rooms look live year-round.
There is no `quest transport` in the map; the game renamed that verb.

Two waivers, both deliberate and opt-in:

- **Dead shops you can still walk into** are `closed`. The tag describes
  the shop; the open door is a forgotten lock. 87 rooms.
- **`gone` rooms have no uid** — all 145, against 21% of the map. The
  game numbers the rooms it has, so its silence corroborates the tag. The
  one exception with a uid was refused until the multi-uid Winding
  Tunnels explained it: room 4 is an unmerged fragment of a tunnel
  consolidated into room 16918, which carries seven uids at once.

### Disconnection has three causes, and only one is a disposition

This is the distinction most worth remembering, because conflating them
hides live content:

1. **No entrance recorded** — the Flotilla is a live OSA town with no
   edge into it. Walk it, do not hide it.
2. **The area is shut or removed** — a disposition.
3. **The entrance is priced or gated** — the Quest Nexus needs a token,
   the Skyship needs a portal from an active Onslaught, the Chronomage
   hub needs a ticket. All live; all access.

## Place

`location` is the game's own answer and stays as it is: useful, gathered
by visiting, and not a partition. 1,357 rooms have none, 661 are
`location_unknowable`, 257 are `check_location`, and 59 distinct values
encode structure in prose ("Arborsong, inside the island town of Mist
Harbor").

**`regions::derive_areas` is not the answer either.** It merges adjacent
units that share a region, and one unlabelled corridor is enough to fuse
two towns: its "Wehnimer's Landing" is 3,050 rooms across 28 locations,
including 186 in Talador. That is why `[Road to Talador]` — a room whose
own `location` says Talador — was drawn in the middle of the town square.

**The seed is Simutronics' own layout.** `crates/mapper/data/areas.tsv`
holds 117 `official layout` areas over 13,689 rooms, hand-drawn and
clean: `wehnimers-landing-town` (171), `-outside-gates` (143),
`-catacombs` (176), `-old-mine-road` (128). `curation/areas.toml` records
only what they do not cover.

### A cave is not a building

The seed already makes this distinction, and it is **not** one of indoor
versus outdoor:

```
wehnimers-landing-catacombs   176 indoor,   0 outdoor   an AREA
wehnimers-landing-old-mines    57 indoor,   0 outdoor   an AREA
wehnimers-landing-town          2 indoor, 169 outdoor   a town
```

A cave system is a place with its own name and extent. A shop's back room
is a room of a building, and the building belongs to whatever area its
front door opens onto. Both are "indoor"; what separates them is
standing, not sense.

So `[[place]]` and `[[building]]` are different kinds of entry rather
than a threshold on one number. A building needs a **real front door** —
an indoor room of the group opening onto an outdoor room of the area.
Adjacency is not belonging: a Krolvin Warship touches
`wehnimers-landing-danjirland` by one exit and is an Open Sea Adventures
ship instance.

`location` was tried as that test and rejected. It correctly drops the
Krolvin, but it also dropped 45 real buildings whose only fault was that
their town is spelled two ways ("Wehnimer's Landing" beside "the town of
Wehnimer's Landing").

A group is **one place** when a single title prefix holds half its rooms
or more, and prefixes of one or two rooms are ignored when deciding.
Without that, a CHE house looks like four places: Cairnfang Manor is 64
rooms of 67, beside a Forsythia Table of one. Every house names the
tables in its lounge.

### Still open

`analysis/area-triage.tsv`, regenerable with
`cargo run -p cena-map-layout --example area_triage`:

```
213  undecided
105  building   (in areas.toml)
 25  skip       (gone; needs no area)
 19  split      (several places welded into one group)
 14  place      (in areas.toml)
```

`split` is not an assignment. The pieces are usually a *mix* — the
192-room group holding Ta'Illistim Keep and the Moonglae Inn is most
likely one place and one building of `taillistim-town`. Splitting by
title prefix is not reliable: the prefix-pieces were each internally
connected in only 19 of the 45 groups tried.

Above place, `landmass → region` is still unbuilt. The Open Sea
Adventures map (cartography by Arianiss Winterfox, May 2025) supplies
what the graph cannot: which places share a continent, and which are
reachable only by sea. Two rooms can be adjacent in the graph — a ship
route, a portal, an urchin — while sitting on different continents.

## Layout: a doorway says which rooms are one building

Not classification, but it answers the same question — what belongs
together — and got it wrong for the same reason.

Components were built over compass edges alone, so every `go door`, `go
archway` and `go yett` was a group boundary. That shattered **71% of
buildings** — 1,061 of 1,497, and 20,431 rooms. The Bard Guild came out
as 97 fragments; the Temple of Tonis as 11 across two sheets, which is
how its Hall of Spring ended up an enormous distance from the Garden
Bower one archway away.

A bearingless doorway cannot place a room, but it does say two rooms are
one place. Which doorways to follow is decided by what they join:

```
indoor <-> indoor      structure, follow
indoor <-> courtyard   structure, follow
indoor <-> open air    a front door, stop
```

Following front doors welds every shop onto its street. Blocking
courtyards splits a temple from the gardens only it reaches. The two look
identical locally, so the difference is taken from the graph: an outdoor
run is the open air when it is a large share of the largest outdoor run
in the map. In `gs.map` those runs are 11,740 rooms, then 308, 242, 211 —
the world outside is forty times the next thing, so the line sits in a
wide gap rather than on a judgement call.

Result: buildings drawn whole 436 → **989**, groups 13,221 → 4,056,
singleton groups 9,335 → 1,950.

**The limitation:** indoor↔indoor also joins a keep to the inn next door.
The 19 `split` groups are exactly that, and nothing distinguishes
"another room of this building" from "the building next door".

## The rest of `meta`

46 namespaces. The ones that classify:

**Identity** — `map:virtual room` (16, exactly the urchin hideouts),
`map:multi-uid` (206), `map:rewritten` (140), `map:no-auto-map` (133),
`maze` (65), `transport` (12), `transition` (2).

`rewritten` is **not** a disposition — Cairnfang Manor is rewritten *and*
live — which is why it became `map:rewritten` rather than a status.

**Behaviour** — `nomagic` (667), `splashy` (235), `trap` (128),
`underwater` (66), `noteleport:fwi` (35), `morphing` (34), `latched`
(21), `jail cell` (11).

**Access** — `che:*` (999 rooms over 32 house keys), `society:*` (136),
`premium` (123),
`mho:*` (129), `pay-to-play` (44), `citizenship:*` (43), `prof:*` (9),
`game:*` (22).

**Lockers** — 551 rooms under one namespace:

```
meta:locker:public                 a public town locker
meta:locker:che                    a shared CHE hall, no one house
meta:locker:che:house:<house>      a house locker room
meta:locker:che:annex:<house>      the house's room in a town annex
meta:locker:che:entrance:<house>   the way into one
```

This replaced four overlapping schemes and 19 tags. The fact that forced
it: `publiclockers` (89 rooms) and bare `meta:locker` (164) overlapped on
**zero** rooms — two disjoint sets naming one concept in two fields, so a
consumer reading `meta` found no public lockers and one reading `tags`
found no house vaults.

**An annex is a shared locker building**, one per town, holding one
private room per Great House — not "a locker in another town". Kraken's
Fall has 12 annex rooms for 12 houses off four `[Inking Den, Annex
Hallway]` corridors; Mist Harbor has 14 for 14. That is why only 59 of
the 208 `locker annex:` rooms had "annex" in the title: the rest are the
berry-room and cubbyhole each house gets inside the building.

`che:<house>:locker` and `:entrance_locker` are **kept**. They carry the
access fact — who may open the door — which is a different question from
whose locker it is.

**mapdb legacy** — `mapname` (234 values), `mapshortname` (198),
`mapcategory` (32), over 233 rooms. Metadata about the old map *files*.
`mapcategory:Events and Festivals` (14 rooms) was left alone: it names no
event to fold into, and only one of its rooms carried a `fest:` key.

## The rest of `tags`

1,910 values, ~1,276 of them forage names.

**Functional:** `no forageables` (5,172), `urchin-access` (517), `private
property` (318), `node` (212), `supernode` (91), `table` (41), `bank`
(36), plus ~30 service tags used as `;go2` destinations.

### The guild "split" was not what it looked like

An earlier draft of this document said every guild was "split almost
exactly in half", so a consumer filtering on `bardguild` would "silently
find half the bard guilds". **That was wrong.** `bardguild` and `bard
guild` were on the *same ten rooms*, all ten — double-tagged, not
divided. Either filter already found everything.

Only six of the 44 spelling families were genuine splits. `clericshop`
was the only one that cost anything: 9 and 4 with 3 shared, so six rooms
were reachable by one spelling only. 217 tags were normalised, 1,964 →
1,910 distinct. The case for a closed enum still stands on hygiene, but
it is weaker than the original claim made it sound.

Two kinds of near-duplicate are deliberately left:

- **99 `peer ...` entries are not tags.** They are disambiguation probes
  holding a regular expression, used to tell apart rooms sharing a title
  — four `[Annex, Booth]`, six `[Enemy Ship, Quarters]`, five `[The
  Rift]`. Two differ only by `^...$`, which is a different pattern, not a
  different spelling, so merging them would change what matches.
- **Ten proper names** (`Whirlin`, `Sadie`, `Khylynnia`, `WillowHall`)
  where capitalisation may carry meaning.

### `jail` is correct modelling with an unfortunate name

11 rooms carry the tag, 11 carry `meta:jail cell`, and the overlap is
**zero** — because they are different rooms.

```
tag jail        -> [Wehnimer's, Constabulary]   (pay the fine)
meta jail cell  -> [Wehnimer's, Jail Cell]      (serve the time)
```

Eleven towns, one of each. Leave it alone.

### Duplicate stubs: 264, and ten of them are not duplicates

Rooms titled `duplicate of NNNN`. The title names its own target, so
which room is current is recorded, not inferred — do **not** use the id
ordering, which agrees 262 times out of 264 and is not a rule.

```
              stubs   targets
  uid           0       242
  exits       139       264
  inbound       5       263
```

253 were deleted. Not one had a uid; the five with inbound edges were
pointed at only by other stubs, so no live room lost an edge.

**The ten exceptions have a description**, and comparing it with the
target's shows they are day/night variants:

```
stub 5896   "The normally crowded intersection of wey and var is
             nearly deserted.  A row of pennants ..."
room 3490   "The crowded intersection of wey and var teems with
             people, each hurrying ...  A row of pennants ..."
```

Same place, different time of day. That is why the test is the
description and not the title: a room with a body of text is a room
someone visited. Deleting on the title alone would have taken ten live
Ta'Vaalor streets. (An earlier draft put this count at 140 and called
them all bookkeeping.)

## Housekeeping still worth doing

- **Update `@herb_list`** in `tags.lic` — missing at least 340 forage
  names, which affects `;tags --sense`, not just the census. Outside this
  repo.
- **Two rooms keep a `no-auto-map` tag** — 30727 and 31846. The
  disposition pass claimed them first, so the loose-tag pass skipped
  them.
- **The `peer ...` probes want a field** rather than sitting in `tags`.
  That is a `cena-map` change.
- **421 rooms need a `location`** the game knows and the file does not:
  252 with none and 169 to verify (`analysis/worklist.tsv`, itself taken
  before the migrations). Map-wide, 1,357 rooms have no location and 257
  are flagged `check_location`. Walking is the only way to close these —
  the wiki covers 0 of them, because the rooms people wrote articles
  about are the ones already well recorded.

## What the map cannot tell us

Several facts in `curation/` came from a person and appear in the map
nowhere: the paid-event calendar, that Summit Academy's last event ran in
2017, Briarmoon Cove's in 2020 and Highman Games' in 2010, that `event
transport` only works while an event is live, that an annex is a shared
building, and that Silvergate Inn and Silvergate Manor are one house.

They are recorded in the `note` fields of `curation/*.toml`, because in
six months the interesting question about `Summit Academy = gone` is not
what it does but how anyone knew.
