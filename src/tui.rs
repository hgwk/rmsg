use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::crypto::KeyPair;
use crate::p2p::{P2PEvent, P2PNode};
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
            let mut node = P2PNode::new(event_tx).unwrap();
            let mut cmd_rx = cmd_rx;
            loop {
                tokio::select! {
                    _ = node.run() => {},
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(TuiCmd::Join(rid)) => { node.join_room(&rid).ok(); }
                            Some(TuiCmd::Send(rid, data)) => { node.send_message(&rid, data).ok(); }
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
        mode: TuiMode::Lobby,
    };

    let key_rx = key_rx;
    loop {
        while let Ok(event) = event_rx.try_recv() {
            match event {
                P2PEvent::Message { data, .. } => {
                    if let Some(ref mut session) = state.session {
                        for ev in session.handle_raw(&data) {
                            state.messages.push(ev);
                            if state.messages.len() > 500 { state.messages.remove(0); }
                        }
                    }
                }
                P2PEvent::PeerDiscovered(_) => { state.peers += 1; state.status = format!("● {} peer(s)", state.peers); }
                P2PEvent::Subscribed => {
                    if let Some(ref session) = state.session {
                        let hs = session.mk_handshake();
                        let _ = cmd_tx.send(TuiCmd::Send(session.room_id().into(), hs.into_bytes()));
                        let ws = session.build_want_since();
                        let _ = cmd_tx.send(TuiCmd::Send(session.room_id().into(), ws.into_bytes()));
                    }
                }
                _ => {}
            }
        }

        while let Ok(key) = key_rx.try_recv() {
            handle_key(&mut state, key, &cmd_tx);
        }

        terminal.draw(|f| draw_ui(f, &state)).unwrap();

        if state.mode == TuiMode::Quit { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
}

#[derive(PartialEq)]
enum TuiMode { Lobby, Chat, Quit }

enum TuiCmd { Join(String), Send(String, Vec<u8>) }

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
    mode: TuiMode,
}

fn handle_key(state: &mut TuiState, key: event::KeyEvent, cmd: &mpsc::UnboundedSender<TuiCmd>) {
    match state.mode {
        TuiMode::Lobby => match key.code {
            KeyCode::Char('q') => state.mode = TuiMode::Quit,
            KeyCode::Char('n') => {
                let room_id = format!("room-{}", &uuid::Uuid::new_v4().to_string()[..8]);
                enter_room(state, &room_id, cmd);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !state.rooms.is_empty() {
                    let idx = state.rooms.iter().position(|r| Some(r) == state.selected_room.as_ref()).unwrap_or(0);
                    let next = (idx + 1).min(state.rooms.len() - 1);
                    state.selected_room = Some(state.rooms[next].clone());
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !state.rooms.is_empty() {
                    let idx = state.rooms.iter().position(|r| Some(r) == state.selected_room.as_ref()).unwrap_or(0);
                    let next = if idx == 0 { 0 } else { idx - 1 };
                    state.selected_room = Some(state.rooms[next].clone());
                }
            }
            KeyCode::Enter => {
                if let Some(ref rid) = state.selected_room {
                    let rid = rid.clone();
                    enter_room(state, &rid, cmd);
                }
            }
            KeyCode::Char(c) => { state.room_input.push(c); }
            KeyCode::Backspace => { state.room_input.pop(); }
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
                                let _ = cmd.send(TuiCmd::Send(state.current_room.clone().unwrap_or_default(), encoded.into_bytes()));
                            }
                        }
                    }
                    state.messages.push(crate::session::ChatEvent {
                        id: uuid::Uuid::new_v4().to_string(), from: state.name.clone(),
                        text, ts: now_ms(), mine: true,
                    });
                    state.input.clear();
                }
            }
            KeyCode::Char(c) => { state.input.push(c); }
            KeyCode::Backspace => { state.input.pop(); }
            _ => {}
        },
        TuiMode::Quit => {}
    }
}

fn enter_room(state: &mut TuiState, room_id: &str, cmd: &mpsc::UnboundedSender<TuiCmd>) {
    let kp = KeyPair::from_bytes(&state.kp.to_bytes());
    let session = ChatSession::new(room_id.into(), state.name.clone(), state.store.clone(), kp);
    state.session = Some(session);
    state.messages.clear();
    state.current_room = Some(room_id.into());
    state.selected_room = Some(room_id.into());
    if !state.rooms.contains(&room_id.to_string()) { state.rooms.push(room_id.into()); }
    let _ = cmd.send(TuiCmd::Join(room_id.into()));
    state.mode = TuiMode::Chat;
}

fn draw_ui(f: &mut ratatui::Frame, state: &TuiState) {
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(f.area());

    if state.mode == TuiMode::Lobby {
        draw_lobby(f, main[0], state);
    } else {
        draw_chat(f, main[0], state);
    }

    let status = Paragraph::new(state.status.as_str())
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(status, main[1]);

    if state.mode == TuiMode::Chat {
        let input = Paragraph::new(format!("> {}", state.input))
            .block(Block::default().borders(Borders::TOP));
        f.render_widget(input, main[1]);
    }
}

fn draw_lobby(f: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let room_items: Vec<ListItem> = state.rooms.iter().map(|r| {
        let selected = state.selected_room.as_ref() == Some(r);
        let prefix = if selected { "► " } else { "  " };
        ListItem::new(format!("{}{}", prefix, r))
    }).collect();

    let room_list = List::new(room_items)
        .block(Block::default().borders(Borders::ALL).title("Rooms"))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(room_list, chunks[0]);

    let help = Paragraph::new(vec![
        Line::from(Span::styled("● rmsg", Style::default().fg(Color::Cyan))),
        Line::from(""),
        Line::from("n  new room"),
        Line::from("j/k  navigate"),
        Line::from("enter  join"),
        Line::from("q  quit"),
        Line::from(""),
        Line::from(format!("Peers: {}", state.peers)),
    ]).block(Block::default().borders(Borders::ALL).title("Info"));
    f.render_widget(help, chunks[1]);
}

fn draw_chat(f: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(area);

    let room_items: Vec<ListItem> = state.rooms.iter().map(|r| {
        let active = state.current_room.as_ref() == Some(r);
        let prefix = if active { "● " } else { "  " };
        ListItem::new(format!("{}{}", prefix, r))
    }).collect();
    let room_list = List::new(room_items)
        .block(Block::default().borders(Borders::ALL).title("Rooms"));
    f.render_widget(room_list, chunks[0]);

    let header = format!(" {} — esc to leave",
        state.current_room.as_deref().unwrap_or("?"));
    let msg_lines: Vec<Line> = state.messages.iter().map(|m| {
        if m.mine {
            Line::from(Span::styled(format!("  > {}", m.text), Style::default().fg(Color::Cyan)))
        } else if m.from == "system" {
            Line::from(Span::styled(format!("  -- {} --", m.text), Style::default().fg(Color::DarkGray)))
        } else {
            Line::from(Span::styled(format!("  [{}] {}", m.from, m.text), Style::default().fg(Color::White)))
        }
    }).collect();
    let msgs = Paragraph::new(msg_lines)
        .block(Block::default().borders(Borders::ALL).title(header))
        .wrap(Wrap { trim: false });
    f.render_widget(msgs, chunks[1]);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}
