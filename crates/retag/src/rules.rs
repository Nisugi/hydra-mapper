//! The curation files: what they say and how a room is matched.
//!
//! Rules are data, not code, so adding a place is an edit to a `.toml`
//! and not a recompile. Each carries a `note` explaining the decision,
//! because six months on the interesting question about
//! `Summit Academy = gone` is not what it does but how anyone knew.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cena_map::Room;
use serde::Deserialize;

/// What the map should say about a room's existence.
///
/// Three values, distinguished by what they disable rather than by
/// flavour: `Closed` still draws, `Gone` does not. Stating that in the
/// value is what stops each consumer inventing its own reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Draw and route. The default, never written to a room: a map of
    /// 36,838 rooms should not carry 31,000 copies of "this one is fine".
    Live,
    /// Draw, do not route. Shut now, expected back -- an event area
    /// between runs.
    Closed,
    /// Do not draw. Removed from the game, or a venue whose event stopped
    /// running with nothing scheduled to bring it back.
    Gone,
}

impl Verdict {
    /// The `meta` string this verdict writes, or `None` for the default.
    #[must_use]
    pub fn meta(self) -> Option<&'static str> {
        match self {
            Verdict::Live => None,
            Verdict::Closed => Some("map:status:closed"),
            Verdict::Gone => Some("map:status:gone"),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Live => "live",
            Verdict::Closed => "closed",
            Verdict::Gone => "gone",
        }
    }
}

/// One disposition rule.
///
/// The three selectors intersect: a rule with both `location` and `title`
/// matches only rooms satisfying both. That is what scopes
/// `title = "[Academy,"` to Vornavis and leaves the nine Rone Academy
/// rooms of the same name alone.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub verdict: Verdict,
    /// Match on the room's `location`, after `locations.toml` has been
    /// applied. Exact, not a substring: locations like `Vornavis` and
    /// `Vornavis Proper` are different places.
    #[serde(default)]
    pub location: Option<String>,
    /// Match on a title prefix. The escape hatch for places whose
    /// `location` belongs to somewhere else -- Briarmoon Cove's rooms say
    /// `the Pinefar Trading Post`, a live town, so only a title rule can
    /// name them without hiding Pinefar's 301 rooms too.
    #[serde(default)]
    pub title: Option<String>,
    /// Match specific room ids. Last resort, for rooms no pattern reaches.
    /// Ids are not stable across a map rebuild; prefer the other two.
    #[serde(default)]
    pub ids: Option<Vec<u32>>,
    /// Which paid event this area belongs to, written as
    /// `meta:event:<name>`.
    ///
    /// Separate from the verdict because it is durable and the verdict is
    /// not: `GemStone` runs events in even months for roughly 21 days, so a
    /// Duskruin room is open in February and shut in March. Recording the
    /// event lets a consumer answer "open now?" from a calendar instead of
    /// this file being re-curated four times a year.
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// The rooms this rule named have since been deleted, so it matches
    /// nothing and never will again.
    ///
    /// Kept rather than removed because the rule IS the record of why
    /// 2,629 rooms are not in the map. Deleting it would leave the
    /// deletion unexplained in the only files anyone reads to find out;
    /// leaving it unmarked would make `the_shipped_curation_has_no_dead_rules`
    /// fail forever, which trains people to ignore it.
    ///
    /// Not the same as a rule that matches nothing by mistake. That is a
    /// typo, and the test still catches it.
    #[serde(default)]
    pub spent: bool,
}

impl Rule {
    /// Whether this rule names `room`. `location_of` supplies the
    /// corrected location, since disposition keys on where a room *is*,
    /// not on a stale recorded answer.
    pub fn matches(&self, room: &Room, location_of: &dyn Fn(&Room) -> String) -> bool {
        if let Some(ids) = &self.ids
            && !ids.contains(&room.id.0)
        {
            return false;
        }
        if let Some(loc) = &self.location
            && &location_of(room) != loc
        {
            return false;
        }
        if let Some(title) = &self.title
            && !room.title.iter().any(|t| t.starts_with(title))
        {
            return false;
        }
        // A rule with no selector at all would match every room; that is
        // never intended and is rejected when the file is read.
        self.ids.is_some() || self.location.is_some() || self.title.is_some()
    }

    /// How the rule reads in a report line.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(l) = &self.location {
            parts.push(format!("location={l:?}"));
        }
        if let Some(t) = &self.title {
            parts.push(format!("title={t:?}"));
        }
        if let Some(i) = &self.ids {
            parts.push(format!("{} ids", i.len()));
        }
        parts.join(" + ")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatusFile {
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

/// One location correction: the recorded `location` is stale and this is
/// the right one.
///
/// Keyed by uid where possible. A uid survives a map rebuild and a room id
/// does not, so `ids` is for the rooms the game has never numbered.
#[derive(Debug, Clone, Deserialize)]
pub struct LocationFix {
    #[serde(default)]
    pub uids: Option<Vec<i64>>,
    #[serde(default)]
    pub ids: Option<Vec<u32>>,
    pub location: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocationFile {
    #[serde(default, rename = "fix")]
    pub fixes: Vec<LocationFix>,
}

/// What to do with a disposition tag no area rule reaches.
///
/// The conditions exist because walkability, not the tag's age, decides
/// the hard cases. A room someone can stand in is live whatever a tag
/// written years ago claims, so a conversion to `closed` or `gone` is
/// allowed only where nothing contradicts it.
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each is an independent precondition on one room, read straight               from the TOML; folding them into an enum would make the file               less legible to the person editing it, which is the point"
)]
pub struct TagConversion {
    /// The tag this rule consumes.
    #[serde(default)]
    pub from_tag: String,
    /// The status it becomes: `gone`, `closed`, or `none` to write no
    /// status, for a tag that was never a disposition.
    #[serde(default)]
    pub to_status: String,
    /// A meta key to write instead of a status. For a tag that belongs in
    /// a namespace rather than being a disposition at all.
    #[serde(default)]
    pub to_meta: Option<String>,
    /// Rewrite every meta with this prefix to `to_meta_prefix`, keeping
    /// the value. `fest:anfelt` -> `event:anfelt`.
    #[serde(default)]
    pub from_meta_prefix: Option<String>,
    #[serde(default)]
    pub to_meta_prefix: Option<String>,
    #[serde(default)]
    pub only_if_unwalkable: bool,
    /// Convert even though the room is walkable.
    ///
    /// Normally walkability vetoes a `closed`/`gone` status, because a
    /// room someone can stand in is live. The dead shops are the
    /// deliberate exception: `closed` is a claim about the SHOP -- no
    /// inventory, no merchant -- and the door being enterable is a game
    /// bug, a lock that was forgotten or reverted. An empty room you can
    /// walk into hurts nothing, so the tag is better evidence than the
    /// edge.
    ///
    /// Opt-in per conversion, never a default, so the veto still catches
    /// the case it was written for: a rule that quietly names a live area
    /// it did not mean to.
    #[serde(default)]
    pub allow_walkable: bool,
    #[serde(default)]
    pub only_if_walkable: bool,
    /// Require the game to have numbered the room -- evidence it is real.
    #[serde(default)]
    pub require_uid: bool,
    /// Require that the game has NEVER numbered the room.
    ///
    /// The corroboration behind `gone`: 21% of the map lacks a uid, but
    /// all 145 `gone`-tagged rooms do, so the absence is evidence rather
    /// than noise. It is what makes `allow_walkable` safe there -- not
    /// "trust the tag", but "trust the tag where the game agrees".
    #[serde(default)]
    pub require_no_uid: bool,
    #[serde(default)]
    pub require_exits: bool,
    #[serde(default)]
    pub note: Option<String>,
}

impl TagConversion {
    /// The status to write, or `None` to drop the tag and write nothing.
    #[must_use]
    pub fn verdict(&self) -> Option<Verdict> {
        match self.to_status.as_str() {
            "gone" => Some(Verdict::Gone),
            "closed" => Some(Verdict::Closed),
            _ => None,
        }
    }

    /// Whether this conversion applies to `room`.
    #[must_use]
    pub fn applies(&self, room: &Room, walkable: bool) -> bool {
        if self.from_tag.is_empty() || !room.tags.contains(&self.from_tag) {
            return false;
        }
        if self.only_if_unwalkable && walkable && !self.allow_walkable {
            return false;
        }
        if self.only_if_walkable && !walkable {
            return false;
        }
        if self.require_uid && room.uid.is_empty() {
            return false;
        }
        if self.require_no_uid && !room.uid.is_empty() {
            return false;
        }
        if self.require_exits && room.exits.is_empty() {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TagFile {
    #[serde(default, rename = "convert")]
    pub conversions: BTreeMap<String, TagConversion>,
}

/// One tag spelling normalisation: every `from` becomes `to`.
#[derive(Debug, Clone, Deserialize)]
pub struct Rename {
    pub to: String,
    pub from: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpellingFile {
    #[serde(default, rename = "rename")]
    pub renames: Vec<Rename>,
}

/// Everything the curation directory says.
#[derive(Debug, Clone, Default)]
pub struct Curation {
    pub locations: LocationFile,
    pub status: StatusFile,
    pub tags: TagFile,
    pub spellings: SpellingFile,
    pub lockers: LockerFile,
    pub regions: RegionFile,
}

/// `generated/regions.toml`: the official mapdb's `loc` field, keyed by
/// uid.
///
/// Not a replacement for `location`. The two answer different questions --
/// `location` is what the game's verb replies, `loc` is an administrative
/// region -- and this pass writes `meta:region:<name>` beside the existing
/// value so the two can be compared on real data before either is trusted
/// over the other.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RegionFile {
    #[serde(default, rename = "region")]
    pub regions: Vec<Region>,
    /// Places that exist once in our map and many times in theirs.
    #[serde(default, rename = "plane")]
    pub planes: Vec<Plane>,
}

/// A place the game instances per town, which we hold one copy of.
///
/// The Elemental Confluence is nine instances of 63 rooms, one hanging
/// off each town, and the mapdb labels each with that town's name. We
/// merged them: all 53 of our rooms carry all nine uids. So no town's
/// name is more true than the others', and taking the first would have
/// drawn a plane inside Wehnimer's Landing -- which it did, until
/// someone looked at the map and saw it under the town square.
#[derive(Debug, Clone, Deserialize)]
pub struct Plane {
    /// Matched against the room title.
    pub title: String,
    /// The region to write instead.
    pub region: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub name: String,
    pub uids: Vec<i64>,
}

impl Curation {
    /// Read `curation/` and check what can be checked without the map:
    /// that every rule selects something, and that no file is malformed.
    ///
    /// # Errors
    ///
    /// If a file is unreadable or malformed, or if a rule has no selector
    /// (which would silently match all 36,838 rooms) or a location fix
    /// names neither a uid nor an id (which would reach none).
    pub fn load(dir: &Path) -> Result<Curation, LoadError> {
        let locations: LocationFile = read_toml(&dir.join("locations.toml"))?;
        let status: StatusFile = read_toml(&dir.join("status.toml"))?;
        let tags: TagFile = read_toml(&dir.join("tags.toml"))?;
        let spellings: SpellingFile = read_toml(&dir.join("spellings.toml"))?;
        let lockers: LockerFile = read_toml(&dir.join("lockers.toml"))?;
        // Generated from the mapdb rather than hand-written, so it lives
        // in its own directory: a reviewer should not have to wonder which
        // of 345KB of uids someone decided by hand.
        let regions: RegionFile = read_toml(&dir.join("generated/regions.toml"))?;

        for (i, rule) in status.rules.iter().enumerate() {
            if rule.location.is_none() && rule.title.is_none() && rule.ids.is_none() {
                return Err(LoadError::RuleSelectsEverything(i + 1));
            }
        }
        for (i, fix) in locations.fixes.iter().enumerate() {
            if fix.uids.is_none() && fix.ids.is_none() {
                return Err(LoadError::FixSelectsNothing(i + 1));
            }
        }
        for rename in &spellings.renames {
            if rename.from.contains(&rename.to) {
                return Err(LoadError::RenameToItself(rename.to.clone()));
            }
        }
        Ok(Curation {
            locations,
            status,
            tags,
            spellings,
            lockers,
            regions,
        })
    }

    /// The corrected location of a room, as the rules should see it.
    ///
    /// Applied before disposition on purpose: a `gone` rule on Caligos
    /// Isle would otherwise hide the eight Arena of the Abyss rooms, which
    /// carry that location but were moved to Evermore Hollow and are live.
    #[must_use]
    pub fn location_of(&self, room: &Room) -> String {
        for fix in &self.locations.fixes {
            let uid_match = fix
                .uids
                .as_ref()
                .is_some_and(|u| room.uid.iter().any(|r| u.contains(&r.0)));
            let id_match = fix.ids.as_ref().is_some_and(|i| i.contains(&room.id.0));
            if uid_match || id_match {
                return fix.location.clone();
            }
        }
        room.location.clone().unwrap_or_default()
    }

    /// Room ids this curation gives an explicit location.
    #[must_use]
    pub fn fixed_rooms(&self, rooms: &[Room]) -> BTreeSet<u32> {
        rooms
            .iter()
            .filter(|r| {
                self.locations.fixes.iter().any(|f| {
                    f.uids
                        .as_ref()
                        .is_some_and(|u| r.uid.iter().any(|x| u.contains(&x.0)))
                        || f.ids.as_ref().is_some_and(|i| i.contains(&r.id.0))
                })
            })
            .map(|r| r.id.0)
            .collect()
    }
}

fn read_toml<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, LoadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(LoadError::Unreadable(path.display().to_string(), e)),
    };
    toml::from_str(&text).map_err(|e| LoadError::Malformed(path.display().to_string(), Box::new(e)))
}

#[derive(Debug)]
pub enum LoadError {
    Unreadable(String, std::io::Error),
    Malformed(String, Box<toml::de::Error>),
    /// A rule with no selector would match all 36,838 rooms.
    RuleSelectsEverything(usize),
    /// A location fix naming neither a uid nor an id cannot reach a room.
    FixSelectsNothing(usize),
    /// A rename listing its own target would loop.
    RenameToItself(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Unreadable(p, e) => write!(f, "{p}: {e}"),
            LoadError::Malformed(p, e) => write!(f, "{p}: {e}"),
            LoadError::RuleSelectsEverything(n) => {
                write!(f, "status.toml rule {n} has no selector: it would match every room")
            }
            LoadError::FixSelectsNothing(n) => {
                write!(f, "locations.toml fix {n} names neither uids nor ids")
            }
            LoadError::RenameToItself(t) => {
                write!(f, "spellings.toml: {t:?} lists itself as a source spelling")
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// One locker consolidation rule: where the fact is now, and where it
/// should be.
#[derive(Debug, Clone, Deserialize)]
pub struct LockerRule {
    /// A fixed target, for lockers nobody owns.
    #[serde(default)]
    pub to_meta: Option<String>,
    /// A target with `{house}` in it, filled from the source key.
    #[serde(default)]
    pub to_meta_pattern: Option<String>,
    #[serde(default)]
    pub from_tags: Vec<String>,
    #[serde(default)]
    pub from_meta: Vec<String>,
    /// `che:{house}:locker` or `locker annex:{House}` -- `{house}` matches
    /// a `snake_case` key, `{House}` a display name needing translation.
    /// Only treat `from_meta` as a match when the room carries no
    /// `che:*` key. Bare `meta:locker` means "is a locker", not "is
    /// public", and 112 of its 164 rooms are house vaults.
    #[serde(default)]
    pub from_meta_requires_no_che: bool,
    #[serde(default)]
    pub from_meta_pattern: Option<String>,
    #[serde(default)]
    pub from_meta_pattern_alt: Option<String>,
    /// Only apply the patterns when the room also carries this meta.
    /// Guards a broad alternate like `che:{House}`, which alone would
    /// match every room of every house.
    #[serde(default)]
    pub require_meta: Option<String>,
    /// Remove the source once the fact has moved. False where the source
    /// carries an access fact too, which is not ours to delete.
    #[serde(default)]
    pub drop_source: bool,
    #[serde(default)]
    pub note: Option<String>,
}

/// Tags naming a house's lockers in one of three spellings.
#[derive(Debug, Clone, Deserialize)]
pub struct HouseTags {
    pub house: String,
    /// `house` (the default) or `entrance`.
    #[serde(default)]
    pub kind: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LockerFile {
    #[serde(default, rename = "locker")]
    pub rules: Vec<LockerRule>,
    #[serde(default, rename = "house_tags")]
    pub house_tags: Vec<HouseTags>,
    /// Display name -> `che:` key, for the four no room pairs up.
    #[serde(default)]
    pub house_keys: BTreeMap<String, String>,
}
