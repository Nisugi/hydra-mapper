# analysis/

Measurements of `gs.map` and material gathered about it. Nothing here is
read by the code: these are working notes that happen to be tabular, kept
because the numbers in `docs/room-classification.md` are only checkable if
what produced them is still around.

Two kinds, and the difference matters when one looks stale.

## Regenerable from the map

Snapshots of a moment. If `gs.map` has moved on, these are wrong and
should be regenerated rather than trusted or patched.

| file | what |
|---|---|
| `tag-census.tsv` | every distinct `tag`, with counts and how many locations it appears in |
| `tags-only.tsv` | the same, narrowed to classification tags — herb and creature names excluded |
| `meta-census.tsv` | every `meta` namespace and value, with counts |
| `forage-names.txt` | herb and forage names from `tags.lic`'s `@herb_list`, `@night_only` and `@day_only`, used to exclude room *contents* from the tag census |

The census files were measured against the map as it stood on
**2026-09-22, before `retag` ran** — 36,838 rooms, 1,976 distinct tags.
The map now has 36,585 rooms and no `gone`, `closed` or `urchin-hideout`
tags at all, so the disposition rows in these files are a record of what
was, not what is.

## Worklists

Things a person acts on. These do not regenerate into the same file,
because the point of them is the column a human fills in.

| file | what |
|---|---|
| `worklist.tsv` | rooms needing a `location` the game knows and the file does not: 421 reachable, 754 with none, 206 flagged `check_location`. Each row names a located neighbour and the command to reach it. Closing these needs walking, not curation — the wiki covers 0 of them, because the rooms people wrote articles about are the ones already recorded. |
| `curation-triage.tsv` | connected components with no verdict in `curation/status.toml`, for deciding live / closed / gone / delete. Regenerable, but the `verdict` column is hand-filled, so a regeneration loses work unless the decisions have been moved into `curation/` first. |

## Gathered from the wiki

From `E:\Gemstone\data\wiki_clean`, 19,618 article dumps.

| file | what |
|---|---|
| `wiki-hunting-areas.tsv` | the curated `region → area` hierarchy parsed out of `List of hunting areas.txt`: 112 areas under 8 regions, human-authored. 58 already agree by name with what `derive_areas` finds from the graph alone, which is the strongest corroboration of that grouping so far, and a ready-made seed for a curated `area` field. |
| `wiki-roomnums.txt` | the room numbers appearing in wiki articles. 670 of 697 join to the map, but **two numbering schemes share the field**: values ≥ 40,000 are uids, values < 40,000 are map ids from before uids existed. Resolve small numbers as ids and large ones as uids. |
| `wiki-geography-categories.md` | a raw category page dump. Scraped source, not documentation — it lived in `docs/` by accident. |
