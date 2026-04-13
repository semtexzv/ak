use std::future::Future;
use ak::{Actor, Handler, ContextRef, Addr, Message};
use crate::data::memo::{MemoJournal, MemoEntry};
use crate::messages::memo_msgs::*;

/// The MemoLogActor is the persistent event journal.
/// It records everything that happens in the RPG world and allows
/// players to query their personal history or the world chronicle.
pub struct MemoLogActor {
    pub journal: MemoJournal,
}

impl Actor for MemoLogActor {}

impl Handler<LogEvent> for MemoLogActor {
    type Future = impl Future<Output = ()> + 'static;

    fn handle(mut self: ContextRef<Self>, msg: LogEvent) -> Self::Future {
        self.journal.log(msg.0);
        async {}
    }
}

impl Handler<QueryMemo> for MemoLogActor {
    type Future = impl Future<Output = Vec<String>> + 'static;

    fn handle(self: ContextRef<Self>, msg: QueryMemo) -> Self::Future {
        let results = self.journal.query(
            msg.player.as_deref(),
            msg.event_type.as_ref(),
            msg.last_n,
        ).iter().map(|e| e.format_display()).collect();
        async move { results }
    }
}

impl Handler<GetChronicle> for MemoLogActor {
    type Future = impl Future<Output = Vec<String>> + 'static;

    fn handle(self: ContextRef<Self>, msg: GetChronicle) -> Self::Future {
        let results = self.journal.world_chronicle(msg.last_n)
            .iter().map(|e| e.format_display()).collect();
        async move { results }
    }
}
