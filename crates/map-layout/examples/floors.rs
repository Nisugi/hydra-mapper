//! How many rooms make a floor? A measure for the author's question
//! (2026-09-29): *"What constitutes a floor change? I think x amount of rooms
//! on the same floor. How many rooms is the right amount though?"*
//!
//! A **level** is the rooms joined without going up or down: every walking
//! exit joins its two rooms, except an explicit `up`/`down` and a climb or
//! `go` through something that changes height (stairs, ladder, hatch...;
//! `plan/21` §3e's list). Each explicit `up`/`down` between two levels is a
//! candidate floor change.
//!
//! With a threshold N, a level of N rooms or more is a floor of its own and
//! one under fewer sits on the floor it was reached from. For each N this
//! prints how many levels become floors, and the tallest stack of floors
//! anywhere -- the mine that went fourteen floors down.
//!
//! Run by hand from the repo root, where `gs.map` is:
//! `cargo run --release -p cena-map-layout --example floors`.

#![allow(
    clippy::expect_used,
    reason = "a one-shot measure run by hand from the repo root: a missing gs.map is a \
              mistyped command, and the panic says so"
)]

use std::collections::{BTreeSet, HashMap, VecDeque};

use cena_map::{Crossing, RoomId};
use cena_map_layout::{Dir, DirectionMap};

/// Nouns a `go` or `climb` changes height through (`plan/21` §3e).
const HEIGHT: &[&str] = &[
    "staircase",
    "stairs",
    "stair",
    "stairway",
    "steps",
    "ladder",
    "hatch",
    "hatchway",
    "hole",
    "trapdoor",
    "ramp",
    "tree",
    "rope",
    "cliff",
    "pit",
    "well",
    "shaft",
    "chute",
    "slope",
];

fn main() {
    let bytes = std::fs::read("gs.map").expect("gs.map in the working folder");
    let map = cena_map::binary::decode(&bytes).expect("a HYDRAMAP file");
    let dirs = DirectionMap::build(&map);

    // Levels: union-find over the exits that do not change height.
    let ids: Vec<RoomId> = map.rooms().iter().map(|r| r.id).collect();
    let index: HashMap<RoomId, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    let mut vertical: Vec<(RoomId, RoomId, i32)> = Vec::new();
    let mut stairs = 0usize;
    // `FLOORS_INDOOR=1`: an up or down with either end outdoors ("Obvious
    // paths") is the land rising, not a floor, and joins its rooms' levels.
    let indoor_only = std::env::var_os("FLOORS_INDOOR").is_some();
    let indoor = |id: RoomId| {
        map.room(id)
            .is_some_and(|r| r.paths.iter().any(|p| p.contains("Obvious exits")))
    };
    for room in map.rooms() {
        for exit in &room.exits {
            if !index.contains_key(&exit.to) || exit.to == room.id {
                continue;
            }
            let rising = indoor_only && !(indoor(room.id) && indoor(exit.to));
            match dirs.get(room.id, exit.to) {
                Some(Dir::Up | Dir::Down) if rising => {
                    union(&mut parent, index[&room.id], index[&exit.to]);
                }
                Some(Dir::Down) => vertical.push((room.id, exit.to, 1)),
                Some(Dir::Up) => vertical.push((room.id, exit.to, -1)),
                _ if changes_height(&exit.crossing) => stairs += 1,
                _ if matches!(exit.crossing, Crossing::Command(_) | Crossing::Steps(_)) => {
                    union(&mut parent, index[&room.id], index[&exit.to]);
                }
                _ => {}
            }
        }
    }
    let level_of: Vec<usize> = (0..ids.len()).map(|i| find(&mut parent, i)).collect();
    let mut size: HashMap<usize, usize> = HashMap::new();
    for &l in &level_of {
        *size.entry(l).or_default() += 1;
    }
    let level = |id: RoomId| level_of[index[&id]];

    // The level-to-level graph of explicit up/down.
    let mut across: HashMap<usize, Vec<(usize, i32)>> = HashMap::new();
    let mut reached: Vec<usize> = Vec::new();
    for &(a, b, delta) in &vertical {
        let (la, lb) = (level(a), level(b));
        if la == lb {
            continue;
        }
        across.entry(la).or_default().push((lb, delta));
        across.entry(lb).or_default().push((la, -delta));
        reached.push(size[&lb]);
    }

    println!(
        "{} rooms; {} levels; {} explicit up/down exits between two levels; \
         {} stairs/ladders/hatches with no stated direction (not counted below)",
        ids.len(),
        size.len(),
        reached.len(),
        stairs
    );
    print_sizes(&reached);

    let area_of = |l: usize| -> String {
        ids.iter()
            .enumerate()
            .filter(|&(i, _)| level_of[i] == l)
            .find_map(|(_, id)| {
                map.room(*id)?
                    .meta
                    .iter()
                    .find_map(|m| m.strip_prefix("area:"))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "(no area)".to_owned())
    };

    println!("\nwith a floor at N rooms or more:");
    println!(
        "  {:>3}  {:>6}  {:>12}  {:>14}  tallest",
        "N", "floors", "stacks of 3+", "tallest stack"
    );
    for n in [1usize, 2, 3, 4, 5, 6, 8, 10, 15, 20, 30] {
        let stacks = stack(&across, &size, n);
        println!(
            "  {n:>3}  {:>6}  {:>12}  {:>14}  {}",
            stacks.floors,
            stacks.tall,
            stacks.tallest + 1,
            stacks.at.map_or_else(String::new, area_of)
        );
    }
}

/// How many up/downs lead to a level of each size.
fn print_sizes(reached: &[usize]) {
    println!("\nsize of the level an up/down leads to:");
    for (lo, hi) in [
        (1, 1),
        (2, 2),
        (3, 3),
        (4, 5),
        (6, 10),
        (11, 20),
        (21, 50),
        (51, usize::MAX),
    ] {
        let n = reached.iter().filter(|&&s| s >= lo && s <= hi).count();
        let label = if hi == usize::MAX {
            format!("{lo}+")
        } else if lo == hi {
            format!("{lo}")
        } else {
            format!("{lo}-{hi}")
        };
        println!("  {label:>6} rooms: {n:>5}");
    }
}

/// The floors with a floor at `n` rooms or more.
struct Stacks {
    /// Floors beyond the first, summed over every stack.
    floors: usize,
    /// Stacks of three floors or more.
    tall: usize,
    /// The tallest stack's height less one, and a level of it.
    tallest: i32,
    at: Option<usize>,
}

/// Floors by a breadth-first walk from each unvisited level: a level under
/// `n` rooms keeps the floor it was reached from.
fn stack(
    across: &HashMap<usize, Vec<(usize, i32)>>,
    size: &HashMap<usize, usize>,
    n: usize,
) -> Stacks {
    let mut floor: HashMap<usize, i32> = HashMap::new();
    let mut stacks = Stacks {
        floors: 0,
        tall: 0,
        tallest: 0,
        at: None,
    };
    let mut starts: Vec<usize> = across.keys().copied().collect();
    starts.sort_unstable();
    for start in starts {
        if floor.contains_key(&start) {
            continue;
        }
        floor.insert(start, 0);
        let mut cluster = vec![start];
        let mut queue = VecDeque::from([start]);
        while let Some(at) = queue.pop_front() {
            for &(next, delta) in across.get(&at).map_or(&[][..], Vec::as_slice) {
                if floor.contains_key(&next) {
                    continue;
                }
                let step = if size[&next] >= n { delta } else { 0 };
                floor.insert(next, floor[&at] + step);
                cluster.push(next);
                queue.push_back(next);
            }
        }
        let distinct: BTreeSet<i32> = cluster.iter().map(|l| floor[l]).collect();
        stacks.floors += distinct.len().saturating_sub(1);
        let span = distinct.last().unwrap_or(&0) - distinct.first().unwrap_or(&0);
        if span >= 2 {
            stacks.tall += 1;
        }
        if span > stacks.tallest {
            stacks.tallest = span;
            stacks.at = Some(start);
        }
    }
    stacks
}

fn changes_height(crossing: &Crossing) -> bool {
    let Crossing::Command(command) = crossing else {
        return false;
    };
    let mut words = command.split_whitespace();
    let verb = words.next().unwrap_or_default().to_ascii_lowercase();
    matches!(verb.as_str(), "go" | "climb")
        && words.any(|w| HEIGHT.contains(&w.to_ascii_lowercase().as_str()))
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra] = rb;
    }
}
