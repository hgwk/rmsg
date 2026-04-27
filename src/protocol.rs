use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub v: u8,
    pub room_id: String,
    pub peers: Vec<String>,
}

impl Invite {
    pub fn new(room_id: String, peers: Vec<String>) -> Self {
        Self {
            v: 1,
            room_id,
            peers,
        }
    }

    pub fn to_uri(&self) -> Result<String, serde_json::Error> {
        use base64::Engine;

        let json = serde_json::to_vec(self)?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        Ok(format!("rmsg://v1/{}", encoded))
    }

    pub fn from_uri(input: &str) -> Option<Self> {
        use base64::Engine;

        let encoded = input.strip_prefix("rmsg://v1/")?;
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .ok()?;
        let invite: Invite = serde_json::from_slice(&json).ok()?;
        (invite.v == 1 && !invite.room_id.is_empty()).then_some(invite)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshEnvelope {
    pub mesh: u8,
    pub id: String,
    pub room_id: String,
    pub origin: String,
    pub ttl: u8,
    pub payload: String,
}

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
    Typing { from: String, sid: String },
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
    #[serde(rename = "peer_exchange")]
    PeerExchange {
        from: String,
        peers: Vec<String>,
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
