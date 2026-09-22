//! What is in focus on the one sheet: the rooms drawn as squares.
//!
//! An area is one continuous map -- every room has one cell, on one
//! sheet -- and the part being looked at is a *unit*: the streets, one
//! building, or a hunting area from the official layout. Everything else
//! is a dot on the same roads. An area opens with nothing in focus -- the
//! whole of it as skeleton -- and clicking a dot enters the unit it
//! belongs to; Back returns. Nothing is laid out again on a change of
//! focus, and nothing moves.
//!
//! The engine's units (`cena_map_layout::scene::Unit`: the streets, and
//! one per building) come first, in its order, so an engine unit index is
//! a focus index too. The official layout's areas are layered on after
//! them, cut down to the rooms this area holds.

use std::collections::HashSet;

use cena_map::RoomId;
use cena_map_layout::MapScene;
use cena_map_layout::scene::{STREETS, UnitKind};

use crate::areas::Area;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusKind {
    Streets,
    Building,
    /// An official layout area (`areas.tsv`) overlapping this one.
    Official,
}

#[derive(Debug, Clone)]
pub struct FocusUnit {
    pub kind: FocusKind,
    pub name: String,
    pub rooms: HashSet<RoomId>,
}

/// The units of the shown area, which one is in focus, and the way back.
#[derive(Debug, Clone)]
pub struct Focus {
    pub units: Vec<FocusUnit>,
    /// `None`: nothing in focus, everything a dot.
    current: Option<usize>,
    stack: Vec<Option<usize>>,
    /// Rooms a door leads into, and street rooms hosting a doorway: the
    /// ways in, drawn larger when out of focus.
    doors: HashSet<RoomId>,
}

impl Focus {
    /// The engine's units, then every official area with a room here.
    #[must_use]
    pub fn build(scene: &MapScene, official: &[Area]) -> Focus {
        let mut units: Vec<FocusUnit> = scene
            .units
            .iter()
            .map(|u| FocusUnit {
                kind: match u.kind {
                    UnitKind::Streets => FocusKind::Streets,
                    UnitKind::Building => FocusKind::Building,
                },
                name: u.name.clone(),
                rooms: u.rooms.iter().copied().collect(),
            })
            .collect();
        let here: HashSet<RoomId> = scene.sheet.rooms.iter().map(|r| r.id).collect();
        for area in official {
            let rooms: HashSet<RoomId> = area
                .rooms
                .iter()
                .copied()
                .filter(|id| here.contains(id))
                .collect();
            if rooms.is_empty() {
                continue;
            }
            units.push(FocusUnit {
                kind: FocusKind::Official,
                name: area.name.clone(),
                rooms,
            });
        }
        let mut doors: HashSet<RoomId> = scene
            .units
            .iter()
            .flat_map(|u| u.door_rooms.iter().copied())
            .collect();
        doors.extend(
            scene
                .sheet
                .rooms
                .iter()
                .filter(|r| r.entrance)
                .map(|r| r.id),
        );
        Focus {
            units,
            current: None,
            stack: Vec::new(),
            doors,
        }
    }

    /// The unit in focus, if any.
    #[must_use]
    pub fn current(&self) -> Option<&FocusUnit> {
        self.current.map(|i| &self.units[i])
    }

    /// The rooms in focus; none when nothing is.
    #[must_use]
    pub fn rooms(&self) -> &HashSet<RoomId> {
        static NONE: std::sync::LazyLock<HashSet<RoomId>> = std::sync::LazyLock::new(HashSet::new);
        self.current().map_or(&NONE, |u| &u.rooms)
    }

    #[must_use]
    pub fn doors(&self) -> &HashSet<RoomId> {
        &self.doors
    }

    /// The street rooms, drawn whatever is in focus.
    #[must_use]
    pub fn streets(&self) -> &HashSet<RoomId> {
        &self.units[STREETS].rooms
    }

    #[must_use]
    pub fn can_go_back(&self) -> bool {
        !self.stack.is_empty()
    }

    /// The unit a room belongs to first: its building, else the official
    /// area it is in, else the streets. A building wins over a hunting
    /// area that happens to list its rooms, because the building is what
    /// a person standing in it is in.
    #[must_use]
    pub fn unit_for(&self, id: RoomId) -> usize {
        let of = |kind: FocusKind| {
            self.units
                .iter()
                .position(|u| u.kind == kind && u.rooms.contains(&id))
        };
        of(FocusKind::Building)
            .or_else(|| of(FocusKind::Official))
            .unwrap_or(STREETS)
    }

    /// Put the unit holding `id` in focus, remembering where we were.
    /// Returns whether the focus changed.
    pub fn enter(&mut self, id: RoomId) -> bool {
        if self.rooms().contains(&id) {
            return false;
        }
        let unit = self.unit_for(id);
        if Some(unit) == self.current {
            return false;
        }
        self.stack.push(self.current);
        self.current = Some(unit);
        true
    }

    /// Back to the previous focus. Returns whether there was one.
    pub fn back(&mut self) -> bool {
        match self.stack.pop() {
            Some(unit) => {
                self.current = unit;
                true
            }
            None => false,
        }
    }

    /// Keep this focus across a rebuild of the same area, by name: an
    /// edit re-solves the layout, and the building being looked at should
    /// still be the one being looked at afterwards.
    pub fn carry_over(&mut self, previous: &Focus) {
        let by_name = |wanted: Option<usize>| -> Option<usize> {
            let wanted = &previous.units[wanted?];
            self.units
                .iter()
                .position(|u| u.kind == wanted.kind && u.name == wanted.name)
        };
        self.current = by_name(previous.current);
        self.stack = previous.stack.iter().map(|&i| by_name(i)).collect();
    }
}
