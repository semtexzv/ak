#![feature(type_alias_impl_trait, arbitrary_self_types, impl_trait_in_assoc_type)]
#![feature(stmt_expr_attributes, proc_macro_hygiene)]
#![allow(unused_imports, unused_mut)]

use std::collections::HashMap;

// Re-declare modules for the binary target
mod data;
mod messages;
mod actors;
mod net;

use ak::*;
use data::world_map::WorldMap;
use data::items;
use data::recipes;
use data::memo::{MemoJournal, MemoEntry, MemoEventType};
use actors::memo_log::MemoLogActor;
use actors::world::WorldActor;

const MAP_WIDTH: i32 = 64;
const MAP_HEIGHT: i32 = 64;
const WORLD_SEED: u64 = 42;
const BIND_ADDR: &str = "0.0.0.0:7878";

fn main() {
    println!("Initializing Factorio Memo RPG Server...");

    ak::rt::System::run(|| {
        ak::rt::spawn(async {
            // 1. Build registries
            let item_registry = items::build_item_registry();
            let recipe_registry = recipes::build_recipe_registry();

            println!("Loaded {} items, {} recipes", item_registry.len(), recipe_registry.len());

            // 2. Generate the world map
            let map = WorldMap::generate(MAP_WIDTH, MAP_HEIGHT, WORLD_SEED);

            // Count resources
            let mut resource_count = 0;
            for row in &map.tiles {
                for tile in row {
                    if tile.resource.is_some() {
                        resource_count += 1;
                    }
                }
            }
            println!("Generated {}x{} map with {} resource tiles", MAP_WIDTH, MAP_HEIGHT, resource_count);

            // 3. Start the MemoLog actor
            let memo_log = MemoLogActor::start(|_| MemoLogActor {
                journal: MemoJournal::new(10000),
            });

            // Log world creation
            let _ = memo_log.send(messages::memo_msgs::LogEvent(MemoEntry {
                tick: 0,
                player: None,
                event_type: MemoEventType::WorldEvent,
                description: "The world was created. A new age begins.".into(),
            })).await;

            // 4. Start the World actor
            let item_reg2 = item_registry.clone();
            let recipe_reg2 = recipe_registry.clone();
            let memo_log2 = memo_log.clone();

            let world = WorldActor::start(move |addr| {
                WorldActor {
                    map,
                    players: HashMap::new(),
                    memo_log: memo_log2,
                    tick: 0,
                    item_registry: item_reg2,
                    recipe_registry: recipe_reg2,
                    next_factory_id: 1,
                    self_addr: Some(addr),
                }
            });

            // 5. Kick off the game tick
            let _ = world.send(messages::world_msgs::Tick).await;

            // 6. Start the TCP server (blocks forever, accepting connections)
            net::server::run_server(world, BIND_ADDR).await;
        });
    });
}
