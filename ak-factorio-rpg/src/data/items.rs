pub type ItemId = u32;

#[derive(Clone, Debug, PartialEq)]
pub enum ItemCategory {
    RawResource,
    ProcessedMaterial,
    Component,
    Machine,
    Tool,
    Equipment,
    Consumable,
}

#[derive(Clone, Debug)]
pub struct ItemDef {
    pub id: ItemId,
    pub name: String,
    pub category: ItemCategory,
    pub stack_size: u32,
}

// Well-known item IDs (Factorio-inspired)
pub const IRON_ORE: ItemId = 1;
pub const COPPER_ORE: ItemId = 2;
pub const COAL: ItemId = 3;
pub const STONE: ItemId = 4;
pub const WOOD: ItemId = 5;
pub const IRON_PLATE: ItemId = 6;
pub const COPPER_PLATE: ItemId = 7;
pub const STEEL_PLATE: ItemId = 8;
pub const STONE_BRICK: ItemId = 9;
pub const IRON_GEAR: ItemId = 10;
pub const COPPER_WIRE: ItemId = 11;
pub const GREEN_CIRCUIT: ItemId = 12;
pub const RED_CIRCUIT: ItemId = 13;
pub const PIPE: ItemId = 14;
pub const FURNACE: ItemId = 15;
pub const ASSEMBLER: ItemId = 16;
pub const MINING_DRILL: ItemId = 17;
pub const TRANSPORT_BELT: ItemId = 18;
pub const INSERTER: ItemId = 19;
pub const HEALTH_POTION: ItemId = 20;
pub const SPEED_POTION: ItemId = 21;
pub const IRON_PICKAXE: ItemId = 22;
pub const IRON_ARMOR: ItemId = 23;

pub fn build_item_registry() -> Vec<ItemDef> {
    vec![
        ItemDef { id: IRON_ORE, name: "Iron Ore".into(), category: ItemCategory::RawResource, stack_size: 50 },
        ItemDef { id: COPPER_ORE, name: "Copper Ore".into(), category: ItemCategory::RawResource, stack_size: 50 },
        ItemDef { id: COAL, name: "Coal".into(), category: ItemCategory::RawResource, stack_size: 50 },
        ItemDef { id: STONE, name: "Stone".into(), category: ItemCategory::RawResource, stack_size: 50 },
        ItemDef { id: WOOD, name: "Wood".into(), category: ItemCategory::RawResource, stack_size: 50 },
        ItemDef { id: IRON_PLATE, name: "Iron Plate".into(), category: ItemCategory::ProcessedMaterial, stack_size: 100 },
        ItemDef { id: COPPER_PLATE, name: "Copper Plate".into(), category: ItemCategory::ProcessedMaterial, stack_size: 100 },
        ItemDef { id: STEEL_PLATE, name: "Steel Plate".into(), category: ItemCategory::ProcessedMaterial, stack_size: 100 },
        ItemDef { id: STONE_BRICK, name: "Stone Brick".into(), category: ItemCategory::ProcessedMaterial, stack_size: 100 },
        ItemDef { id: IRON_GEAR, name: "Iron Gear".into(), category: ItemCategory::Component, stack_size: 100 },
        ItemDef { id: COPPER_WIRE, name: "Copper Wire".into(), category: ItemCategory::Component, stack_size: 100 },
        ItemDef { id: GREEN_CIRCUIT, name: "Electronic Circuit".into(), category: ItemCategory::Component, stack_size: 100 },
        ItemDef { id: RED_CIRCUIT, name: "Advanced Circuit".into(), category: ItemCategory::Component, stack_size: 100 },
        ItemDef { id: PIPE, name: "Pipe".into(), category: ItemCategory::Component, stack_size: 50 },
        ItemDef { id: FURNACE, name: "Stone Furnace".into(), category: ItemCategory::Machine, stack_size: 10 },
        ItemDef { id: ASSEMBLER, name: "Assembling Machine".into(), category: ItemCategory::Machine, stack_size: 10 },
        ItemDef { id: MINING_DRILL, name: "Mining Drill".into(), category: ItemCategory::Machine, stack_size: 10 },
        ItemDef { id: TRANSPORT_BELT, name: "Transport Belt".into(), category: ItemCategory::Machine, stack_size: 100 },
        ItemDef { id: INSERTER, name: "Inserter".into(), category: ItemCategory::Machine, stack_size: 50 },
        ItemDef { id: HEALTH_POTION, name: "Health Potion".into(), category: ItemCategory::Consumable, stack_size: 20 },
        ItemDef { id: SPEED_POTION, name: "Speed Potion".into(), category: ItemCategory::Consumable, stack_size: 20 },
        ItemDef { id: IRON_PICKAXE, name: "Iron Pickaxe".into(), category: ItemCategory::Tool, stack_size: 1 },
        ItemDef { id: IRON_ARMOR, name: "Iron Armor".into(), category: ItemCategory::Equipment, stack_size: 1 },
    ]
}

pub fn item_name(registry: &[ItemDef], id: ItemId) -> &str {
    registry.iter()
        .find(|i| i.id == id)
        .map(|i| i.name.as_str())
        .unwrap_or("Unknown")
}
