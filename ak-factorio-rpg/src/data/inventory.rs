use super::items::ItemId;

#[derive(Clone, Debug)]
pub struct ItemStack {
    pub item_id: ItemId,
    pub quantity: u32,
}

#[derive(Clone, Debug)]
pub struct Inventory {
    pub slots: Vec<Option<ItemStack>>,
    pub max_slots: usize,
}

impl Inventory {
    pub fn new(max_slots: usize) -> Self {
        Inventory {
            slots: vec![None; max_slots],
            max_slots,
        }
    }

    /// Add items to inventory. Returns number of items that could NOT be added (overflow).
    pub fn add_item(&mut self, item_id: ItemId, mut quantity: u32, stack_size: u32) -> u32 {
        // First, try to stack onto existing slots with same item
        for slot in self.slots.iter_mut() {
            if quantity == 0 { break; }
            if let Some(ref mut stack) = slot {
                if stack.item_id == item_id {
                    let space = stack_size.saturating_sub(stack.quantity);
                    let add = quantity.min(space);
                    stack.quantity += add;
                    quantity -= add;
                }
            }
        }
        // Then, fill empty slots
        for slot in self.slots.iter_mut() {
            if quantity == 0 { break; }
            if slot.is_none() {
                let add = quantity.min(stack_size);
                *slot = Some(ItemStack { item_id, quantity: add });
                quantity -= add;
            }
        }
        quantity // leftover
    }

    /// Remove items from inventory. Returns number actually removed.
    pub fn remove_item(&mut self, item_id: ItemId, mut quantity: u32) -> u32 {
        let mut removed = 0u32;
        for slot in self.slots.iter_mut() {
            if quantity == 0 { break; }
            if let Some(ref mut stack) = slot {
                if stack.item_id == item_id {
                    let take = quantity.min(stack.quantity);
                    stack.quantity -= take;
                    quantity -= take;
                    removed += take;
                    if stack.quantity == 0 {
                        *slot = None;
                    }
                }
            }
        }
        removed
    }

    pub fn count_item(&self, item_id: ItemId) -> u32 {
        self.slots.iter()
            .filter_map(|s| s.as_ref())
            .filter(|s| s.item_id == item_id)
            .map(|s| s.quantity)
            .sum()
    }

    pub fn has_item(&self, item_id: ItemId, quantity: u32) -> bool {
        self.count_item(item_id) >= quantity
    }

    pub fn items_list(&self) -> Vec<(ItemId, u32)> {
        let mut map = std::collections::HashMap::new();
        for slot in self.slots.iter().filter_map(|s| s.as_ref()) {
            *map.entry(slot.item_id).or_insert(0u32) += slot.quantity;
        }
        let mut list: Vec<_> = map.into_iter().collect();
        list.sort_by_key(|&(id, _)| id);
        list
    }

    pub fn used_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}
