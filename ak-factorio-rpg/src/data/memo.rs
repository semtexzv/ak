/// The Memo system - a persistent event journal that tracks everything
/// happening in the RPG world. This is the "living memory" of the server.

#[derive(Clone, Debug, PartialEq)]
pub enum MemoEventType {
    PlayerJoined,
    PlayerLeft,
    LevelUp,
    SkillUp,
    ResourceGathered,
    ItemCrafted,
    FactoryBuilt,
    FactoryProduced,
    QuestStarted,
    QuestCompleted,
    PlayerMoved,
    WorldEvent,
    Discovery,
    PlayerNote,
    Achievement,
}

impl MemoEventType {
    pub fn icon(&self) -> &str {
        match self {
            MemoEventType::PlayerJoined => "[+]",
            MemoEventType::PlayerLeft => "[-]",
            MemoEventType::LevelUp => "[^]",
            MemoEventType::SkillUp => "[*]",
            MemoEventType::ResourceGathered => "[R]",
            MemoEventType::ItemCrafted => "[C]",
            MemoEventType::FactoryBuilt => "[F]",
            MemoEventType::FactoryProduced => "[P]",
            MemoEventType::QuestStarted => "[Q]",
            MemoEventType::QuestCompleted => "[!]",
            MemoEventType::PlayerMoved => "[>]",
            MemoEventType::WorldEvent => "[W]",
            MemoEventType::Discovery => "[?]",
            MemoEventType::PlayerNote => "[N]",
            MemoEventType::Achievement => "[A]",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MemoEntry {
    pub tick: u64,
    pub player: Option<String>,
    pub event_type: MemoEventType,
    pub description: String,
}

impl MemoEntry {
    pub fn format_display(&self) -> String {
        let player_str = self.player.as_deref().unwrap_or("World");
        format!("[T:{}] {} {} - {}",
            self.tick,
            self.event_type.icon(),
            player_str,
            self.description
        )
    }
}

/// The Memo Journal - an append-only, queryable event log
#[derive(Clone, Debug)]
pub struct MemoJournal {
    pub entries: Vec<MemoEntry>,
    pub max_entries: usize,
}

impl MemoJournal {
    pub fn new(max_entries: usize) -> Self {
        MemoJournal {
            entries: Vec::new(),
            max_entries,
        }
    }

    pub fn log(&mut self, entry: MemoEntry) {
        self.entries.push(entry);
        // Ring buffer: drop oldest if over capacity
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }

    pub fn query(&self, player: Option<&str>, event_type: Option<&MemoEventType>, last_n: usize) -> Vec<&MemoEntry> {
        self.entries.iter()
            .rev()
            .filter(|e| {
                player.map_or(true, |p| e.player.as_deref() == Some(p))
                    && event_type.map_or(true, |t| &e.event_type == t)
            })
            .take(last_n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    pub fn world_chronicle(&self, last_n: usize) -> Vec<&MemoEntry> {
        self.entries.iter()
            .rev()
            .filter(|e| matches!(e.event_type,
                MemoEventType::LevelUp |
                MemoEventType::FactoryBuilt |
                MemoEventType::QuestCompleted |
                MemoEventType::WorldEvent |
                MemoEventType::Discovery |
                MemoEventType::Achievement |
                MemoEventType::PlayerJoined |
                MemoEventType::PlayerLeft
            ))
            .take(last_n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}
