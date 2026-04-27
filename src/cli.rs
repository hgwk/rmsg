use clap::{Parser, Subcommand};
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc;

use crate::crypto::KeyPair;
use crate::p2p::{P2PEvent, P2PNode};
use crate::session::ChatSession;
use crate::storage::MessageStore;

#[derive(Parser)]
#[command(name = "rmsg", version, about = "Serverless P2P encrypted chat")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[arg(short, long, default_value = "anon")]
    pub name: String,
}

#[derive(Subcommand)]
pub enum Command {
    Create,
    Join { room_id: String },
    List,
    Tui,
}

enum CliCmd {
    Send(String, Vec<u8>),
}

pub fn run_cli(cli: Cli, store: Arc<MessageStore>, kp: Arc<KeyPair>) {
    match cli.command.unwrap_or(Command::List) {
        Command::Create => {
            let room_id = format!("room-{}", &uuid::Uuid::new_v4().to_string()[..8]);
            println!("Room created: {}", room_id);
            run_chat(room_id, store, cli.name, kp);
        }
        Command::Join { room_id } => run_chat(room_id, store, cli.name, kp),
        Command::List => {
            match store.list_rooms() {
                Ok(rooms) => {
                    if rooms.is_empty() { println!("No rooms."); }
                    else { for r in rooms { println!("  {}", r); } }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        Command::Tui => {
            crate::tui::run_tui(store, kp, cli.name);
        }
    }
}

fn run_chat(room_id: String, store: Arc<MessageStore>, name: String, kp: Arc<KeyPair>) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (stdin_tx, stdin_rx) = std_mpsc::channel();

    let rid = room_id.clone();
    std::thread::spawn(move || {
        rt.block_on(async {
            let mut node = match P2PNode::new(event_tx) { Ok(n) => n, Err(e) => { eprintln!("P2P: {}", e); return; } };
            let _ = node.join_room(&rid);
            let mut cmd_rx = cmd_rx;
            loop {
                tokio::select! {
                    _ = node.run() => {},
                    cmd = cmd_rx.recv() => {
                            match cmd {
                                Some(CliCmd::Send(rid, data)) => { let _ = node.send_message(&rid, data); }
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
            if text.is_empty() { continue; }
            if text == "/quit" { shutdown_clone.store(true, Ordering::SeqCst); break; }
            let _ = stdin_tx.send(text);
        }
    });

    let local_kp = KeyPair::from_bytes(&kp.to_bytes());
    let mut session = ChatSession::new(room_id.clone(), name, store, local_kp);
    let hs = session.mk_handshake();
    let _ = cmd_tx.send(CliCmd::Send(room_id.clone(), hs.into_bytes()));
    let ws = session.build_want_since();
    let _ = cmd_tx.send(CliCmd::Send(room_id.clone(), ws.into_bytes()));

    println!("Chat started. /quit to exit.");
    print!("> "); io::stdout().flush().ok();

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
            }
            println!("\r[me] {}", text);
            print!("> "); io::stdout().flush().ok();
        }

        if let Ok(event) = event_rx.try_recv() {
            match event {
                P2PEvent::Message { data, .. } => {
                    for ev in session.handle_raw(&data) {
                        println!("\r[{}] {}", ev.from, ev.text);
                    }
                    print!("> "); io::stdout().flush().ok();
                }
                P2PEvent::PeerDiscovered(peer) => {
                    println!("\r* Peer: {}", peer);
                    print!("> "); io::stdout().flush().ok();
                }
                _ => {}
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
