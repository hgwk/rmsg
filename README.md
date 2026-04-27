# rmsg

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Serverless P2P encrypted chat — zero servers, zero logs, E2E encryption via libp2p + Kademlia DHT.

## What It Does

`rmsg` lets you chat peer-to-peer without any server. Messages are encrypted end-to-end (X25519 + AES-256-GCM) and delivered via libp2p GossipSub. Peer discovery uses Kademlia DHT — no central server, no registration, no phone number.

All messages are stored only on your device (SQLite). Nothing touches a server in plaintext.

## Install

### Homebrew (macOS/Linux)

```bash
brew install hgwk/tap/rmsg
```

### Cargo

```bash
cargo install rmsg
```

### npm (like cduo)

```bash
npm install -g @hgwk/rmsg
```

### From source

```bash
git clone https://github.com/hgwk/rmsg.git
cd rmsg
cargo build --release
```

## Quick Start

### CLI mode

```bash
# Create a room
rmsg create
# → Room created: room-a1b2c3d4

# On another machine, join
rmsg join room-a1b2c3d4

# Type messages. /quit to exit.
```

### GUI mode

```bash
# Launch without arguments
rmsg
```

### TUI mode

```bash
# Terminal UI with ratatui
rmsg tui
```

## Commands

| Command | Description |
|---------|-------------|
| `rmsg` | Launch GUI |
| `rmsg tui` | Launch TUI (ratatui) |
| `rmsg create` | Create a new room and start chatting |
| `rmsg join <room-id>` | Join an existing room |
| `rmsg list` | List known rooms from local database |
| `rmsg --help` | Show help |

## Security

| Layer | Algorithm |
|-------|-----------|
| Key exchange | X25519 (Elliptic-curve Diffie-Hellman) |
| Key derivation | HKDF-SHA256 |
| Message encryption | AES-256-GCM (authenticated) |
| Transport | libp2p Noise + Yamux |
| Replay protection | Sequence number + room-scoped AAD |

Messages are never stored on any server. Encrypted messages may be held temporarily in Kademlia DHT by other peers' devices for offline delivery — these peers cannot decrypt them.

## Architecture

```
┌───────────┐    GossipSub     ┌───────────┐
│  Alice     │◄───────────────▶│   Bob      │
│  (rmsg)    │  (encrypted)    │  (rmsg)    │
└─────┬─────┘                  └─────┬─────┘
      │                              │
      │    Kademlia DHT              │
      │  (peer discovery)            │
      └──────────┬───────────────────┘
                 │
          ┌──────▼──────┐
          │  Bootstrap   │  (first connect only)
          │  Nodes       │
          └─────────────┘
```

## Requirements

- Rust 1.75+
- Network connection (STUN for NAT traversal)

## Supported Platforms

- macOS (arm64, x86_64)
- Linux (x86_64, aarch64)
- Windows (planned)

## License

MIT
