use futures::StreamExt;
use libp2p::{
    core::upgrade,
    dcutr, gossipsub, identify, identity, kad, mdns,
    multiaddr::Protocol,
    noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm, Transport,
};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::protocol::MeshEnvelope;

const BOOTSTRAP_NODES: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
];

const ROOMS_DHT_KEY: &str = "rmsg-active-rooms";

pub enum P2PEvent {
    Message {
        data: Vec<u8>,
    },
    PeerDiscovered(PeerId),
    PeerAddress(String),
    Listening(String),
    Subscribed,
    RoomsDiscovered(Vec<String>),
    #[allow(dead_code)]
    Error(String),
}

#[derive(NetworkBehaviour)]
struct RelayBehaviour {
    gossipsub: gossipsub::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    dcutr: dcutr::Behaviour,
    mdns: mdns::tokio::Behaviour,
    relay_client: relay::client::Behaviour,
    relay_server: relay::Behaviour,
}

pub struct P2PNode {
    swarm: Swarm<RelayBehaviour>,
    topic_peers: HashSet<PeerId>,
    event_tx: mpsc::UnboundedSender<P2PEvent>,
    rooms: HashSet<String>,
    seen_mesh: HashSet<String>,
    local_peer_id: PeerId,
    known_addrs: HashSet<String>,
    known_peers: HashSet<PeerId>,
}

impl P2PNode {
    pub fn new(
        event_tx: mpsc::UnboundedSender<P2PEvent>,
        listen_addr: Option<Multiaddr>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let local_key = identity::Keypair::generate_ed25519();
        let peer_id = PeerId::from(local_key.public());
        log::info!("Peer ID: {}", peer_id);

        let (relay_transport, relay_client) = relay::client::new(peer_id);
        let tcp_transport = tcp::tokio::Transport::default();
        let transport = relay_transport
            .or_transport(tcp_transport)
            .upgrade(upgrade::Version::V1)
            .authenticate(noise::Config::new(&local_key)?)
            .multiplex(yamux::Config::default())
            .boxed();

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(1))
            .validation_mode(gossipsub::ValidationMode::Strict)
            .build()?;

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        )?;

        let kademlia = kad::Behaviour::new(peer_id, kad::store::MemoryStore::new(peer_id));

        let identify = identify::Behaviour::new(
            identify::Config::new("rmsg/0.1.0".into(), local_key.public())
                .with_agent_version(format!("rmsg-p2p/{}", env!("CARGO_PKG_VERSION"))),
        );

        let ping =
            ping::Behaviour::new(ping::Config::default().with_interval(Duration::from_secs(30)));

        let dcutr = dcutr::Behaviour::new(peer_id);
        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)?;
        let relay_server = relay::Behaviour::new(peer_id, relay::Config::default());

        let behaviour = RelayBehaviour {
            gossipsub,
            kademlia,
            identify,
            ping,
            dcutr,
            mdns,
            relay_client,
            relay_server,
        };

        let swarm_config = libp2p::swarm::Config::with_tokio_executor()
            .with_idle_connection_timeout(Duration::from_secs(60));

        let mut swarm = Swarm::new(transport, behaviour, peer_id, swarm_config);

        swarm.listen_on(listen_addr.unwrap_or("/ip4/0.0.0.0/tcp/0".parse::<Multiaddr>()?))?;

        for addr_str in BOOTSTRAP_NODES {
            if let Ok(multiaddr) = addr_str.parse::<Multiaddr>() {
                if let Err(e) = swarm.dial(multiaddr) {
                    log::warn!("Failed to dial bootstrap: {}", e);
                }
            }
        }

        Ok(Self {
            swarm,
            topic_peers: HashSet::new(),
            event_tx,
            rooms: HashSet::new(),
            seen_mesh: HashSet::new(),
            local_peer_id: peer_id,
            known_addrs: HashSet::new(),
            known_peers: HashSet::new(),
        })
    }

    pub fn join_room(&mut self, room_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.rooms.insert(room_id.to_string());
        let topic = gossipsub::IdentTopic::new(format!("rmsg-room-{}", room_id));
        self.swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
        self.advertise_room();
        Ok(())
    }

    fn advertise_room(&mut self) {
        let key = kad::RecordKey::new(&ROOMS_DHT_KEY);
        let rooms_json =
            serde_json::to_string(&self.rooms.iter().collect::<Vec<_>>()).unwrap_or_default();
        let record = kad::Record::new(key, rooms_json.into_bytes());
        log::info!("Advertising {} rooms to DHT", self.rooms.len());
        if let Err(e) = self
            .swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, kad::Quorum::One)
        {
            log::warn!("Failed to advertise rooms: {:?}", e);
        }
    }

    pub fn discover_rooms(&mut self) {
        let key = kad::RecordKey::new(&ROOMS_DHT_KEY);
        log::info!("Discovering rooms from DHT");
        self.swarm.behaviour_mut().kademlia.get_record(key);
    }

    fn write_rooms_record(&mut self) {
        let key = kad::RecordKey::new(&ROOMS_DHT_KEY);
        let rooms_json =
            serde_json::to_string(&self.rooms.iter().collect::<Vec<_>>()).unwrap_or_default();
        let record = kad::Record::new(key, rooms_json.into_bytes());
        if let Err(e) = self
            .swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, kad::Quorum::One)
        {
            log::warn!("Failed to put rooms record: {:?}", e);
        }
    }

    fn handle_kad_get(
        &mut self,
        key: kad::RecordKey,
        result: Result<Option<Vec<u8>>, kad::GetRecordError>,
    ) {
        if key.as_ref() != ROOMS_DHT_KEY.as_bytes() {
            return;
        }
        match result {
            Ok(Some(data)) => {
                let mut rooms: Vec<String> = serde_json::from_slice::<Vec<&str>>(&data)
                    .unwrap_or_default()
                    .into_iter()
                    .map(String::from)
                    .collect();
                for r in &self.rooms {
                    if !rooms.contains(r) {
                        rooms.push(r.clone());
                    }
                }
                self.write_rooms_record();
                let _ = self.event_tx.send(P2PEvent::RoomsDiscovered(rooms));
            }
            Ok(None) | Err(kad::GetRecordError::NotFound { .. }) => {
                self.write_rooms_record();
                let rooms: Vec<String> = self.rooms.iter().cloned().collect();
                let _ = self.event_tx.send(P2PEvent::RoomsDiscovered(rooms));
            }
            Err(e) => {
                log::warn!("Failed to get rooms record: {:?}", e);
            }
        }
    }

    pub fn send_message(
        &mut self,
        room_id: &str,
        data: Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let topic = gossipsub::IdentTopic::new(format!("rmsg-room-{}", room_id));
        let envelope = MeshEnvelope {
            mesh: 1,
            id: uuid::Uuid::new_v4().to_string(),
            room_id: room_id.to_string(),
            origin: self.local_peer_id.to_string(),
            ttl: 7,
            payload: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
        };
        self.seen_mesh.insert(envelope.id.clone());
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, serde_json::to_vec(&envelope)?)?;
        Ok(())
    }

    pub fn dial_addr(&mut self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        if !self.known_addrs.insert(addr.to_string()) {
            return Ok(());
        }
        let multiaddr = addr.parse::<Multiaddr>()?;
        self.swarm.dial(multiaddr)?;
        Ok(())
    }

    pub fn reserve_relay(&mut self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let relay_addr = addr.parse::<Multiaddr>()?;
        let reservation_addr = relay_reservation_addr(&relay_addr)?;
        self.swarm.listen_on(reservation_addr.clone())?;
        let advertised = reservation_addr.with(Protocol::P2p(self.local_peer_id));
        let _ = self
            .event_tx
            .send(P2PEvent::Listening(advertised.to_string()));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn local_peer_id(&self) -> PeerId {
        *self.swarm.local_peer_id()
    }

    pub async fn run(&mut self) {
        loop {
            match self.swarm.next().await.expect("swarm ended") {
                SwarmEvent::Behaviour(RelayBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { message, .. },
                )) => {
                    if let Ok(mut envelope) = serde_json::from_slice::<MeshEnvelope>(&message.data)
                    {
                        if envelope.mesh == 1 && self.seen_mesh.insert(envelope.id.clone()) {
                            if let Ok(payload) = base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                &envelope.payload,
                            ) {
                                let _ = self.event_tx.send(P2PEvent::Message { data: payload });
                            }
                            if envelope.ttl > 0 {
                                envelope.ttl -= 1;
                                let topic = gossipsub::IdentTopic::new(format!(
                                    "rmsg-room-{}",
                                    envelope.room_id
                                ));
                                if let Ok(bytes) = serde_json::to_vec(&envelope) {
                                    let _ =
                                        self.swarm.behaviour_mut().gossipsub.publish(topic, bytes);
                                }
                            }
                        }
                    } else {
                        let _ = self.event_tx.send(P2PEvent::Message { data: message.data });
                    }
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Gossipsub(
                    gossipsub::Event::Subscribed { peer_id, .. },
                )) => {
                    self.topic_peers.insert(peer_id);
                    let _ = self.event_tx.send(P2PEvent::Subscribed);
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Kademlia(
                    kad::Event::OutboundQueryProgressed {
                        result:
                            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(peer_record))),
                        ..
                    },
                )) => {
                    self.handle_kad_get(peer_record.record.key, Ok(Some(peer_record.record.value)));
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Kademlia(
                    kad::Event::OutboundQueryProgressed {
                        result: kad::QueryResult::GetRecord(Err(e)),
                        ..
                    },
                )) => {
                    let key = kad::RecordKey::new(&ROOMS_DHT_KEY);
                    self.handle_kad_get(key, Err(e));
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(
                    identify::Event::Received { peer_id, info, .. },
                )) => {
                    for addr in &info.listen_addrs {
                        self.swarm
                            .behaviour_mut()
                            .kademlia
                            .add_address(&peer_id, addr.clone());
                        let addr_with_peer = format!("{}/p2p/{}", addr, peer_id);
                        if self.known_addrs.insert(addr_with_peer.clone()) {
                            let _ = self.swarm.dial(addr.clone());
                            let _ = self.event_tx.send(P2PEvent::PeerAddress(addr_with_peer));
                        }
                    }
                    self.swarm
                        .behaviour_mut()
                        .gossipsub
                        .add_explicit_peer(&peer_id);
                    if self.known_peers.insert(peer_id) {
                        let _ = self.event_tx.send(P2PEvent::PeerDiscovered(peer_id));
                    }
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Mdns(mdns::Event::Discovered(
                    peers,
                ))) => {
                    for (peer_id, addr) in peers {
                        self.swarm
                            .behaviour_mut()
                            .kademlia
                            .add_address(&peer_id, addr.clone());
                        self.swarm
                            .behaviour_mut()
                            .gossipsub
                            .add_explicit_peer(&peer_id);
                        let addr_with_peer = format!("{}/p2p/{}", addr, peer_id);
                        if self.known_addrs.insert(addr_with_peer.clone()) {
                            let _ = self.event_tx.send(P2PEvent::PeerAddress(addr_with_peer));
                            let _ = self.swarm.dial(addr);
                        }
                        if self.known_peers.insert(peer_id) {
                            let _ = self.event_tx.send(P2PEvent::PeerDiscovered(peer_id));
                        }
                    }
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                    for (peer_id, _) in peers {
                        self.swarm
                            .behaviour_mut()
                            .gossipsub
                            .remove_explicit_peer(&peer_id);
                        self.topic_peers.remove(&peer_id);
                    }
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::RelayClient(
                    relay::client::Event::ReservationReqAccepted { relay_peer_id, .. },
                )) => {
                    log::info!("Relay reservation accepted by {}", relay_peer_id);
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::RelayClient(
                    relay::client::Event::OutboundCircuitEstablished { relay_peer_id, .. },
                )) => {
                    log::info!("Outbound relay circuit established via {}", relay_peer_id);
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::RelayClient(
                    relay::client::Event::InboundCircuitEstablished { src_peer_id, .. },
                )) => {
                    log::info!("Inbound relay circuit established from {}", src_peer_id);
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Dcutr(event)) => {
                    log::info!("DCUtR event: {:?}", event);
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::RelayServer(event)) => {
                    log::debug!("Relay server event: {:?}", event);
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    log::info!("Listening on {}", address);
                    let _ = self.event_tx.send(P2PEvent::Listening(format!(
                        "{}/p2p/{}",
                        address, self.local_peer_id
                    )));
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    log::info!("Connected to {}", peer_id);
                }
                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    self.topic_peers.remove(&peer_id);
                }
                _ => {}
            }
        }
    }
}

fn relay_reservation_addr(addr: &Multiaddr) -> Result<Multiaddr, Box<dyn std::error::Error>> {
    let has_peer_id = addr.iter().any(|p| matches!(p, Protocol::P2p(_)));
    if !has_peer_id {
        return Err("relay address must include /p2p/<peer-id>".into());
    }
    if addr.iter().any(|p| matches!(p, Protocol::P2pCircuit)) {
        return Ok(addr.clone());
    }
    Ok(addr.clone().with(Protocol::P2pCircuit))
}
