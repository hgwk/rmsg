use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ControlMessage {
    #[serde(rename = "pk")]
    PublicKey {
        from: String,
        pk: String,
        sid: String,
    },
    #[serde(rename = "typing")]
    Typing {
        from: String,
        sid: String,
    },
    #[serde(rename = "want_since")]
    WantSince {
        from: String,
        since: i64,
        sid: String,
    },
    #[serde(rename = "history_chunk")]
    HistoryChunk {
        from: String,
        since: i64,
        items: Vec<HistoryItem>,
        sid: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub from: String,
    pub ct: String,
    pub nonce: String,
    pub seq: u64,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub from: String,
    pub ct: String,
    pub nonce: String,
    pub seq: u64,
    pub ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckMessage {
    pub id: String,
    pub from: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WireMessage {
    #[serde(rename = "control")]
    Control(ControlMessage),
    #[serde(rename = "chat")]
    Chat(ChatMessage),
    #[serde(rename = "ack")]
    Ack(AckMessage),
}
