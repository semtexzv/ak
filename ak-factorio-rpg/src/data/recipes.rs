use super::items::*;

#[derive(Clone, Debug)]
pub struct RecipeIngredient {
    pub item_id: ItemId,
    pub quantity: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MachineType {
    Hand,
    Furnace,
    Assembler,
}

#[derive(Clone, Debug)]
pub struct Recipe {
    pub id: u32,
    pub name: String,
    pub inputs: Vec<RecipeIngredient>,
    pub outputs: Vec<RecipeIngredient>,
    pub craft_ticks: u32,
    pub machine_type: MachineType,
    pub required_skill: Option<(String, u32)>,
    pub xp_reward: u64,
    pub skill_xp_reward: Option<(String, u64)>,
}

pub fn build_recipe_registry() -> Vec<Recipe> {
    vec![
        // Smelting (Furnace)
        Recipe {
            id: 1, name: "Smelt Iron Plate".into(),
            inputs: vec![RecipeIngredient { item_id: IRON_ORE, quantity: 1 }],
            outputs: vec![RecipeIngredient { item_id: IRON_PLATE, quantity: 1 }],
            craft_ticks: 32, machine_type: MachineType::Furnace,
            required_skill: None, xp_reward: 5,
            skill_xp_reward: Some(("Smelting".into(), 10)),
        },
        Recipe {
            id: 2, name: "Smelt Copper Plate".into(),
            inputs: vec![RecipeIngredient { item_id: COPPER_ORE, quantity: 1 }],
            outputs: vec![RecipeIngredient { item_id: COPPER_PLATE, quantity: 1 }],
            craft_ticks: 32, machine_type: MachineType::Furnace,
            required_skill: None, xp_reward: 5,
            skill_xp_reward: Some(("Smelting".into(), 10)),
        },
        Recipe {
            id: 3, name: "Smelt Steel Plate".into(),
            inputs: vec![RecipeIngredient { item_id: IRON_PLATE, quantity: 5 }],
            outputs: vec![RecipeIngredient { item_id: STEEL_PLATE, quantity: 1 }],
            craft_ticks: 160, machine_type: MachineType::Furnace,
            required_skill: Some(("Smelting".into(), 3)), xp_reward: 25,
            skill_xp_reward: Some(("Smelting".into(), 40)),
        },
        Recipe {
            id: 4, name: "Smelt Stone Brick".into(),
            inputs: vec![RecipeIngredient { item_id: STONE, quantity: 2 }],
            outputs: vec![RecipeIngredient { item_id: STONE_BRICK, quantity: 1 }],
            craft_ticks: 32, machine_type: MachineType::Furnace,
            required_skill: None, xp_reward: 5,
            skill_xp_reward: Some(("Smelting".into(), 8)),
        },
        // Components (Assembler or Hand)
        Recipe {
            id: 5, name: "Craft Iron Gear".into(),
            inputs: vec![RecipeIngredient { item_id: IRON_PLATE, quantity: 2 }],
            outputs: vec![RecipeIngredient { item_id: IRON_GEAR, quantity: 1 }],
            craft_ticks: 10, machine_type: MachineType::Hand,
            required_skill: None, xp_reward: 8,
            skill_xp_reward: Some(("Crafting".into(), 12)),
        },
        Recipe {
            id: 6, name: "Craft Copper Wire".into(),
            inputs: vec![RecipeIngredient { item_id: COPPER_PLATE, quantity: 1 }],
            outputs: vec![RecipeIngredient { item_id: COPPER_WIRE, quantity: 2 }],
            craft_ticks: 5, machine_type: MachineType::Hand,
            required_skill: None, xp_reward: 5,
            skill_xp_reward: Some(("Crafting".into(), 8)),
        },
        Recipe {
            id: 7, name: "Craft Electronic Circuit".into(),
            inputs: vec![
                RecipeIngredient { item_id: IRON_PLATE, quantity: 1 },
                RecipeIngredient { item_id: COPPER_WIRE, quantity: 3 },
            ],
            outputs: vec![RecipeIngredient { item_id: GREEN_CIRCUIT, quantity: 1 }],
            craft_ticks: 10, machine_type: MachineType::Assembler,
            required_skill: Some(("Crafting".into(), 2)), xp_reward: 15,
            skill_xp_reward: Some(("Crafting".into(), 20)),
        },
        Recipe {
            id: 8, name: "Craft Pipe".into(),
            inputs: vec![RecipeIngredient { item_id: IRON_PLATE, quantity: 1 }],
            outputs: vec![RecipeIngredient { item_id: PIPE, quantity: 1 }],
            craft_ticks: 5, machine_type: MachineType::Hand,
            required_skill: None, xp_reward: 3,
            skill_xp_reward: Some(("Crafting".into(), 5)),
        },
        // Machines
        Recipe {
            id: 9, name: "Craft Stone Furnace".into(),
            inputs: vec![RecipeIngredient { item_id: STONE, quantity: 5 }],
            outputs: vec![RecipeIngredient { item_id: FURNACE, quantity: 1 }],
            craft_ticks: 10, machine_type: MachineType::Hand,
            required_skill: None, xp_reward: 10,
            skill_xp_reward: Some(("Crafting".into(), 15)),
        },
        Recipe {
            id: 10, name: "Craft Mining Drill".into(),
            inputs: vec![
                RecipeIngredient { item_id: IRON_PLATE, quantity: 3 },
                RecipeIngredient { item_id: IRON_GEAR, quantity: 3 },
            ],
            outputs: vec![RecipeIngredient { item_id: MINING_DRILL, quantity: 1 }],
            craft_ticks: 20, machine_type: MachineType::Hand,
            required_skill: Some(("Crafting".into(), 2)), xp_reward: 20,
            skill_xp_reward: Some(("Crafting".into(), 30)),
        },
        Recipe {
            id: 11, name: "Craft Assembling Machine".into(),
            inputs: vec![
                RecipeIngredient { item_id: IRON_PLATE, quantity: 3 },
                RecipeIngredient { item_id: IRON_GEAR, quantity: 5 },
                RecipeIngredient { item_id: GREEN_CIRCUIT, quantity: 3 },
            ],
            outputs: vec![RecipeIngredient { item_id: ASSEMBLER, quantity: 1 }],
            craft_ticks: 30, machine_type: MachineType::Assembler,
            required_skill: Some(("Crafting".into(), 4)), xp_reward: 40,
            skill_xp_reward: Some(("Crafting".into(), 50)),
        },
        Recipe {
            id: 12, name: "Craft Transport Belt".into(),
            inputs: vec![
                RecipeIngredient { item_id: IRON_PLATE, quantity: 1 },
                RecipeIngredient { item_id: IRON_GEAR, quantity: 1 },
            ],
            outputs: vec![RecipeIngredient { item_id: TRANSPORT_BELT, quantity: 2 }],
            craft_ticks: 5, machine_type: MachineType::Hand,
            required_skill: None, xp_reward: 5,
            skill_xp_reward: Some(("Logistics".into(), 10)),
        },
        Recipe {
            id: 13, name: "Craft Inserter".into(),
            inputs: vec![
                RecipeIngredient { item_id: IRON_PLATE, quantity: 1 },
                RecipeIngredient { item_id: IRON_GEAR, quantity: 1 },
                RecipeIngredient { item_id: GREEN_CIRCUIT, quantity: 1 },
            ],
            outputs: vec![RecipeIngredient { item_id: INSERTER, quantity: 1 }],
            craft_ticks: 10, machine_type: MachineType::Assembler,
            required_skill: Some(("Logistics".into(), 2)), xp_reward: 12,
            skill_xp_reward: Some(("Logistics".into(), 15)),
        },
        // Tools & Equipment
        Recipe {
            id: 14, name: "Craft Iron Pickaxe".into(),
            inputs: vec![
                RecipeIngredient { item_id: IRON_PLATE, quantity: 3 },
                RecipeIngredient { item_id: WOOD, quantity: 2 },
            ],
            outputs: vec![RecipeIngredient { item_id: IRON_PICKAXE, quantity: 1 }],
            craft_ticks: 15, machine_type: MachineType::Hand,
            required_skill: None, xp_reward: 10,
            skill_xp_reward: Some(("Crafting".into(), 15)),
        },
        Recipe {
            id: 15, name: "Craft Iron Armor".into(),
            inputs: vec![RecipeIngredient { item_id: IRON_PLATE, quantity: 8 }],
            outputs: vec![RecipeIngredient { item_id: IRON_ARMOR, quantity: 1 }],
            craft_ticks: 20, machine_type: MachineType::Hand,
            required_skill: Some(("Crafting".into(), 3)), xp_reward: 20,
            skill_xp_reward: Some(("Crafting".into(), 25)),
        },
        // Advanced Circuit
        Recipe {
            id: 16, name: "Craft Advanced Circuit".into(),
            inputs: vec![
                RecipeIngredient { item_id: GREEN_CIRCUIT, quantity: 2 },
                RecipeIngredient { item_id: COPPER_WIRE, quantity: 4 },
            ],
            outputs: vec![RecipeIngredient { item_id: RED_CIRCUIT, quantity: 1 }],
            craft_ticks: 60, machine_type: MachineType::Assembler,
            required_skill: Some(("Crafting".into(), 5)), xp_reward: 50,
            skill_xp_reward: Some(("Crafting".into(), 60)),
        },
    ]
}
