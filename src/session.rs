use std::collections::HashSet;
use std::sync::Arc;

use crate::crypto::{CryptoSession, KeyPair};
use crate::protocol::*;
use crate::storage::{MessageStore, StoredMessage};

#[derive(Debug, Clone)]
pub struct ChatEvent {
    #[allow(dead_code)]
    pub id: String,
    pub from: String,
    pub text: String,
    #[allow(dead_code)]
    pub ts: i64,
    pub mine: bool,
}

pub struct ChatSession {
    room_id: String,
    user_id: String,
    seq: u64,
    kp: KeyPair,
    peer_pk: Option<[u8; 32]>,
    cs: CryptoSession,
    ready: bool,
    seen: HashSet<String>,
    store: Arc<MessageStore>,
}

impl ChatSession {
    pub fn new(room_id: String, user_id: String, store: Arc<MessageStore>, kp: KeyPair) -> Self {
        let cs = CryptoSession::new(room_id.clone());
        Self {
            room_id,
            user_id,
            seq: 0,
            kp,
            peer_pk: None,
            cs,
            ready: false,
            seen: HashSet::new(),
            store,
        }
    }

    pub fn room_id(&self) -> &str {
        &self.room_id
    }
    pub fn is_ready(&self) -> bool {
        self.ready
    }
    #[allow(dead_code)]
    pub fn current_seq(&self) -> u64 {
        self.seq
    }

    pub fn set_peer_pk(&mut self, pk: &[u8; 32]) -> Result<(), String> {
        self.peer_pk = Some(*pk);
        self.cs.start(&self.kp.sk, pk).map_err(|e| e.to_string())?;
        self.ready = true;
        Ok(())
    }

    pub fn handle_raw(&mut self, raw: &[u8]) -> Vec<ChatEvent> {
        let wire: WireMessage = match serde_json::from_slice(raw) {
            Ok(w) => w,
            Err(_) => return vec![],
        };
        self.process_wire(wire)
    }

    pub fn build_want_since(&self) -> String {
        let last_ts = self
            .store
            .last_ts(&self.room_id)
            .unwrap_or_default()
            .unwrap_or(0);
        serde_json::to_string(&WireMessage::Control(ControlMessage::WantSince {
            from: self.user_id.clone(),
            since: last_ts,
            sid: self.user_id.clone(),
        }))
        .expect("json encode want_since")
    }

    fn process_wire(&mut self, wire: WireMessage) -> Vec<ChatEvent> {
        let mut events = vec![];
        match wire {
            WireMessage::Chat(msg) => {
                if msg.from == self.user_id || self.seen.contains(&msg.id) {
                    return events;
                }
                self.seen.insert(msg.id.clone());
                if let (Ok(c), Ok(n)) = (
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &msg.ct),
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &msg.nonce),
                ) {
                    if n.len() == 12 {
                        let mut nonce = [0u8; 12];
                        nonce.copy_from_slice(&n);
                        if let Ok(plain) = self.cs.decrypt(&c, &nonce, msg.seq) {
                            let text = String::from_utf8_lossy(&plain).to_string();
                            self.store
                                .save(&StoredMessage {
                                    id: msg.id.clone(),
                                    room_id: self.room_id.clone(),
                                    from_id: msg.from.clone(),
                                    text: text.clone(),
                                    ts: msg.ts,
                                })
                                .ok();
                            events.push(ChatEvent {
                                id: msg.id,
                                from: msg.from,
                                text,
                                ts: msg.ts,
                                mine: false,
                            });
                        }
                    }
                }
            }
            WireMessage::Control(ctrl) => match ctrl {
                ControlMessage::PublicKey { from, pk, .. } => {
                    if from != self.user_id && !self.ready {
                        if let Ok(bytes) =
                            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &pk)
                        {
                            if bytes.len() == 32 {
                                let mut arr = [0u8; 32];
                                arr.copy_from_slice(&bytes);
                                self.set_peer_pk(&arr).ok();
                                events.push(ChatEvent {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    from: "system".into(),
                                    text: "Encryption established".into(),
                                    ts: chrono_now(),
                                    mine: false,
                                });
                            }
                        }
                    }
                }
                ControlMessage::HistoryChunk { from, items, .. } => {
                    if from == self.user_id {
                        return events;
                    }
                    for item in items {
                        if let (Ok(c), Ok(n)) = (
                            base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                &item.ct,
                            ),
                            base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                &item.nonce,
                            ),
                        ) {
                            if n.len() == 12 {
                                let mut nonce = [0u8; 12];
                                nonce.copy_from_slice(&n);
                                if let Ok(plain) = self.cs.decrypt(&c, &nonce, item.seq) {
                                    let text = String::from_utf8_lossy(&plain).to_string();
                                    self.store
                                        .save(&StoredMessage {
                                            id: item.id.clone(),
                                            room_id: self.room_id.clone(),
                                            from_id: item.from.clone(),
                                            text: text.clone(),
                                            ts: item.ts,
                                        })
                                        .ok();
                                    events.push(ChatEvent {
                                        id: item.id,
                                        from: item.from,
                                        text,
                                        ts: item.ts,
                                        mine: false,
                                    });
                                }
                            }
                        }
                    }
                }
                ControlMessage::PeerExchange { .. } => {}
                _ => {}
            },
            WireMessage::Ack(_) => {}
        }
        events
    }

    pub fn encrypt_and_encode(&mut self, text: &str) -> Result<String, String> {
        self.seq += 1;
        let id = uuid::Uuid::new_v4().to_string();
        let enc = self
            .cs
            .encrypt(text.as_bytes(), self.seq)
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&WireMessage::Chat(ChatMessage {
            id,
            from: self.user_id.clone(),
            ct: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &enc.ct),
            nonce: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, enc.nonce),
            seq: self.seq,
            ts: chrono_now(),
            sid: Some(self.user_id.clone()),
        }))
        .expect("json encode chat message"))
    }

    pub fn mk_handshake(&self) -> String {
        let pk_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, self.kp.pk);
        serde_json::to_string(&WireMessage::Control(ControlMessage::PublicKey {
            from: self.user_id.clone(),
            pk: pk_b64,
            sid: self.user_id.clone(),
        }))
        .expect("json encode handshake")
    }

    pub fn build_peer_exchange(&self, peers: Vec<String>) -> String {
        serde_json::to_string(&WireMessage::Control(ControlMessage::PeerExchange {
            from: self.user_id.clone(),
            peers,
            sid: self.user_id.clone(),
        }))
        .expect("json encode peer exchange")
    }

    #[allow(dead_code)]
    pub fn build_history_chunk(&mut self, since: i64) -> String {
        let msgs = self.store.load_all(&self.room_id).unwrap_or_default();
        let missing: Vec<_> = msgs.iter().filter(|m| m.ts > since).collect();
        let mut items = vec![];
        for m in &missing {
            self.seq += 1;
            let enc = self
                .cs
                .encrypt(m.text.as_bytes(), self.seq)
                .map(|e| (e.ct, e.nonce))
                .unwrap_or_default();
            items.push(HistoryItem {
                id: m.id.clone(),
                from: m.from_id.clone(),
                ct: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &enc.0),
                nonce: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, enc.1),
                seq: self.seq,
                ts: m.ts,
            });
        }
        serde_json::to_string(&WireMessage::Control(ControlMessage::HistoryChunk {
            from: self.user_id.clone(),
            since,
            items,
            sid: self.user_id.clone(),
        }))
        .expect("json encode history chunk")
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
