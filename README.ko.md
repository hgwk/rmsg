[English](README.md) | 한국어

# relay

서버리스 P2P 암호화 채팅 — 서버 0, 로그 0, libp2p + Kademlia DHT 기반 E2E 암호화.

## 하는 일

`relay`는 서버 없이 P2P로 채팅합니다. 모든 메시지는 E2E 암호화(X25519 + AES-256-GCM)되고 libp2p GossipSub로 전달됩니다. 피어 발견은 Kademlia DHT를 사용하며 중앙 서버, 가입, 전화번호가 전혀 필요하지 않습니다.

모든 메시지는 사용자 디바이스에만 저장됩니다(SQLite). 어떤 서버도 평문을 볼 수 없습니다.

## 설치

### Homebrew (macOS/Linux)

```bash
brew install hgwk/tap/relay
```

### Cargo

```bash
cargo install relay-chat
```

### npm (cduo 방식)

```bash
npm install -g @hgwk/relay
```

### 소스에서 빌드

```bash
git clone https://github.com/hgwk/relay.git
cd relay/relay-rs
cargo build --release
```

## 빠른 시작

### CLI 모드

```bash
# 방 만들기
relay create
# → Room created: room-a1b2c3d4

# 다른 기기에서 참여
relay join room-a1b2c3d4

# 메시지 입력. /quit 으로 종료.
```

### GUI 모드

```bash
# 인자 없이 실행
relay
```

## 명령어

| 명령어 | 설명 |
|---------|-------------|
| `relay` | GUI 실행 |
| `relay create` | 새 방 만들고 채팅 시작 |
| `relay join <room-id>` | 기존 방 참여 |
| `relay list` | 로컬 DB에서 방 목록 조회 |
| `relay --help` | 도움말 |
| `relay --version` | 버전 |

## 보안

| 레이어 | 알고리즘 |
|-------|-----------|
| 키 교환 | X25519 (타원곡선 Diffie-Hellman) |
| 키 유도 | HKDF-SHA256 |
| 메시지 암호화 | AES-256-GCM (인증 암호화) |
| 전송 | libp2p Noise + Yamux |
| 재전송 방지 | 시퀀스 번호 + 방별 AAD |

메시지는 어떤 서버에도 저장되지 않습니다. 오프라인 전달을 위해 암호화된 메시지가 다른 피어의 디바이스에 일시적으로 보관될 수 있지만, 해당 피어는 복호화할 수 없습니다.

## 아키텍처

```
┌───────────┐    GossipSub     ┌───────────┐
│  Alice     │◄───────────────▶│   Bob      │
│  (relay)   │  (암호화됨)      │  (relay)   │
└─────┬─────┘                  └─────┬─────┘
      │                              │
      │    Kademlia DHT              │
      │  (피어 발견)                  │
      └──────────┬───────────────────┘
                 │
          ┌──────▼──────┐
          │  Bootstrap   │  (최초 연결시만)
          │  Nodes       │
          └─────────────┘
```

## 요구사항

- Rust 1.75+
- 네트워크 연결 (NAT 통과를 위한 STUN)

## 지원 플랫폼

- macOS (arm64, x86_64)
- Linux (x86_64, aarch64)
- Windows (계획됨)

## 라이선스

MIT
