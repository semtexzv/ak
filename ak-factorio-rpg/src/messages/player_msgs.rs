use ak::Message;
use crate::data::items::ItemId;

/// Gather resource from current tile
pub struct GatherResource;

impl Message for GatherResource {
    type Result = Result<String, String>;
}

/// Hand-craft an item by recipe ID
pub struct CraftItem {
    pub recipe_id: u32,
}

impl Message for CraftItem {
    type Result = Result<String, String>;
}

/// Move the player in a direction
pub struct MovePlayer {
    pub direction: Direction,
}

#[derive(Clone, Debug)]
pub enum Direction {
    North,
    South,
    East,
    West,
}

impl Direction {
    pub fn delta(&self) -> (i32, i32) {
        match self {
            Direction::North => (0, -1),
            Direction::South => (0, 1),
            Direction::East => (1, 0),
            Direction::West => (-1, 0),
        }
    }

    pub fn parse(s: &str) -> Option<Direction> {
        match s.to_lowercase().as_str() {
            "n" | "north" | "up" => Some(Direction::North),
            "s" | "south" | "down" => Some(Direction::South),
            "e" | "east" | "right" => Some(Direction::East),
            "w" | "west" | "left" => Some(Direction::West),
            _ => None,
        }
    }
}

impl Message for MovePlayer {
    type Result = Result<String, String>;
}

/// Get full player status (character sheet)
pub struct GetStatus;

impl Message for GetStatus {
    type Result = String;
}

/// Get inventory contents
pub struct GetInventory;

impl Message for GetInventory {
    type Result = String;
}

/// Allocate a stat point
pub struct AllocateStat {
    pub stat_name: String,
}

impl Message for AllocateStat {
    type Result = Result<String, String>;
}

/// Look around - see nearby tiles
pub struct LookAround {
    pub radius: i32,
}

impl Message for LookAround {
    type Result = String;
}

/// View personal memo journal
pub struct ViewMemo {
    pub last_n: usize,
}

impl Message for ViewMemo {
    type Result = String;
}

/// Build a factory at a position
pub struct BuildFactory {
    pub factory_type: String,
    pub x: i32,
    pub y: i32,
}

impl Message for BuildFactory {
    type Result = Result<String, String>;
}

/// List available recipes
pub struct ListRecipes;

impl Message for ListRecipes {
    type Result = String;
}

/// Get help text
pub struct GetHelp;

impl Message for GetHelp {
    type Result = String;
}
