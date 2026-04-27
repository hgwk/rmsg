# rmsg

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Serverless P2P encrypted chat — zero servers, zero logs, E2E encryption via libp2p + Kademlia DHT.

## What It Does

`rmsg` lets you chat peer-to-peer without any server. Messages are encrypted end-to-end (X25519 + AES-256-GCM) and delivered via libp2p GossipSub. Peer discovery uses Kademlia DHT — no central server, no registration, no phone number.

All messages are stored only on your device (SQLite). Nothing touches a server in plaintext.

## Install

### From source

```bash
git clone https://github.com/hgwk/rmsg.git
cd rmsg
cargo build --release
```

### Coming soon

```bash
# npm (like cduo)
npm install -g @hgwk/rmsg

# Cargo
cargo install rmsg

# Homebrew
brew install hgwk/tap/rmsg
```

## Quick Start

### CLI mode

```bash
# Create a room and print an invite URI
rmsg invite create
# → Invite: rmsg://v1/...

# On another machine, join
rmsg join 'rmsg://v1/...'

# Type messages. /quit to exit.
```

### Internet / NAT traversal

Any reachable user PC can act as a libp2p circuit relay:

```bash
rmsg relay --listen /ip4/0.0.0.0/tcp/4001
# Share the printed /p2p address.
```

Private peers can create or join rooms through that relay:

```bash
rmsg --relay /ip4/<host>/tcp/4001/p2p/<relay-peer-id> invite create
rmsg --relay /ip4/<host>/tcp/4001/p2p/<relay-peer-id> join 'rmsg://v1/...'
```

Each `rmsg` node also runs a relay server, exchanges known peer addresses, attempts direct dials,
reserves relay circuits on known relay-capable peers, and rebroadcasts encrypted mesh envelopes
with TTL-based loop prevention.

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

The TUI opens a menu-driven lobby:

- `1` create an invite room
- `2` join a room ID or `rmsg://v1/...` invite
- `3` add a relay or peer multiaddr
- `4` open the selected local room

Inside a room, the left pane shows the current invite URI. Share that URI with another peer.

## Commands

| Command | Description |
|---------|-------------|
| `rmsg` | Launch GUI |
| `rmsg tui` | Launch TUI (ratatui) |
| `rmsg create` | Create a new room, print an invite URI, and start chatting |
| `rmsg invite create` | Create a new room and print an invite URI |
| `rmsg join <room-id-or-invite>` | Join an existing room |
| `rmsg relay --listen <multiaddr>` | Run a reachable relay node |
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
