use libp2p::{
    core::upgrade,
    dcutr, gossipsub, identify, kad, noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, PeerId, Swarm, Transport,
    identity, Multiaddr,
};
use futures::StreamExt;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;

const BOOTSTRAP_NODES: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
];

pub enum P2PEvent {
    Message { data: Vec<u8> },
    PeerDiscovered(PeerId),
    Subscribed,
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
}

pub struct P2PNode {
    swarm: Swarm<RelayBehaviour>,
    topic_peers: HashSet<PeerId>,
    event_tx: mpsc::UnboundedSender<P2PEvent>,
    rooms: HashSet<String>,
}

impl P2PNode {
    pub fn new(
        event_tx: mpsc::UnboundedSender<P2PEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let local_key = identity::Keypair::generate_ed25519();
        let peer_id = PeerId::from(local_key.public());
        log::info!("Peer ID: {}", peer_id);

        let transport = tcp::tokio::Transport::default()
            .upgrade(upgrade::Version::V1Lazy)
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

        let kademlia = kad::Behaviour::new(
            peer_id,
            kad::store::MemoryStore::new(peer_id),
        );

        let identify = identify::Behaviour::new(
identify::Config::new("rmsg/0.1.0".into(), local_key.public())
.with_agent_version("rmsg-p2p/0.1.0".into()),
        );

        let ping = ping::Behaviour::new(ping::Config::default()
            .with_interval(Duration::from_secs(30)));

        let dcutr = dcutr::Behaviour::new(peer_id);

        let behaviour = RelayBehaviour {
            gossipsub,
            kademlia,
            identify,
            ping,
            dcutr,
        };

        let swarm_config = libp2p::swarm::Config::with_tokio_executor()
            .with_idle_connection_timeout(Duration::from_secs(60));

        let mut swarm = Swarm::new(transport, behaviour, peer_id, swarm_config);

        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse::<Multiaddr>()?)?;

        for addr_str in BOOTSTRAP_NODES {
            if let Ok(multiaddr) = addr_str.parse::<Multiaddr>() {
                if let Err(e) = swarm.dial(multiaddr) {
                    log::warn!("Failed to dial bootstrap: {}", e);
                }
            }
        }

        Ok(Self { swarm, topic_peers: HashSet::new(), event_tx, rooms: HashSet::new() })
    }

    pub fn join_room(&mut self, room_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.rooms.insert(room_id.to_string());
        let topic = gossipsub::IdentTopic::new(format!("rmsg-room-{}", room_id));
        self.swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
        Ok(())
    }

    pub fn send_message(&mut self, room_id: &str, data: Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
        let topic = gossipsub::IdentTopic::new(format!("rmsg-room-{}", room_id));
        self.swarm.behaviour_mut().gossipsub.publish(topic, data)?;
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
                    gossipsub::Event::Message { message, .. }
                )) => {
                    let _ = self.event_tx.send(P2PEvent::Message {
                        data: message.data,
                    });
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Gossipsub(
                    gossipsub::Event::Subscribed { peer_id, .. }
                )) => {
                    self.topic_peers.insert(peer_id);
                    let _ = self.event_tx.send(P2PEvent::Subscribed);
                }
                SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(
                    identify::Event::Received { peer_id, info, .. }
                )) => {
                    for addr in &info.listen_addrs {
                        self.swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
                    }
                    let _ = self.event_tx.send(P2PEvent::PeerDiscovered(peer_id));
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    log::info!("Listening on {}", address);
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
