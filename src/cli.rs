use clap::{Parser, Subcommand};
use libp2p::Multiaddr;
use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::crypto::KeyPair;
use crate::p2p::{P2PEvent, P2PNode};
use crate::protocol::{ControlMessage, Invite, WireMessage};
use crate::session::ChatSession;
use crate::storage::MessageStore;

#[derive(Parser)]
#[command(name = "rmsg", version, about = "Serverless P2P encrypted chat")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[arg(short, long, default_value = "anon")]
    pub name: String,
    #[arg(long = "relay")]
    pub relays: Vec<String>,
}

#[derive(Subcommand)]
pub enum Command {
    Create,
    Invite {
        #[command(subcommand)]
        command: InviteCommand,
    },
    Join {
        room: String,
    },
    Relay {
        #[arg(long, default_value = "/ip4/0.0.0.0/tcp/4001")]
        listen: String,
    },
    List,
    Discover,
    Tui,
}

#[derive(Subcommand)]
pub enum InviteCommand {
    Create,
}

enum CliCmd {
    Send(String, Vec<u8>),
    Dial(String),
    Reserve(String),
}

pub fn run_cli(cli: Cli, store: Arc<MessageStore>, kp: Arc<KeyPair>) {
    match cli.command.unwrap_or(Command::List) {
        Command::Create => {
            let room_id = format!("room-{}", &uuid::Uuid::new_v4().to_string()[..8]);
            println!("Room created: {}", room_id);
            run_chat(room_id, cli.relays, store, cli.name, kp, true);
        }
        Command::Invite {
            command: InviteCommand::Create,
        } => {
            let room_id = format!("room-{}", &uuid::Uuid::new_v4().to_string()[..8]);
            println!("Room created: {}", room_id);
            run_chat(room_id, cli.relays, store, cli.name, kp, true);
        }
        Command::Join { room } => {
            let (room_id, peers) = parse_room_arg(&room);
            let mut initial_peers = cli.relays;
            initial_peers.extend(peers);
            run_chat(room_id, initial_peers, store, cli.name, kp, false);
        }
        Command::Relay { listen } => {
            run_relay(listen);
        }
        Command::List => match store.list_rooms() {
            Ok(rooms) => {
                if rooms.is_empty() {
                    println!("No rooms.");
                } else {
                    for r in rooms {
                        println!("  {}", r);
                    }
                }
            }
            Err(e) => eprintln!("Error: {}", e),
        },
        Command::Discover => run_discover(store, cli.name, kp),
        Command::Tui => {
            crate::tui::run_tui(store, kp, cli.name);
        }
    }
}

fn run_chat(
    room_id: String,
    initial_peers: Vec<String>,
    store: Arc<MessageStore>,
    name: String,
    kp: Arc<KeyPair>,
    print_invite: bool,
) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (stdin_tx, stdin_rx) = std_mpsc::channel();

    let rid = room_id.clone();
    let initial_known_peers = initial_peers.clone();
    std::thread::spawn(move || {
        rt.block_on(async {
            let mut node = match P2PNode::new(event_tx, None) { Ok(n) => n, Err(e) => { eprintln!("P2P: {}", e); return; } };
            for peer in initial_peers {
                let _ = node.dial_addr(&peer);
                let _ = node.reserve_relay(&peer);
            }
            let _ = node.join_room(&rid);
            let mut cmd_rx = cmd_rx;
            loop {
                tokio::select! {
                    _ = node.run() => {},
                    cmd = cmd_rx.recv() => {
                            match cmd {
                                Some(CliCmd::Send(rid, data)) => { let _ = node.send_message(&rid, data); }
                                Some(CliCmd::Dial(addr)) => { let _ = node.dial_addr(&addr); }
                                Some(CliCmd::Reserve(addr)) => { let _ = node.reserve_relay(&addr); }
                                None => break,
                            }
                    }
                }
            }
        });
    });

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    let _room = room_id.clone();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let text = line.unwrap_or_default().trim().to_string();
            if text.is_empty() {
                continue;
            }
            if text == "/quit" {
                shutdown_clone.store(true, Ordering::SeqCst);
                break;
            }
            let _ = stdin_tx.send(text);
        }
    });

    let local_kp = KeyPair::from_bytes(&kp.to_bytes());
    let run_id = &uuid::Uuid::new_v4().to_string()[..4];
    let user_id = format!("{}-{}-{}", name, local_kp.short_id(), run_id);
    let mut session = ChatSession::new(room_id.clone(), user_id.clone(), store, local_kp);
    let mut known_peers: HashSet<String> = initial_known_peers.into_iter().collect();
    send_session_intro(&cmd_tx, &room_id, &session, &known_peers);
    let mut last_intro = std::time::Instant::now();
    let mut last_invite: Option<String> = None;

    println!("Chat started as {}. /quit to exit.", user_id);
    print!("> ");
    io::stdout().flush().ok();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            println!("\nGoodbye.");
            break;
        }

        if let Ok(text) = stdin_rx.try_recv() {
            if session.is_ready() {
                if let Ok(encoded) = session.encrypt_and_encode(&text) {
                    let _ = cmd_tx.send(CliCmd::Send(room_id.clone(), encoded.into_bytes()));
                }
                println!("\r[me] {}", text);
            } else {
                println!("\r* Waiting for peer encryption; not sent: {}", text);
            }
            print!("> ");
            io::stdout().flush().ok();
        }

        if let Ok(event) = event_rx.try_recv() {
            match event {
                P2PEvent::Message { data, .. } => {
                    if let Ok(WireMessage::Control(ControlMessage::PeerExchange {
                        from,
                        peers,
                        ..
                    })) = serde_json::from_slice::<WireMessage>(&data)
                    {
                        if from != user_id {
                            for peer in peers {
                                if known_peers.insert(peer.clone()) {
                                    let _ = cmd_tx.send(CliCmd::Dial(peer.clone()));
                                    let _ = cmd_tx.send(CliCmd::Reserve(peer));
                                }
                            }
                        }
                    }
                    if let Ok(WireMessage::Control(ControlMessage::WantSince {
                        from, since, ..
                    })) = serde_json::from_slice::<WireMessage>(&data)
                    {
                        if from != user_id && session.is_ready() {
                            let history = session.build_history_chunk(since);
                            let _ =
                                cmd_tx.send(CliCmd::Send(room_id.clone(), history.into_bytes()));
                        }
                    }
                    for ev in session.handle_raw(&data) {
                        println!("\r[{}] {}", ev.from, ev.text);
                    }
                    print!("> ");
                    io::stdout().flush().ok();
                }
                P2PEvent::PeerDiscovered(peer) => {
                    println!("\r* Peer: {}", peer);
                    send_session_intro(&cmd_tx, &room_id, &session, &known_peers);
                    last_intro = std::time::Instant::now();
                    print!("> ");
                    io::stdout().flush().ok();
                }
                P2PEvent::PeerAddress(addr) => {
                    if known_peers.insert(addr.clone()) {
                        let _ = cmd_tx.send(CliCmd::Dial(addr.clone()));
                        let _ = cmd_tx.send(CliCmd::Reserve(addr));
                    }
                }
                P2PEvent::Listening(addr) => {
                    let changed = known_peers.insert(addr);
                    if changed && print_invite && !known_peers.is_empty() {
                        let peers = known_peers.iter().cloned().collect::<Vec<_>>();
                        if let Ok(uri) = Invite::new(room_id.clone(), peers).to_uri() {
                            if last_invite.as_ref() != Some(&uri) {
                                println!("\rInvite: {}", uri);
                                print!("> ");
                                io::stdout().flush().ok();
                                last_invite = Some(uri);
                            }
                        }
                    }
                }
                P2PEvent::Subscribed => {
                    send_session_intro(&cmd_tx, &room_id, &session, &known_peers);
                    last_intro = std::time::Instant::now();
                    print!("> ");
                    io::stdout().flush().ok();
                }
                _ => {}
            }
        }

        if !session.is_ready() && last_intro.elapsed() >= std::time::Duration::from_secs(2) {
            send_session_intro(&cmd_tx, &room_id, &session, &known_peers);
            last_intro = std::time::Instant::now();
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn send_session_intro(
    cmd_tx: &mpsc::UnboundedSender<CliCmd>,
    room_id: &str,
    session: &ChatSession,
    known_peers: &HashSet<String>,
) {
    let _ = cmd_tx.send(CliCmd::Send(
        room_id.to_string(),
        session.mk_handshake().into_bytes(),
    ));
    let _ = cmd_tx.send(CliCmd::Send(
        room_id.to_string(),
        session.build_want_since().into_bytes(),
    ));
    if !known_peers.is_empty() {
        let peers = known_peers.iter().cloned().collect::<Vec<_>>();
        let exchange = session.build_peer_exchange(peers);
        let _ = cmd_tx.send(CliCmd::Send(room_id.to_string(), exchange.into_bytes()));
    }
}

fn parse_room_arg(input: &str) -> (String, Vec<String>) {
    if let Some(invite) = Invite::from_uri(input) {
        return (invite.room_id, invite.peers);
    }
    (input.to_string(), vec![])
}

fn run_relay(listen: String) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let listen_addr = listen.parse::<Multiaddr>().ok();

    std::thread::spawn(move || {
        rt.block_on(async {
            let mut node = match P2PNode::new(event_tx, listen_addr) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("P2P: {}", e);
                    return;
                }
            };
            node.run().await;
        });
    });

    println!(
        "Relay node starting on {}. Share the printed /p2p address.",
        listen
    );
    loop {
        while let Ok(event) = event_rx.try_recv() {
            match event {
                P2PEvent::Listening(addr) => println!("Relay address: {}", addr),
                P2PEvent::PeerDiscovered(peer) => println!("Peer: {}", peer),
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn run_discover(store: Arc<MessageStore>, name: String, kp: Arc<KeyPair>) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();

    std::thread::spawn(move || {
        rt.block_on(async {
            let mut node = match P2PNode::new(event_tx, None) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("P2P: {}", e);
                    return;
                }
            };
            node.discover_rooms();
            loop {
                tokio::select! {
                    _ = node.run() => {},
                }
            }
        });
    });

    println!("Discovering rooms... (waiting for peers)");

    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > std::time::Duration::from_secs(15) {
            println!("No rooms found. Try creating one with: rmsg create");
            break;
        }
        while let Ok(event) = event_rx.try_recv() {
            if let P2PEvent::RoomsDiscovered(rooms) = event {
                if rooms.is_empty() {
                    println!("No rooms on the network yet.");
                } else {
                    println!("Rooms on the network:");
                    for r in &rooms {
                        println!("  {}  (join: rmsg join {})", r, r);
                    }
                }
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = (store, name, kp);
}
