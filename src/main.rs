mod crypto;
mod storage;
mod protocol;
mod p2p;
mod session;
mod app;
mod cli;
mod tui;

use std::path::PathBuf;
use std::sync::Arc;
use clap::Parser;

use app::RelayApp;
use cli::Cli;
use crypto::KeyPair;
use storage::MessageStore;

fn main() -> Result<(), eframe::Error> {
    env_logger::init();

    let cli = Cli::parse();

    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("relay");
    std::fs::create_dir_all(&data_dir).ok();
    let db_path = data_dir.join("messages.db");
    let key_path = data_dir.join("identity.key");
    let store = Arc::new(MessageStore::open(&db_path).expect("Failed to open database"));
    let kp = Arc::new(KeyPair::load_or_generate(&key_path).expect("Failed to load key"));

    if cli.command.is_some() || std::env::args().len() > 1 {
        cli::run_cli(cli, store, kp);
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "relay — serverless P2P chat",
        options,
        Box::new(|_cc| Ok(Box::new(RelayApp::new(store, kp)))),
    )
}
