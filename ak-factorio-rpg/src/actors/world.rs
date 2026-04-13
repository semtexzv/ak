use std::future::Future;
use std::collections::HashMap;
use ak::{Actor, Handler, ContextRef, Addr, Message};
use crate::data::world_map::{WorldMap, TileType};
use crate::data::items;
use crate::data::recipes;
use crate::data::inventory::Inventory;
use crate::data::stats::CharacterState;
use crate::data::memo::{MemoEntry, MemoEventType};
use crate::messages::world_msgs::*;
use crate::messages::player_msgs::*;
use crate::messages::memo_msgs::{LogEvent, GetChronicle};
use crate::actors::memo_log::MemoLogActor;
use crate::actors::player::PlayerActor;

/// The WorldActor is the central authority for the game world.
/// It owns the map, manages player connections, and drives the game tick.
pub struct WorldActor {
    pub map: WorldMap,
    pub players: HashMap<String, Addr<PlayerActor>>,
    pub memo_log: Addr<MemoLogActor>,
    pub tick: u64,
    pub item_registry: Vec<items::ItemDef>,
    pub recipe_registry: Vec<recipes::Recipe>,
    pub next_factory_id: u64,
    pub self_addr: Option<Addr<WorldActor>>,
}

impl Actor for WorldActor {
    fn started(self: &mut ContextRef<Self>) {
        println!("[World] Factorio Memo RPG world started!");
        println!("[World] Map: {}x{}", self.map.width, self.map.height);
    }
}

impl Handler<Tick> for WorldActor {
    type Future = impl Future<Output = ()> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, _msg: Tick) -> Self::Future {
        async move {
            self.tick += 1;

            // Regenerate resources every 50 ticks
            if self.tick % 50 == 0 {
                for y in 0..self.map.height as usize {
                    for x in 0..self.map.width as usize {
                        if let Some(ref mut resource) = self.map.tiles[y][x].resource {
                            if resource.quantity < resource.max_quantity {
                                resource.quantity = (resource.quantity + resource.regen_rate)
                                    .min(resource.max_quantity);
                            }
                        }
                    }
                }
            }

            // Schedule next tick
            if let Some(ref addr) = self.self_addr {
                let addr = addr.clone();
                ak::rt::spawn(async move {
                    tokio_timer::delay_for(std::time::Duration::from_millis(100)).await;
                    let _ = addr.send(Tick).await;
                });
            }
        }
    }
}

impl Handler<PlayerConnect> for WorldActor {
    type Future = impl Future<Output = Result<String, String>> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: PlayerConnect) -> Self::Future {
        async move {
            if self.players.contains_key(&msg.name) {
                return Err(format!("Player '{}' is already connected!", msg.name));
            }

            let name = msg.name.clone();
            let world_addr = self.self_addr.clone().unwrap();
            let memo_addr = self.memo_log.clone();
            let item_reg = self.item_registry.clone();
            let recipe_reg = self.recipe_registry.clone();
            let spawn_x = self.map.width / 2;
            let spawn_y = self.map.height / 2;
            let tick = self.tick;

            let player_addr = PlayerActor::start(move |_addr| {
                let mut character = CharacterState::new(name.clone());
                character.position = (spawn_x, spawn_y);
                PlayerActor {
                    character,
                    inventory: Inventory::new(20),
                    world: world_addr,
                    memo_log: memo_addr,
                    tick,
                    item_registry: item_reg,
                    recipe_registry: recipe_reg,
                }
            });

            self.players.insert(msg.name.clone(), player_addr);

            // Log to memo
            let _ = self.memo_log.send(LogEvent(MemoEntry {
                tick: self.tick,
                player: Some(msg.name.clone()),
                event_type: MemoEventType::PlayerJoined,
                description: format!("{} entered the world", msg.name),
            })).await;

            println!("[World] Player '{}' connected (tick {})", msg.name, self.tick);
            Ok(format!("Welcome to Factorio Memo RPG, {}! You spawn at ({}, {}).\nType 'help' for commands.",
                msg.name, spawn_x, spawn_y))
        }
    }
}

impl Handler<PlayerDisconnect> for WorldActor {
    type Future = impl Future<Output = ()> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: PlayerDisconnect) -> Self::Future {
        async move {
            if self.players.remove(&msg.name).is_some() {
                let _ = self.memo_log.send(LogEvent(MemoEntry {
                    tick: self.tick,
                    player: Some(msg.name.clone()),
                    event_type: MemoEventType::PlayerLeft,
                    description: format!("{} left the world", msg.name),
                })).await;
                println!("[World] Player '{}' disconnected", msg.name);
            }
        }
    }
}

impl Handler<QueryTile> for WorldActor {
    type Future = impl Future<Output = Option<TileInfo>> + 'static;

    fn handle(self: ContextRef<Self>, msg: QueryTile) -> Self::Future {
        let result = self.map.get_tile(msg.x, msg.y).map(|tile| {
            TileInfo {
                x: msg.x,
                y: msg.y,
                tile_type: format!("{:?}", tile.tile_type),
                resource: tile.resource.as_ref().map(|r| (r.resource_type.name().to_string(), r.quantity)),
                factory: tile.factory_id.map(|id| format!("Factory#{}", id)),
                passable: tile.tile_type != TileType::Water && tile.tile_type != TileType::Rock,
            }
        });
        async move { result }
    }
}

impl Handler<MineResource> for WorldActor {
    type Future = impl Future<Output = Option<MineResult>> + 'static;

    fn handle(mut self: ContextRef<Self>, msg: MineResource) -> Self::Future {
        let result = if let Some(tile) = self.map.get_tile_mut(msg.x, msg.y) {
            if let Some(ref mut resource) = tile.resource {
                if resource.quantity > 0 {
                    let mined = msg.amount.min(resource.quantity);
                    resource.quantity -= mined;
                    Some(MineResult {
                        item_id: resource.resource_type.item_id(),
                        quantity: mined,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        async move { result }
    }
}

impl Handler<CanBuild> for WorldActor {
    type Future = impl Future<Output = bool> + 'static;

    fn handle(self: ContextRef<Self>, msg: CanBuild) -> Self::Future {
        let result = self.map.get_tile(msg.x, msg.y)
            .map(|t| {
                t.tile_type != TileType::Water
                    && t.tile_type != TileType::Rock
                    && t.factory_id.is_none()
            })
            .unwrap_or(false);
        async move { result }
    }
}

impl Handler<PlaceFactory> for WorldActor {
    type Future = impl Future<Output = bool> + 'static;

    fn handle(mut self: ContextRef<Self>, msg: PlaceFactory) -> Self::Future {
        let success = if let Some(tile) = self.map.get_tile_mut(msg.x, msg.y) {
            if tile.tile_type != TileType::Water && tile.tile_type != TileType::Rock && tile.factory_id.is_none() {
                tile.factory_id = Some(msg.factory_id);
                tile.owner = Some(msg.player.clone());
                true
            } else {
                false
            }
        } else {
            false
        };
        async move { success }
    }
}

impl Handler<GetMapView> for WorldActor {
    type Future = impl Future<Output = Vec<TileInfo>> + 'static;

    fn handle(self: ContextRef<Self>, msg: GetMapView) -> Self::Future {
        let mut tiles = Vec::new();
        for dy in -msg.radius..=msg.radius {
            for dx in -msg.radius..=msg.radius {
                let x = msg.center_x + dx;
                let y = msg.center_y + dy;
                if let Some(tile) = self.map.get_tile(x, y) {
                    tiles.push(TileInfo {
                        x, y,
                        tile_type: format!("{:?}", tile.tile_type),
                        resource: tile.resource.as_ref().map(|r| (r.resource_type.name().to_string(), r.quantity)),
                        factory: tile.factory_id.map(|id| format!("Factory#{}", id)),
                        passable: tile.tile_type != TileType::Water && tile.tile_type != TileType::Rock,
                    });
                }
            }
        }
        async move { tiles }
    }
}

/// The main command dispatcher - routes text commands to the appropriate player actor
impl Handler<PlayerCommand> for WorldActor {
    type Future = impl Future<Output = String> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: PlayerCommand) -> Self::Future {
        async move {
            let parts: Vec<&str> = msg.command.trim().splitn(2, char::is_whitespace).collect();
            let cmd = parts.get(0).map(|s| s.to_lowercase()).unwrap_or_default();
            let args = parts.get(1).map(|s| s.trim()).unwrap_or("");

            // Handle CONNECT specially - it doesn't need an existing player
            if cmd == "connect" {
                if args.is_empty() {
                    return "ERR: Usage: connect <player_name>".into();
                }
                let name = args.to_string();
                match self.self_addr.clone() {
                    Some(addr) => {
                        match addr.send(PlayerConnect { name: name.clone() }).await {
                            Ok(Ok(welcome)) => welcome,
                            Ok(Err(e)) => format!("ERR: {}", e),
                            Err(_) => "ERR: World unreachable".into(),
                        }
                    }
                    None => "ERR: World not initialized".into(),
                }
            } else if let Some(player_addr) = self.players.get(&msg.player_name).cloned() {
                // Dispatch to player actor
                match cmd.as_str() {
                    "status" | "stats" | "char" => {
                        player_addr.send(GetStatus).await.unwrap_or_else(|_| "ERR: Player unreachable".into())
                    }
                    "inventory" | "inv" | "i" => {
                        player_addr.send(GetInventory).await.unwrap_or_else(|_| "ERR: Player unreachable".into())
                    }
                    "move" | "go" | "m" => {
                        match Direction::parse(args) {
                            Some(dir) => {
                                match player_addr.send(MovePlayer { direction: dir }).await {
                                    Ok(Ok(msg)) => msg,
                                    Ok(Err(e)) => format!("ERR: {}", e),
                                    Err(_) => "ERR: Player unreachable".into(),
                                }
                            }
                            None => "ERR: Usage: move <N/S/E/W>".into(),
                        }
                    }
                    "gather" | "mine" | "g" => {
                        match player_addr.send(GatherResource).await {
                            Ok(Ok(msg)) => msg,
                            Ok(Err(e)) => format!("ERR: {}", e),
                            Err(_) => "ERR: Player unreachable".into(),
                        }
                    }
                    "craft" | "c" => {
                        match args.parse::<u32>() {
                            Ok(id) => {
                                match player_addr.send(CraftItem { recipe_id: id }).await {
                                    Ok(Ok(msg)) => msg,
                                    Ok(Err(e)) => format!("ERR: {}", e),
                                    Err(_) => "ERR: Player unreachable".into(),
                                }
                            }
                            Err(_) => "ERR: Usage: craft <recipe_id>".into(),
                        }
                    }
                    "recipes" | "r" => {
                        player_addr.send(ListRecipes).await.unwrap_or_else(|_| "ERR: Player unreachable".into())
                    }
                    "look" | "l" | "map" => {
                        let radius = args.parse::<i32>().unwrap_or(3);
                        player_addr.send(LookAround { radius }).await.unwrap_or_else(|_| "ERR: Player unreachable".into())
                    }
                    "stat" | "allocate" => {
                        if args.is_empty() {
                            "ERR: Usage: stat <str/dex/int/con/wis/cha>".into()
                        } else {
                            match player_addr.send(AllocateStat { stat_name: args.to_string() }).await {
                                Ok(Ok(msg)) => msg,
                                Ok(Err(e)) => format!("ERR: {}", e),
                                Err(_) => "ERR: Player unreachable".into(),
                            }
                        }
                    }
                    "memo" | "journal" | "j" => {
                        let count = args.parse::<usize>().unwrap_or(20);
                        player_addr.send(ViewMemo { last_n: count }).await.unwrap_or_else(|_| "ERR: Player unreachable".into())
                    }
                    "chronicle" | "news" => {
                        let count = args.parse::<usize>().unwrap_or(20);
                        match self.memo_log.send(GetChronicle { last_n: count }).await {
                            Ok(lines) => {
                                let mut result = format!("=== World Chronicle (last {}) ===\n", count);
                                if lines.is_empty() {
                                    result.push_str("  (no major events yet)");
                                } else {
                                    for line in lines {
                                        result.push_str(&line);
                                        result.push('\n');
                                    }
                                }
                                result
                            }
                            Err(_) => "ERR: Memo log unreachable".into(),
                        }
                    }
                    "help" | "h" | "?" => {
                        player_addr.send(GetHelp).await.unwrap_or_else(|_| "ERR: Player unreachable".into())
                    }
                    "quit" | "exit" | "q" => {
                        "QUIT".into()
                    }
                    "" => "Type 'help' for available commands.".into(),
                    _ => format!("Unknown command: '{}'. Type 'help' for available commands.", cmd),
                }
            } else {
                "ERR: Not connected. Use: connect <player_name>".into()
            }
        }
    }
}
