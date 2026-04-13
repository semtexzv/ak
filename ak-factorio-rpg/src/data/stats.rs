/// Core RPG character stats, inspired by Diablo/Comfy RPG systems.
/// - STR: melee damage, carry capacity, mining power
/// - DEX: movement speed, crafting speed, gathering speed
/// - INT: unlocks advanced recipes, research bonus, XP multiplier
/// - CON: max HP, pollution resistance, defense
/// - WIS: memo journal detail, XP gain bonus, mana regen
/// - CHA: trade prices, NPC interaction (future)

#[derive(Clone, Debug)]
pub struct Stats {
    pub strength: u32,
    pub dexterity: u32,
    pub intelligence: u32,
    pub constitution: u32,
    pub wisdom: u32,
    pub charisma: u32,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            strength: 5,
            dexterity: 5,
            intelligence: 5,
            constitution: 5,
            wisdom: 5,
            charisma: 5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DerivedStats {
    pub max_hp: u32,
    pub carry_capacity: u32,
    pub gather_speed_mult: f32,
    pub craft_speed_mult: f32,
    pub xp_mult: f32,
    pub melee_damage: u32,
    pub move_speed_mult: f32,
}

impl DerivedStats {
    pub fn from_stats(stats: &Stats) -> Self {
        DerivedStats {
            max_hp: 50 + stats.constitution * 10,
            carry_capacity: 10 + stats.strength * 2,
            gather_speed_mult: 1.0 + stats.dexterity as f32 * 0.05,
            craft_speed_mult: 1.0 + stats.dexterity as f32 * 0.04,
            xp_mult: 1.0 + stats.wisdom as f32 * 0.08,
            melee_damage: 5 + stats.strength * 3,
            move_speed_mult: 1.0 + stats.dexterity as f32 * 0.03,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub level: u32,
    pub xp: u64,
}

impl Skill {
    pub fn new(name: &str) -> Self {
        Skill {
            name: name.to_string(),
            level: 1,
            xp: 0,
        }
    }

    pub fn xp_to_next(&self) -> u64 {
        (self.level as u64) * 100 + 50
    }

    pub fn add_xp(&mut self, amount: u64) -> bool {
        self.xp += amount;
        let mut leveled = false;
        while self.xp >= self.xp_to_next() {
            self.xp -= self.xp_to_next();
            self.level += 1;
            leveled = true;
        }
        leveled
    }
}

/// XP required to reach the next level from the current level.
pub fn xp_for_level(level: u32) -> u64 {
    // Quadratic scaling: levels get progressively harder
    (level as u64) * (level as u64) * 50 + 100
}

#[derive(Clone, Debug)]
pub struct CharacterState {
    pub name: String,
    pub level: u32,
    pub xp: u64,
    pub stats: Stats,
    pub derived: DerivedStats,
    pub hp: u32,
    pub position: (i32, i32),
    pub stat_points: u32,
    pub skills: Vec<Skill>,
}

impl CharacterState {
    pub fn new(name: String) -> Self {
        let stats = Stats::default();
        let derived = DerivedStats::from_stats(&stats);
        let hp = derived.max_hp;
        CharacterState {
            name,
            level: 1,
            xp: 0,
            stats,
            derived,
            hp,
            position: (0, 0),
            stat_points: 0,
            skills: vec![
                Skill::new("Mining"),
                Skill::new("Smelting"),
                Skill::new("Crafting"),
                Skill::new("Logistics"),
                Skill::new("Combat"),
            ],
        }
    }

    pub fn xp_to_next(&self) -> u64 {
        xp_for_level(self.level)
    }

    /// Add XP, applying wisdom multiplier. Returns true if leveled up.
    pub fn add_xp(&mut self, base_amount: u64) -> bool {
        let actual = (base_amount as f32 * self.derived.xp_mult) as u64;
        self.xp += actual;
        let mut leveled = false;
        while self.xp >= self.xp_to_next() {
            self.xp -= self.xp_to_next();
            self.level += 1;
            self.stat_points += 3;
            leveled = true;
        }
        if leveled {
            self.derived = DerivedStats::from_stats(&self.stats);
            self.hp = self.derived.max_hp; // full heal on level up
        }
        leveled
    }

    pub fn allocate_stat(&mut self, stat_name: &str) -> bool {
        if self.stat_points == 0 {
            return false;
        }
        match stat_name {
            "str" | "strength" => self.stats.strength += 1,
            "dex" | "dexterity" => self.stats.dexterity += 1,
            "int" | "intelligence" => self.stats.intelligence += 1,
            "con" | "constitution" => self.stats.constitution += 1,
            "wis" | "wisdom" => self.stats.wisdom += 1,
            "cha" | "charisma" => self.stats.charisma += 1,
            _ => return false,
        }
        self.stat_points -= 1;
        self.derived = DerivedStats::from_stats(&self.stats);
        true
    }

    pub fn get_skill(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    pub fn add_skill_xp(&mut self, name: &str, amount: u64) -> bool {
        if let Some(skill) = self.skills.iter_mut().find(|s| s.name == name) {
            skill.add_xp(amount)
        } else {
            false
        }
    }
}
