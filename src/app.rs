use eframe::egui;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::crypto::KeyPair;
use crate::protocol::{ControlMessage, WireMessage};
use crate::p2p::{P2PEvent, P2PNode};
use crate::session::{ChatEvent, ChatSession};
use crate::storage::MessageStore;

#[derive(PartialEq, Clone)]
enum Screen { Lobby, ChatRoom(String) }

struct RoomInfo { id: String }

pub struct RelayApp {
    screen: Screen,
    store: Arc<MessageStore>,
    kp: Arc<KeyPair>,
    name: String,
    rooms: Vec<RoomInfo>,
    error: String,
    p2p_events: mpsc::UnboundedReceiver<P2PEvent>,
    p2p_cmd: mpsc::UnboundedSender<P2PCmd>,
    session: Option<ChatSession>,
    messages: Vec<ChatEvent>,
    input: String,
    peers: HashSet<String>,
    send_hs: bool,
    peer_typing: bool,
    last_typing_sent: std::time::Instant,
    room_alias: String,
    connection_status: String,
    room_alias_input: String,
    pending_clipboard: Option<String>,
    join_input: String,
    fonts_loaded: bool,
}

enum P2PCmd { JoinRoom(String), Send(String, Vec<u8>) }

impl RelayApp {
    pub fn new(store: Arc<MessageStore>, kp: Arc<KeyPair>) -> Self {
        let rooms = store.list_rooms().unwrap_or_default().into_iter().map(|id| RoomInfo { id }).collect();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        std::thread::spawn(move || { rt.block_on(async {
            let mut node = match P2PNode::new(event_tx) { Ok(n) => n, Err(e) => { log::error!("P2P init: {}", e); return; } };
            let mut cmd_rx = cmd_rx;
            loop { tokio::select! {
                _ = node.run() => {},
                cmd = cmd_rx.recv() => { match cmd {
                    Some(P2PCmd::JoinRoom(room_id)) => { node.join_room(&room_id).ok(); }
                    Some(P2PCmd::Send(room_id, data)) => { node.send_message(&room_id, data).ok(); }
                    None => break,
                }}
            }}
        })});
        Self { screen: Screen::Lobby, store, kp, name: String::new(), rooms, error: String::new(),
            p2p_events: event_rx, p2p_cmd: cmd_tx, session: None, messages: Vec::new(), input: String::new(),
            peers: HashSet::new(), send_hs: false, peer_typing: false, last_typing_sent: std::time::Instant::now(),
            room_alias: String::new(), connection_status: String::new(), room_alias_input: String::new(),
            pending_clipboard: None, join_input: String::new(), fonts_loaded: false }
    }
}

impl eframe::App for RelayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.fonts_loaded {
            self.fonts_loaded = true;
            let mut fonts = egui::FontDefinitions::default();
            if let Ok(data) = std::fs::read("/System/Library/Fonts/AppleSDGothicNeo.ttc") {
                fonts.font_data.insert("korean".into(), std::sync::Arc::new(egui::FontData::from_owned(data)));
                fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "korean".into());
            }
            if let Ok(data) = std::fs::read("/System/Library/Fonts/Apple Color Emoji.ttc") {
                fonts.font_data.insert("emoji".into(), std::sync::Arc::new(egui::FontData::from_owned(data)));
                fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().push("emoji".into());
            }
            ctx.set_fonts(fonts);
        }
        self.drain_p2p_events();
        if let Some(text) = self.pending_clipboard.take() { ctx.copy_text(text); }
        egui::CentralPanel::default().show(ctx, |ui| match &self.screen {
            Screen::Lobby => self.show_lobby(ui),
            Screen::ChatRoom(rid) => { let rid = rid.clone(); self.show_chat(ui, &rid); }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

impl RelayApp {
    fn drain_p2p_events(&mut self) {
        while let Ok(event) = self.p2p_events.try_recv() { match event {
            P2PEvent::Message { data, .. } => if let Some(ref mut session) = self.session {
                if let Ok(wire) = serde_json::from_slice::<WireMessage>(&data) {
                    if matches!(wire, WireMessage::Control(ControlMessage::Typing { .. })) { self.peer_typing = true; continue; }
                }
                let events = session.handle_raw(&data);
                for ev in &events { if !ev.mine && ev.from != "system" { self.connection_status = format!("Chatting with {}", ev.from); } }
                self.messages.extend(events);
            },
            P2PEvent::Subscribed => {
                if !self.peers.is_empty() { self.connection_status = format!("Connected ({} peer{})", self.peers.len(), if self.peers.len() == 1 { "" } else { "s" }); }
                if self.send_hs { self.send_hs = false;
                    if let Some(ref session) = self.session {
                        let rid = session.room_id().to_string();
                        let _ = self.p2p_cmd.send(P2PCmd::Send(rid.clone(), session.mk_handshake().into_bytes()));
                        let _ = self.p2p_cmd.send(P2PCmd::Send(rid, session.build_want_since().into_bytes()));
                    }
                }
            },
            P2PEvent::PeerDiscovered(peer) => { self.peers.insert(peer.to_string()); self.connection_status = format!("Connected ({} peer{})", self.peers.len(), if self.peers.len() == 1 { "" } else { "s" }); }
            P2PEvent::Error(e) => { self.error = e; }
        }}
    }

    fn show_lobby(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.heading(egui::RichText::new("● relay").size(28.0));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("No signup. No phone. No server.").color(egui::Color32::GRAY));
            ui.add_space(2.0);
            let net_dot = if self.peers.is_empty() { "○" } else { "●" };
            let net_color = if self.peers.is_empty() { egui::Color32::DARK_GRAY } else { egui::Color32::GREEN };
            ui.label(egui::RichText::new(format!("{} network ({} peer{})", net_dot, self.peers.len(), if self.peers.len() == 1 { "" } else { "s" })).color(net_color).size(11.0));
        });
        ui.add_space(16.0);
        ui.horizontal(|ui| { ui.label("Your name:"); ui.text_edit_singleline(&mut self.name); });
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        if self.rooms.is_empty() {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("No conversations yet").color(egui::Color32::GRAY).size(13.0));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Create a room and share the code with your friend.").color(egui::Color32::DARK_GRAY).size(11.0));
            });
        } else {
            let mut selected = None;
            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| for room in &self.rooms { if ui.button(&room.id).clicked() { selected = Some(room.id.clone()); } });
            if let Some(rid) = selected { self.enter_room(rid); }
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.vertical_centered(|ui| {
            if ui.button(egui::RichText::new("+ Create new room").size(14.0)).clicked() {
                let room_id = format!("room-{}", &uuid::Uuid::new_v4().to_string()[..8]);
                self.enter_room(room_id);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("or join:");
                let resp = ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.join_input).hint_text("paste room code..."));
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.join_input.is_empty() {
                    let rid = self.join_input.trim().to_string(); self.join_input.clear(); self.enter_room(rid);
                }
            });
        });

        if !self.error.is_empty() { ui.add_space(8.0); ui.colored_label(egui::Color32::RED, &self.error); }
    }

    fn enter_room(&mut self, room_id: String) {
        let kp = KeyPair::from_bytes(&self.kp.to_bytes());
        let session = ChatSession::new(room_id.clone(), self.name.clone(), self.store.clone(), kp);
        self.session = Some(session);
        self.messages.clear();
        self.send_hs = true;
        self.connection_status = "Waiting for peer...".into();
        self.room_alias = room_id.clone();
        self.room_alias_input = String::new();
        self.pending_clipboard = Some(room_id.clone());
        self.messages.push(ChatEvent { id: uuid::Uuid::new_v4().to_string(), from: "system".into(), text: format!("Room: {} — copied to clipboard. Share with your peer.", room_id), ts: now_ms(), mine: false });
        let _ = self.p2p_cmd.send(P2PCmd::JoinRoom(room_id.clone()));
        self.screen = Screen::ChatRoom(room_id);
    }

    fn show_chat(&mut self, ui: &mut egui::Ui, room_id: &str) {
        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() { self.screen = Screen::Lobby; self.session = None; self.connection_status.clear(); return; }
            ui.heading(if self.room_alias != *room_id { &self.room_alias } else { room_id });
        });
        let ready = self.session.as_ref().map(|s| s.is_ready()).unwrap_or(false);
        let (color, text) = if ready {
            (egui::Color32::GREEN,
             if self.connection_status.contains("Chatting") { format!("🔒 End-to-end encrypted — {}", self.connection_status) }
             else { "🔒 End-to-end encrypted".into() })
        } else if self.connection_status.is_empty() { (egui::Color32::YELLOW, "⏳ Waiting for peer...".into()) }
            else { (egui::Color32::YELLOW, self.connection_status.clone()) };
        ui.colored_label(color, &text);
        ui.horizontal(|ui| {
            if ui.small_button("📋 Copy ID").clicked() { self.pending_clipboard = Some(room_id.to_string()); }
            ui.label("Alias:");
            ui.add_sized([120.0, 18.0], egui::TextEdit::singleline(&mut self.room_alias_input).hint_text("optional"));
        });
        ui.separator();
        let available_height = ui.available_height() - 80.0;
        egui::ScrollArea::vertical().max_height(available_height).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            if self.messages.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(egui::RichText::new("No messages yet").color(egui::Color32::GRAY).size(14.0));
                    ui.label(egui::RichText::new("Share the room code with your peer to start chatting.").color(egui::Color32::DARK_GRAY).size(11.0));
                });
            }
            let mut last_date = String::new();
            for msg in &self.messages {
                let date = format_date(msg.ts);
                if date != last_date {
                    last_date = date.clone();
                    ui.vertical_centered(|ui| {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&date).color(egui::Color32::from_gray(150)).size(11.0));
                        ui.add_space(2.0);
                    });
                }
                if msg.from == "system" {
                    ui.vertical_centered(|ui| { ui.label(egui::RichText::new(&msg.text).color(egui::Color32::GRAY).size(11.0).italics()); });
                } else if msg.mine {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&format_time(msg.ts)).color(egui::Color32::DARK_GRAY).size(10.0));
                            ui.colored_label(egui::Color32::from_rgb(0, 120, 255), &msg.text);
                        });
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(60, 60, 60), &msg.text);
                        ui.label(egui::RichText::new(&format_time(msg.ts)).color(egui::Color32::DARK_GRAY).size(10.0));
                    });
                }
                ui.add_space(2.0);
            }
            if self.peer_typing { ui.label(egui::RichText::new("typing...").color(egui::Color32::GRAY).size(11.0).italics()); self.peer_typing = false; }
        });
        ui.separator();
        let mut send_triggered = false;
        ui.horizontal(|ui| {
            let resp = ui.add_sized([ui.available_width() - 60.0, 20.0], egui::TextEdit::singleline(&mut self.input).hint_text("Type a message..."));
            if resp.changed() { let now = std::time::Instant::now(); if now.duration_since(self.last_typing_sent).as_millis() > 500 { self.last_typing_sent = now;
                let tm = serde_json::to_string(&WireMessage::Control(ControlMessage::Typing { from: self.name.clone(), sid: self.name.clone() })).unwrap_or_default();
                if let Some(ref session) = self.session { let _ = self.p2p_cmd.send(P2PCmd::Send(session.room_id().to_string(), tm.into_bytes())); }
            }}
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { send_triggered = true; }
            if ui.button("Send").clicked() { send_triggered = true; }
        });
        if send_triggered { let text = self.input.trim().to_string(); if !text.is_empty() {
            let ts = now_ms();
            if let Some(ref mut session) = self.session { if session.is_ready() { if let Ok(encoded) = session.encrypt_and_encode(&text) { let _ = self.p2p_cmd.send(P2PCmd::Send(room_id.to_string(), encoded.into_bytes())); } } }
            self.messages.push(ChatEvent { id: uuid::Uuid::new_v4().to_string(), from: self.name.clone(), text, ts, mine: true }); self.input.clear();
        }}
    }
}

fn format_time(ts_ms: i64) -> String { let secs = ts_ms / 1000; let m = secs / 60; let h = m / 60; format!("{:02}:{:02}", h % 24, m % 60) }
fn format_date(ts_ms: i64) -> String {
    let days = ts_ms / 86400000;
    let now_days = now_ms() / 86400000;
    if days == now_days { return "Today".into(); }
    if days == now_days - 1 { return "Yesterday".into(); }
    let secs = ts_ms / 1000;
    let d = secs / 86400;
    format!("{:04}-{:02}-{:02}", 1970 + d / 365, (d % 365) / 30 + 1, (d % 365) % 30 + 1)
}
fn now_ms() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64 }
