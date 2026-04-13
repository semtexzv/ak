use std::future::Future;
use ak::{Actor, Handler, ContextRef, Addr, Message};
use crate::data::stats::CharacterState;
use crate::data::inventory::Inventory;
use crate::data::items;
use crate::data::recipes::{self, MachineType};
use crate::data::memo::{MemoEntry, MemoEventType};
use crate::messages::player_msgs::*;
use crate::messages::world_msgs::*;
use crate::messages::memo_msgs::LogEvent;
use crate::actors::memo_log::MemoLogActor;
use crate::actors::world::WorldActor;

/// A PlayerActor represents a single connected player's RPG character.
/// Each player has their own actor that handles all their commands.
pub struct PlayerActor {
    pub character: CharacterState,
    pub inventory: Inventory,
    pub world: Addr<WorldActor>,
    pub memo_log: Addr<MemoLogActor>,
    pub tick: u64,
    pub item_registry: Vec<items::ItemDef>,
    pub recipe_registry: Vec<recipes::Recipe>,
}

impl Actor for PlayerActor {}

impl Handler<GetStatus> for PlayerActor {
    type Future = impl Future<Output = String> + 'static;

    fn handle(self: ContextRef<Self>, _msg: GetStatus) -> Self::Future {
        let c = &self.character;
        let d = &c.derived;
        let status = format!(
            "=== {} (Level {}) ===\n\
             HP: {}/{} | XP: {}/{} | Stat Points: {}\n\
             Position: ({}, {})\n\
             --- Stats ---\n\
             STR: {:>3}  DEX: {:>3}  INT: {:>3}\n\
             CON: {:>3}  WIS: {:>3}  CHA: {:>3}\n\
             --- Derived ---\n\
             Melee Dmg: {} | Carry: {} | Gather Speed: {:.1}x\n\
             Craft Speed: {:.1}x | XP Mult: {:.1}x | Move Speed: {:.1}x\n\
             --- Skills ---\n{}",
            c.name, c.level,
            c.hp, d.max_hp, c.xp, c.xp_to_next(), c.stat_points,
            c.position.0, c.position.1,
            c.stats.strength, c.stats.dexterity, c.stats.intelligence,
            c.stats.constitution, c.stats.wisdom, c.stats.charisma,
            d.melee_damage, d.carry_capacity, d.gather_speed_mult,
            d.craft_speed_mult, d.xp_mult, d.move_speed_mult,
            c.skills.iter()
                .map(|s| format!("  {} Lv.{} ({}/{})", s.name, s.level, s.xp, s.xp_to_next()))
                .collect::<Vec<_>>()
                .join("\n")
        );
        async move { status }
    }
}

impl Handler<GetInventory> for PlayerActor {
    type Future = impl Future<Output = String> + 'static;

    fn handle(self: ContextRef<Self>, _msg: GetInventory) -> Self::Future {
        let items_list = self.inventory.items_list();
        let reg = &self.item_registry;
        let mut result = format!("=== Inventory ({}/{} slots) ===\n",
            self.inventory.used_slots(), self.inventory.max_slots);
        if items_list.is_empty() {
            result.push_str("  (empty)");
        } else {
            for (id, qty) in items_list {
                let name = items::item_name(reg, id);
                result.push_str(&format!("  {} x{}\n", name, qty));
            }
        }
        async move { result }
    }
}

impl Handler<MovePlayer> for PlayerActor {
    type Future = impl Future<Output = Result<String, String>> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: MovePlayer) -> Self::Future {
        async move {
            let (dx, dy) = msg.direction.delta();
            let new_x = self.character.position.0 + dx;
            let new_y = self.character.position.1 + dy;

            // Ask world if tile is passable
            let tile_info = self.world.send(QueryTile { x: new_x, y: new_y }).await;
            match tile_info {
                Ok(Some(info)) if info.passable => {
                    self.character.position = (new_x, new_y);
                    let mut desc = format!("Moved to ({}, {}) - {}", new_x, new_y, info.tile_type);
                    if let Some((res_name, qty)) = &info.resource {
                        desc.push_str(&format!(" [Resource: {} x{}]", res_name, qty));
                    }
                    Ok(desc)
                }
                Ok(Some(_)) => Err("That tile is impassable!".into()),
                Ok(None) => Err("You can't go there - edge of the world!".into()),
                Err(_) => Err("World unreachable".into()),
            }
        }
    }
}

impl Handler<GatherResource> for PlayerActor {
    type Future = impl Future<Output = Result<String, String>> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, _msg: GatherResource) -> Self::Future {
        async move {
            let (x, y) = self.character.position;
            let gather_amount = (5.0 * self.character.derived.gather_speed_mult) as u32;

            let result = self.world.send(MineResource {
                x, y, amount: gather_amount,
            }).await;

            match result {
                Ok(Some(mine_result)) => {
                    // Clone name to avoid borrow conflicts
                    let item_name = items::item_name(&self.item_registry, mine_result.item_id).to_string();
                    let stack_size = self.item_registry.iter()
                        .find(|i| i.id == mine_result.item_id)
                        .map(|i| i.stack_size)
                        .unwrap_or(50);
                    let overflow = self.inventory.add_item(
                        mine_result.item_id,
                        mine_result.quantity,
                        stack_size,
                    );
                    let gathered = mine_result.quantity - overflow;

                    // Grant XP
                    let base_xp = gathered as u64 * 3;
                    let leveled = self.character.add_xp(base_xp);
                    self.character.add_skill_xp("Mining", gathered as u64 * 5);

                    // Log to memo
                    let player_name = self.character.name.clone();
                    let tick = self.tick;
                    let _ = self.memo_log.send(LogEvent(MemoEntry {
                        tick,
                        player: Some(player_name.clone()),
                        event_type: MemoEventType::ResourceGathered,
                        description: format!("Gathered {} x{}", item_name, gathered),
                    })).await;

                    if leveled {
                        let level = self.character.level;
                        let _ = self.memo_log.send(LogEvent(MemoEntry {
                            tick,
                            player: Some(player_name),
                            event_type: MemoEventType::LevelUp,
                            description: format!("Reached level {}!", level),
                        })).await;
                    }

                    let mut msg = format!("Gathered {} x{} (+{} XP)", item_name, gathered, base_xp);
                    if overflow > 0 {
                        msg.push_str(&format!(" ({} lost - inventory full!)", overflow));
                    }
                    if leveled {
                        msg.push_str(&format!("\n*** LEVEL UP! You are now level {}! +3 stat points ***", self.character.level));
                    }
                    Ok(msg)
                }
                Ok(None) => Err("No resource to gather here.".into()),
                Err(_) => Err("World unreachable".into()),
            }
        }
    }
}

impl Handler<CraftItem> for PlayerActor {
    type Future = impl Future<Output = Result<String, String>> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: CraftItem) -> Self::Future {
        async move {
            let recipe = self.recipe_registry.iter().find(|r| r.id == msg.recipe_id).cloned();
            let recipe = match recipe {
                Some(r) => r,
                None => return Err(format!("Unknown recipe ID: {}", msg.recipe_id)),
            };

            // Check machine type - only Hand recipes can be hand-crafted
            if recipe.machine_type != MachineType::Hand {
                return Err(format!("'{}' requires a {:?} - cannot hand-craft!", recipe.name, recipe.machine_type));
            }

            // Check skill requirement
            if let Some((ref skill_name, min_level)) = recipe.required_skill {
                let skill_level = self.character.get_skill(skill_name)
                    .map(|s| s.level)
                    .unwrap_or(0);
                if skill_level < min_level {
                    return Err(format!("Requires {} level {} (you have level {})",
                        skill_name, min_level, skill_level));
                }
            }

            // Check ingredients
            for input in &recipe.inputs {
                let have = self.inventory.count_item(input.item_id);
                if have < input.quantity {
                    let name = items::item_name(&self.item_registry, input.item_id).to_string();
                    return Err(format!("Missing {} (need {}, have {})", name, input.quantity, have));
                }
            }

            // Consume inputs
            for input in &recipe.inputs {
                self.inventory.remove_item(input.item_id, input.quantity);
            }

            // Produce outputs
            let mut produced = Vec::new();
            for output in &recipe.outputs {
                let stack_size = self.item_registry.iter()
                    .find(|i| i.id == output.item_id)
                    .map(|i| i.stack_size)
                    .unwrap_or(50);
                let name = items::item_name(&self.item_registry, output.item_id).to_string();
                self.inventory.add_item(output.item_id, output.quantity, stack_size);
                produced.push(format!("{} x{}", name, output.quantity));
            }

            // Grant XP
            let leveled = self.character.add_xp(recipe.xp_reward);
            if let Some((ref skill_name, skill_xp)) = recipe.skill_xp_reward {
                self.character.add_skill_xp(skill_name, skill_xp);
            }

            // Log to memo
            let player_name = self.character.name.clone();
            let tick = self.tick;
            let recipe_name = recipe.name.clone();
            let _ = self.memo_log.send(LogEvent(MemoEntry {
                tick,
                player: Some(player_name.clone()),
                event_type: MemoEventType::ItemCrafted,
                description: format!("Crafted {}", recipe_name),
            })).await;

            if leveled {
                let level = self.character.level;
                let _ = self.memo_log.send(LogEvent(MemoEntry {
                    tick,
                    player: Some(player_name),
                    event_type: MemoEventType::LevelUp,
                    description: format!("Reached level {}!", level),
                })).await;
            }

            let mut msg = format!("Crafted: {} (+{} XP)", produced.join(", "), recipe.xp_reward);
            if leveled {
                msg.push_str(&format!("\n*** LEVEL UP! You are now level {}! +3 stat points ***", self.character.level));
            }
            Ok(msg)
        }
    }
}

impl Handler<AllocateStat> for PlayerActor {
    type Future = impl Future<Output = Result<String, String>> + 'static;

    fn handle(mut self: ContextRef<Self>, msg: AllocateStat) -> Self::Future {
        let result = if self.character.allocate_stat(&msg.stat_name) {
            Ok(format!("Allocated 1 point to {}. {} stat points remaining.",
                msg.stat_name, self.character.stat_points))
        } else if self.character.stat_points == 0 {
            Err("No stat points available! Level up to earn more.".into())
        } else {
            Err(format!("Unknown stat '{}'. Use: str, dex, int, con, wis, cha", msg.stat_name))
        };
        async move { result }
    }
}

impl Handler<LookAround> for PlayerActor {
    type Future = impl Future<Output = String> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: LookAround) -> Self::Future {
        async move {
            let (cx, cy) = self.character.position;
            let radius = msg.radius.min(5);

            let map_view = self.world.send(GetMapView {
                center_x: cx,
                center_y: cy,
                radius,
            }).await;

            match map_view {
                Ok(tiles) => {
                    let mut result = format!("=== Map around ({}, {}) ===\n", cx, cy);
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            let tx = cx + dx;
                            let ty = cy + dy;
                            if tx == cx && ty == cy {
                                result.push('@'); // player position
                            } else if let Some(tile) = tiles.iter().find(|t| t.x == tx && t.y == ty) {
                                if tile.resource.is_some() {
                                    result.push('*'); // resource
                                } else if tile.factory.is_some() {
                                    result.push('F'); // factory
                                } else {
                                    match tile.tile_type.as_str() {
                                        "Water" => result.push('~'),
                                        "Rock" => result.push('#'),
                                        "Forest" => result.push('T'),
                                        "Sand" => result.push('.'),
                                        _ => result.push(' '),
                                    }
                                }
                            } else {
                                result.push('?');
                            }
                        }
                        result.push('\n');
                    }
                    result.push_str("Legend: @ = you, * = resource, F = factory, ~ = water, # = rock, T = forest\n");
                    result
                }
                Err(_) => "Cannot see the map right now.".into(),
            }
        }
    }
}

impl Handler<ViewMemo> for PlayerActor {
    type Future = impl Future<Output = String> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: ViewMemo) -> Self::Future {
        async move {
            use crate::messages::memo_msgs::QueryMemo;

            let entries = self.memo_log.send(QueryMemo {
                player: Some(self.character.name.clone()),
                event_type: None,
                last_n: msg.last_n,
            }).await;

            match entries {
                Ok(lines) => {
                    let mut result = format!("=== {}'s Memo Journal (last {}) ===\n",
                        self.character.name, msg.last_n);
                    if lines.is_empty() {
                        result.push_str("  (no entries yet - go explore!)");
                    } else {
                        for line in lines {
                            result.push_str(&line);
                            result.push('\n');
                        }
                    }
                    result
                }
                Err(_) => "Cannot read memo journal right now.".into(),
            }
        }
    }
}

impl Handler<ListRecipes> for PlayerActor {
    type Future = impl Future<Output = String> + 'static;

    fn handle(self: ContextRef<Self>, _msg: ListRecipes) -> Self::Future {
        let reg = &self.recipe_registry;
        let item_reg = &self.item_registry;
        let mut result = "=== Available Recipes ===\n".to_string();
        for recipe in reg {
            let inputs: Vec<String> = recipe.inputs.iter()
                .map(|i| format!("{} x{}", items::item_name(item_reg, i.item_id), i.quantity))
                .collect();
            let outputs: Vec<String> = recipe.outputs.iter()
                .map(|o| format!("{} x{}", items::item_name(item_reg, o.item_id), o.quantity))
                .collect();
            let machine = match recipe.machine_type {
                MachineType::Hand => "Hand",
                MachineType::Furnace => "Furnace",
                MachineType::Assembler => "Assembler",
            };
            let skill_req = recipe.required_skill.as_ref()
                .map(|(s, l)| format!(" [Req: {} Lv{}]", s, l))
                .unwrap_or_default();

            result.push_str(&format!("  #{}: {} ({}) -> {} [{}]{} (+{} XP)\n",
                recipe.id, inputs.join(" + "), machine, outputs.join(", "),
                recipe.name, skill_req, recipe.xp_reward));
        }
        async move { result }
    }
}

impl Handler<GetHelp> for PlayerActor {
    type Future = impl Future<Output = String> + 'static;

    fn handle(self: ContextRef<Self>, _msg: GetHelp) -> Self::Future {
        let help = "\
=== Factorio Memo RPG - Commands ===
  status          - View your character sheet
  inventory / inv - View your inventory
  move <N/S/E/W>  - Move in a direction
  gather          - Gather resource from current tile
  craft <id>      - Hand-craft a recipe by ID
  recipes         - List all recipes
  look [radius]   - View the map around you (default radius 3)
  stat <name>     - Allocate a stat point (str/dex/int/con/wis/cha)
  memo [count]    - View your personal memo journal
  chronicle [n]   - View the world chronicle
  help            - Show this help
  quit            - Disconnect
===
Tip: Gather resources, craft items, level up, and build your factory empire!
Your journey is recorded in your Memo Journal - review it anytime.
".to_string();
        async move { help }
    }
}
