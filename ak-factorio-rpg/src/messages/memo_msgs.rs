use ak::Message;
use crate::data::memo::{MemoEntry, MemoEventType};

/// Log an event to the memo journal
pub struct LogEvent(pub MemoEntry);

impl Message for LogEvent {
    type Result = ();
}

/// Query the memo journal
pub struct QueryMemo {
    pub player: Option<String>,
    pub event_type: Option<MemoEventType>,
    pub last_n: usize,
}

impl Message for QueryMemo {
    type Result = Vec<String>;
}

/// Get the world chronicle (major events visible to all)
pub struct GetChronicle {
    pub last_n: usize,
}

impl Message for GetChronicle {
    type Result = Vec<String>;
}
