use super::items;

#[derive(Clone, Debug, PartialEq)]
pub enum TileType {
    Grass,
    Water,
    Rock,
    Forest,
    Sand,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResourceType {
    IronOre,
    CopperOre,
    Coal,
    Stone,
    Wood,
}

impl ResourceType {
    pub fn item_id(&self) -> items::ItemId {
        match self {
            ResourceType::IronOre => items::IRON_ORE,
            ResourceType::CopperOre => items::COPPER_ORE,
            ResourceType::Coal => items::COAL,
            ResourceType::Stone => items::STONE,
            ResourceType::Wood => items::WOOD,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ResourceType::IronOre => "Iron Ore Deposit",
            ResourceType::CopperOre => "Copper Ore Deposit",
            ResourceType::Coal => "Coal Deposit",
            ResourceType::Stone => "Stone Quarry",
            ResourceType::Wood => "Forest Grove",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceNode {
    pub resource_type: ResourceType,
    pub quantity: u32,
    pub max_quantity: u32,
    pub regen_rate: u32,
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub tile_type: TileType,
    pub resource: Option<ResourceNode>,
    pub factory_id: Option<u64>,
    pub owner: Option<String>,
}

impl Default for Tile {
    fn default() -> Self {
        Tile {
            tile_type: TileType::Grass,
            resource: None,
            factory_id: None,
            owner: None,
        }
    }
}

pub struct WorldMap {
    pub width: i32,
    pub height: i32,
    pub tiles: Vec<Vec<Tile>>,
}

impl WorldMap {
    pub fn new(width: i32, height: i32) -> Self {
        let tiles = (0..height as usize)
            .map(|_| (0..width as usize).map(|_| Tile::default()).collect())
            .collect();
        WorldMap { width, height, tiles }
    }

    /// Generate a procedural world with resource patches
    pub fn generate(width: i32, height: i32, seed: u64) -> Self {
        let mut map = WorldMap::new(width, height);

        // Simple pseudo-random using seed (no external dependency)
        let mut rng = seed;
        let next_rand = |rng: &mut u64| -> u64 {
            *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *rng >> 33
        };

        // Place terrain features
        for y in 0..height as usize {
            for x in 0..width as usize {
                let r = next_rand(&mut rng) % 100;
                map.tiles[y][x].tile_type = if r < 3 {
                    TileType::Water
                } else if r < 8 {
                    TileType::Rock
                } else if r < 15 {
                    TileType::Forest
                } else {
                    TileType::Grass
                };
            }
        }

        // Place resource patches (clusters)
        let resource_types = [
            (ResourceType::IronOre, 8, 500, 2),    // 8 patches, 500 max qty, regen 2/tick
            (ResourceType::CopperOre, 6, 400, 2),
            (ResourceType::Coal, 5, 600, 3),
            (ResourceType::Stone, 5, 300, 1),
            (ResourceType::Wood, 10, 200, 5),
        ];

        for (res_type, patch_count, max_qty, regen) in &resource_types {
            for _ in 0..*patch_count {
                let cx = (next_rand(&mut rng) % (width as u64 - 4)) as i32 + 2;
                let cy = (next_rand(&mut rng) % (height as u64 - 4)) as i32 + 2;
                let patch_size = (next_rand(&mut rng) % 3) as i32 + 1;

                for dy in -patch_size..=patch_size {
                    for dx in -patch_size..=patch_size {
                        let nx = cx + dx;
                        let ny = cy + dy;
                        if nx >= 0 && nx < width && ny >= 0 && ny < height {
                            let tile = &mut map.tiles[ny as usize][nx as usize];
                            if tile.tile_type != TileType::Water && tile.resource.is_none() {
                                let qty_var = (next_rand(&mut rng) % (*max_qty as u64 / 2)) as u32;
                                tile.resource = Some(ResourceNode {
                                    resource_type: res_type.clone(),
                                    quantity: *max_qty / 2 + qty_var,
                                    max_quantity: *max_qty,
                                    regen_rate: *regen,
                                });
                                tile.tile_type = TileType::Grass; // resource tiles are always accessible
                            }
                        }
                    }
                }
            }
        }

        // Clear spawn area (center of map)
        let cx = width / 2;
        let cy = height / 2;
        for dy in -2..=2i32 {
            for dx in -2..=2i32 {
                let x = (cx + dx) as usize;
                let y = (cy + dy) as usize;
                if x < width as usize && y < height as usize {
                    map.tiles[y][x] = Tile {
                        tile_type: TileType::Grass,
                        resource: None,
                        factory_id: None,
                        owner: None,
                    };
                }
            }
        }

        map
    }

    pub fn get_tile(&self, x: i32, y: i32) -> Option<&Tile> {
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(&self.tiles[y as usize][x as usize])
        } else {
            None
        }
    }

    pub fn get_tile_mut(&mut self, x: i32, y: i32) -> Option<&mut Tile> {
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(&mut self.tiles[y as usize][x as usize])
        } else {
            None
        }
    }

    pub fn is_passable(&self, x: i32, y: i32) -> bool {
        self.get_tile(x, y)
            .map(|t| t.tile_type != TileType::Water && t.tile_type != TileType::Rock)
            .unwrap_or(false)
    }
}
