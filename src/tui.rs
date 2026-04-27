use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use std::collections::HashSet;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::crypto::KeyPair;
use crate::p2p::{P2PEvent, P2PNode};
use crate::protocol::{ControlMessage, Invite, WireMessage};
use crate::session::ChatSession;
use crate::storage::MessageStore;

pub fn run_tui(store: Arc<MessageStore>, kp: Arc<KeyPair>, name: String) {
    enable_raw_mode().ok();
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (key_tx, key_rx) = std_mpsc::channel();

    let _rt_handle = rt.handle().clone();
    std::thread::spawn(move || {
        rt.block_on(async {
            let mut node = P2PNode::new(event_tx, None).unwrap();
            let mut cmd_rx = cmd_rx;
            loop {
                tokio::select! {
                    _ = node.run() => {},
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(TuiCmd::Join(rid)) => { node.join_room(&rid).ok(); }
                            Some(TuiCmd::Send(rid, data)) => { node.send_message(&rid, data).ok(); }
                            Some(TuiCmd::Dial(addr)) => { node.dial_addr(&addr).ok(); }
                            Some(TuiCmd::Reserve(addr)) => { node.reserve_relay(&addr).ok(); }
                            None => break,
                        }
                    }
                }
            }
        });
    });

    std::thread::spawn(move || loop {
        if let Ok(Event::Key(key)) = event::read() {
            let _ = key_tx.send(key);
        }
    });

    let rooms = store.list_rooms().unwrap_or_default();
    let mut state = TuiState {
        rooms: rooms.clone(),
        selected_room: None,
        messages: vec![],
        input: String::new(),
        current_room: None,
        session: None,
        name,
        store,
        kp,
        status: "● network ready".into(),
        peers: 0,
        room_input: String::new(),
        known_peers: HashSet::new(),
        invite_uri: None,
        last_intro: Instant::now(),
        mode: TuiMode::Lobby,
    };

    let key_rx = key_rx;
    loop {
        while let Ok(event) = event_rx.try_recv() {
            match event {
                P2PEvent::Message { data, .. } => {
                    if let Ok(WireMessage::Control(ControlMessage::PeerExchange {
                        peers, ..
                    })) = serde_json::from_slice(&data)
                    {
                        for peer in peers {
                            add_known_peer(&mut state, &cmd_tx, peer, true);
                        }
                    }
                    if let Ok(WireMessage::Control(ControlMessage::WantSince { since, .. })) =
                        serde_json::from_slice(&data)
                    {
                        if let Some(ref mut session) = state.session {
                            if session.is_ready() {
                                let chunk = session.build_history_chunk(since);
                                let _ = cmd_tx.send(TuiCmd::Send(
                                    session.room_id().into(),
                                    chunk.into_bytes(),
                                ));
                            }
                        }
                    }
                    if let Some(ref mut session) = state.session {
                        for ev in session.handle_raw(&data) {
                            state.messages.push(ev);
                            if state.messages.len() > 500 {
                                state.messages.remove(0);
                            }
                        }
                    }
                }
                P2PEvent::PeerDiscovered(_) => {
                    state.peers += 1;
                    state.status = format!("● {} peer(s)", state.peers);
                }
                P2PEvent::Subscribed => send_intro(&mut state, &cmd_tx),
                P2PEvent::RoomsDiscovered(rooms) => {
                    for r in rooms {
                        if !state.rooms.contains(&r) {
                            state.rooms.push(r);
                        }
                    }
                }
                P2PEvent::PeerAddress(addr) => add_known_peer(&mut state, &cmd_tx, addr, true),
                P2PEvent::Listening(addr) => add_known_peer(&mut state, &cmd_tx, addr, false),
                _ => {}
            }
        }

        while let Ok(key) = key_rx.try_recv() {
            handle_key(&mut state, key, &cmd_tx);
        }

        if state.session.as_ref().is_some_and(|s| !s.is_ready())
            && state.last_intro.elapsed() >= Duration::from_secs(2)
        {
            send_intro(&mut state, &cmd_tx);
            state.last_intro = Instant::now();
        }

        terminal.draw(|f| draw_ui(f, &state)).unwrap();

        if state.mode == TuiMode::Quit {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
}

#[derive(PartialEq)]
enum TuiMode {
    Lobby,
    Chat,
    JoinInput,
    RelayInput,
    Quit,
}

enum TuiCmd {
    Join(String),
    Send(String, Vec<u8>),
    Dial(String),
    Reserve(String),
}

struct TuiState {
    rooms: Vec<String>,
    selected_room: Option<String>,
    messages: Vec<crate::session::ChatEvent>,
    input: String,
    current_room: Option<String>,
    session: Option<ChatSession>,
    name: String,
    store: Arc<MessageStore>,
    kp: Arc<KeyPair>,
    status: String,
    peers: usize,
    room_input: String,
    known_peers: HashSet<String>,
    invite_uri: Option<String>,
    last_intro: Instant,
    mode: TuiMode,
}

fn handle_key(state: &mut TuiState, key: event::KeyEvent, cmd: &mpsc::UnboundedSender<TuiCmd>) {
    match state.mode {
        TuiMode::Lobby => match key.code {
            KeyCode::Char('q') => state.mode = TuiMode::Quit,
            KeyCode::Char('1') | KeyCode::Char('n') => {
                let room_id = format!("room-{}", &uuid::Uuid::new_v4().to_string()[..8]);
                enter_room(state, &room_id, cmd);
            }
            KeyCode::Char('2') | KeyCode::Char('i') => {
                state.room_input.clear();
                state.status = "paste room id or rmsg invite URI".into();
                state.mode = TuiMode::JoinInput;
            }
            KeyCode::Char('3') | KeyCode::Char('r') => {
                state.room_input.clear();
                state.status = "paste relay or peer multiaddr".into();
                state.mode = TuiMode::RelayInput;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !state.rooms.is_empty() {
                    let idx = state
                        .rooms
                        .iter()
                        .position(|r| Some(r) == state.selected_room.as_ref())
                        .unwrap_or(0);
                    let next = (idx + 1).min(state.rooms.len() - 1);
                    state.selected_room = Some(state.rooms[next].clone());
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !state.rooms.is_empty() {
                    let idx = state
                        .rooms
                        .iter()
                        .position(|r| Some(r) == state.selected_room.as_ref())
                        .unwrap_or(0);
                    let next = if idx == 0 { 0 } else { idx - 1 };
                    state.selected_room = Some(state.rooms[next].clone());
                }
            }
            KeyCode::Char('4') | KeyCode::Enter => {
                if let Some(ref rid) = state.selected_room {
                    let rid = rid.clone();
                    enter_room(state, &rid, cmd);
                }
            }
            _ => {}
        },
        TuiMode::JoinInput => match key.code {
            KeyCode::Esc => {
                state.room_input.clear();
                state.status = "● network ready".into();
                state.mode = TuiMode::Lobby;
            }
            KeyCode::Enter => {
                let input = state.room_input.trim().to_string();
                state.room_input.clear();
                if !input.is_empty() {
                    enter_room_or_invite(state, &input, cmd);
                }
            }
            KeyCode::Char(c) => {
                state.room_input.push(c);
            }
            KeyCode::Backspace => {
                state.room_input.pop();
            }
            _ => {}
        },
        TuiMode::RelayInput => match key.code {
            KeyCode::Esc => {
                state.room_input.clear();
                state.status = "● network ready".into();
                state.mode = TuiMode::Lobby;
            }
            KeyCode::Enter => {
                let addr = state.room_input.trim().to_string();
                state.room_input.clear();
                if !addr.is_empty() {
                    add_known_peer(state, cmd, addr.clone(), true);
                    state.status = format!("relay/peer added: {}", addr);
                }
                state.mode = TuiMode::Lobby;
            }
            KeyCode::Char(c) => {
                state.room_input.push(c);
            }
            KeyCode::Backspace => {
                state.room_input.pop();
            }
            _ => {}
        },
        TuiMode::Chat => match key.code {
            KeyCode::Esc => {
                state.mode = TuiMode::Lobby;
                state.session = None;
                state.current_room = None;
                state.messages.clear();
            }
            KeyCode::Enter => {
                let text = state.input.trim().to_string();
                if !text.is_empty() {
                    if let Some(ref mut session) = state.session {
                        if session.is_ready() {
                            if let Ok(encoded) = session.encrypt_and_encode(&text) {
                                let _ = cmd.send(TuiCmd::Send(
                                    state.current_room.clone().unwrap_or_default(),
                                    encoded.into_bytes(),
                                ));
                            }
                        }
                    }
                    state.messages.push(crate::session::ChatEvent {
                        id: uuid::Uuid::new_v4().to_string(),
                        from: state.name.clone(),
                        text,
                        ts: now_ms(),
                        mine: true,
                    });
                    state.input.clear();
                }
            }
            KeyCode::Char(c) => {
                state.input.push(c);
            }
            KeyCode::Backspace => {
                state.input.pop();
            }
            _ => {}
        },
        TuiMode::Quit => {}
    }
}

fn enter_room_or_invite(state: &mut TuiState, input: &str, cmd: &mpsc::UnboundedSender<TuiCmd>) {
    if let Some(invite) = Invite::from_uri(input) {
        for peer in invite.peers {
            add_known_peer(state, cmd, peer, true);
        }
        enter_room(state, &invite.room_id, cmd);
    } else {
        enter_room(state, input, cmd);
    }
}

fn enter_room(state: &mut TuiState, room_id: &str, cmd: &mpsc::UnboundedSender<TuiCmd>) {
    let kp = KeyPair::from_bytes(&state.kp.to_bytes());
    let run_id = uuid::Uuid::new_v4().to_string();
    let user_id = format!("{}-{}-{}", state.name, state.kp.short_id(), &run_id[..8]);
    let session = ChatSession::new(room_id.into(), user_id, state.store.clone(), kp);
    state.session = Some(session);
    state.messages.clear();
    state.current_room = Some(room_id.into());
    state.selected_room = Some(room_id.into());
    if !state.rooms.contains(&room_id.to_string()) {
        state.rooms.push(room_id.into());
    }
    let _ = cmd.send(TuiCmd::Join(room_id.into()));
    state.mode = TuiMode::Chat;
    update_invite(state);
    send_intro(state, cmd);
}

fn add_known_peer(
    state: &mut TuiState,
    cmd: &mpsc::UnboundedSender<TuiCmd>,
    addr: String,
    connect: bool,
) {
    if state.known_peers.insert(addr.clone()) {
        state.peers = state.known_peers.len();
        if connect {
            let _ = cmd.send(TuiCmd::Dial(addr.clone()));
            let _ = cmd.send(TuiCmd::Reserve(addr.clone()));
        }
        update_invite(state);
        state.status = format!("● {} known peer address(es)", state.known_peers.len());
    }
}

fn update_invite(state: &mut TuiState) {
    if let Some(room_id) = state.current_room.clone() {
        let mut peers: Vec<String> = state.known_peers.iter().cloned().collect();
        peers.sort();
        state.invite_uri = Invite::new(room_id, peers).to_uri().ok();
    }
}

fn send_intro(state: &mut TuiState, cmd: &mpsc::UnboundedSender<TuiCmd>) {
    if let Some(ref session) = state.session {
        let room_id = session.room_id().to_string();
        let hs = session.mk_handshake();
        let _ = cmd.send(TuiCmd::Send(room_id.clone(), hs.into_bytes()));
        let ws = session.build_want_since();
        let _ = cmd.send(TuiCmd::Send(room_id.clone(), ws.into_bytes()));
        if !state.known_peers.is_empty() {
            let mut peers: Vec<String> = state.known_peers.iter().cloned().collect();
            peers.sort();
            let px = session.build_peer_exchange(peers);
            let _ = cmd.send(TuiCmd::Send(room_id, px.into_bytes()));
        }
    }
}

fn draw_ui(f: &mut ratatui::Frame, state: &TuiState) {
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(f.area());

    if matches!(
        state.mode,
        TuiMode::Lobby | TuiMode::JoinInput | TuiMode::RelayInput
    ) {
        draw_lobby(f, main[0], state);
    } else {
        draw_chat(f, main[0], state);
    }

    let bottom = match state.mode {
        TuiMode::Chat => format!("> {}", state.input),
        TuiMode::JoinInput => format!("Join room/invite: {}", state.room_input),
        TuiMode::RelayInput => format!("Relay/peer address: {}", state.room_input),
        _ => state.status.clone(),
    };
    let input = Paragraph::new(bottom)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(input, main[1]);
}

fn draw_lobby(f: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let room_items: Vec<ListItem> = state
        .rooms
        .iter()
        .map(|r| {
            let selected = state.selected_room.as_ref() == Some(r);
            let prefix = if selected { "► " } else { "  " };
            ListItem::new(format!("{}{}", prefix, r))
        })
        .collect();

    let room_list = List::new(room_items)
        .block(Block::default().borders(Borders::ALL).title("Rooms"))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(room_list, chunks[0]);

    let help = Paragraph::new(vec![
        Line::from(Span::styled("● rmsg", Style::default().fg(Color::Cyan))),
        Line::from(""),
        Line::from("1  create invite room"),
        Line::from("2  join room/invite"),
        Line::from("3  add relay/peer address"),
        Line::from("4  open selected room"),
        Line::from("j/k  move room"),
        Line::from("q  quit"),
        Line::from(""),
        Line::from(format!("Peers: {}", state.peers)),
        Line::from(""),
        Line::from(state.status.clone()),
    ])
    .block(Block::default().borders(Borders::ALL).title("Info"));
    f.render_widget(help, chunks[1]);
}

fn draw_chat(f: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(area);

    let room_items: Vec<ListItem> = state
        .rooms
        .iter()
        .map(|r| {
            let active = state.current_room.as_ref() == Some(r);
            let prefix = if active { "● " } else { "  " };
            ListItem::new(format!("{}{}", prefix, r))
        })
        .collect();
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    let room_list =
        List::new(room_items).block(Block::default().borders(Borders::ALL).title("Rooms"));
    f.render_widget(room_list, left[0]);

    let invite = Paragraph::new(
        state
            .invite_uri
            .as_deref()
            .unwrap_or("waiting for listen address"),
    )
    .block(Block::default().borders(Borders::ALL).title("Invite"))
    .wrap(Wrap { trim: false });
    f.render_widget(invite, left[1]);

    let header = format!(
        " {} — esc to leave",
        state.current_room.as_deref().unwrap_or("?")
    );
    let msg_lines: Vec<Line> = state
        .messages
        .iter()
        .map(|m| {
            if m.mine {
                Line::from(Span::styled(
                    format!("  > {}", m.text),
                    Style::default().fg(Color::Cyan),
                ))
            } else if m.from == "system" {
                Line::from(Span::styled(
                    format!("  -- {} --", m.text),
                    Style::default().fg(Color::DarkGray),
                ))
            } else {
                Line::from(Span::styled(
                    format!("  [{}] {}", m.from, m.text),
                    Style::default().fg(Color::White),
                ))
            }
        })
        .collect();
    let msgs = Paragraph::new(msg_lines)
        .block(Block::default().borders(Borders::ALL).title(header))
        .wrap(Wrap { trim: false });
    f.render_widget(msgs, chunks[1]);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
