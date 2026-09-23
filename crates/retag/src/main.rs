//! `retag plan` / `retag apply`.
//!
//! `plan` writes nothing and prints what would change. `apply` does the
//! same work and then saves. They share [`cena_retag::plan`], so the
//! report cannot describe one thing while the write does another.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cena_map::{Map, binary};
use cena_retag::{Curation, Plan, Verdict, apply, plan};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str);
    let write = match mode {
        Some("plan") => false,
        Some("apply") => true,
        _ => {
            eprintln!("usage: retag <plan|apply> [--map gs.map] [--curation curation/]");
            return ExitCode::from(2);
        }
    };

    let map_path = flag(&args, "--map").unwrap_or_else(|| PathBuf::from("gs.map"));
    let curation_dir = flag(&args, "--curation").unwrap_or_else(|| PathBuf::from("curation"));

    match run(&map_path, &curation_dir, write) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(
    map_path: &Path,
    curation_dir: &Path,
    write: bool,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let curation = Curation::load(curation_dir)?;
    let bytes = std::fs::read(map_path)?;
    let map = binary::decode(&bytes)?;

    // The safety property this whole tool rests on: if re-encoding an
    // untouched map does not reproduce the input, then a diff in the
    // output cannot be attributed to the curation, and nothing below is
    // trustworthy. Checked on the real file every run, not just in tests.
    let control = binary::encode(&map)?;
    if control != bytes {
        return Err(format!(
            "{} does not round-trip: re-encoding it unchanged gives {} bytes, not {}. \
             Refusing to write, since any diff would be indistinguishable from corruption.",
            map_path.display(),
            control.len(),
            bytes.len()
        )
        .into());
    }

    let plan = plan(&map, &curation);
    report(&plan, &map);

    if !plan.violations.is_empty() {
        eprintln!(
            "\nrefusing to {}: {} rule(s) would mark a walkable room closed or gone.",
            if write { "apply" } else { "plan" },
            plan.violations.len()
        );
        return Ok(ExitCode::FAILURE);
    }

    if !write {
        println!("\nplan only; nothing written. Re-run with `apply` to save.");
        return Ok(ExitCode::SUCCESS);
    }
    if plan.is_empty() {
        println!("\nnothing to do.");
        return Ok(ExitCode::SUCCESS);
    }

    let rooms = apply(map.rooms(), &plan);
    let updated = Map::from_rooms(rooms)?;
    let out = binary::encode(&updated)?;

    // Write via a temporary file in the same directory, then rename. A
    // half-written 23MB map is worse than no write at all, and a rename
    // on the same filesystem is the one step that cannot leave one.
    let tmp = map_path.with_extension("map.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, map_path)?;
    println!(
        "\nwrote {} ({} rooms, {} bytes)",
        map_path.display(),
        updated.len(),
        out.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn report(plan: &Plan, map: &Map) {
    if !plan.empty_rules.is_empty() {
        println!("== rules matching nothing ==");
        for rule in &plan.empty_rules {
            println!("  {rule}");
        }
        println!();
    }

    if !plan.violations.is_empty() {
        println!("== INVARIANT VIOLATIONS ==");
        println!("A room someone can walk to is live, whatever a rule says.\n");
        for violation in &plan.violations {
            println!(
                "  {} = {}  ({} matched, {} of them walkable)",
                violation.rule,
                violation.verdict.as_str(),
                violation.matched,
                violation.reachable.len()
            );
            for (id, title) in violation.reachable.iter().take(5) {
                println!("      {id}  {title}");
            }
            if violation.reachable.len() > 5 {
                println!("      ... and {} more", violation.reachable.len() - 5);
            }
        }
        println!();
    }

    println!("== verdicts ==");
    for verdict in [Verdict::Live, Verdict::Closed, Verdict::Gone] {
        let n = plan.verdicts.get(&verdict).copied().unwrap_or(0);
        println!("  {:6} {n:6}", verdict.as_str());
    }
    let unassigned = map.len() - plan.verdicts.values().sum::<usize>();
    println!("  {:6} {unassigned:6}  (no rule; reads as live)", "--");

    println!("\n== changes ==");
    for (kind, n) in plan.counts() {
        println!("  {kind:14} {n:6}");
    }
    if plan.changes.is_empty() {
        println!("  (none)");
    }
}

fn flag(args: &[String], name: &str) -> Option<PathBuf> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(PathBuf::from)
}
