use ak::Message;
use crate::data::world_map::Tile;

/// Game tick - advances world simulation
pub struct Tick;

impl Message for Tick {
    type Result = ();
}

/// Player connects to the world
pub struct PlayerConnect {
    pub name: String,
}

impl Message for PlayerConnect {
    type Result = Result<String, String>;
}

/// Player disconnects
pub struct PlayerDisconnect {
    pub name: String,
}

impl Message for PlayerDisconnect {
    type Result = ();
}

/// Query a tile at a position (used by player actors)
pub struct QueryTile {
    pub x: i32,
    pub y: i32,
}

/// Response for tile query
#[derive(Clone, Debug)]
pub struct TileInfo {
    pub x: i32,
    pub y: i32,
    pub tile_type: String,
    pub resource: Option<(String, u32)>,
    pub factory: Option<String>,
    pub passable: bool,
}

impl Message for QueryTile {
    type Result = Option<TileInfo>;
}

/// Mine resource from a specific tile
pub struct MineResource {
    pub x: i32,
    pub y: i32,
    pub amount: u32,
}

#[derive(Clone, Debug)]
pub struct MineResult {
    pub item_id: u32,
    pub quantity: u32,
}

impl Message for MineResource {
    type Result = Option<MineResult>;
}

/// Check if a position is valid for building
pub struct CanBuild {
    pub x: i32,
    pub y: i32,
    pub player: String,
}

impl Message for CanBuild {
    type Result = bool;
}

/// Place a factory on the world map
pub struct PlaceFactory {
    pub x: i32,
    pub y: i32,
    pub player: String,
    pub factory_type: String,
    pub factory_id: u64,
}

impl Message for PlaceFactory {
    type Result = bool;
}

/// Get map view around a position
pub struct GetMapView {
    pub center_x: i32,
    pub center_y: i32,
    pub radius: i32,
}

impl Message for GetMapView {
    type Result = Vec<TileInfo>;
}

/// Dispatch a text command from a connected player
pub struct PlayerCommand {
    pub player_name: String,
    pub command: String,
}

impl Message for PlayerCommand {
    type Result = String;
}
