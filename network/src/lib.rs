use anyhow::{Context, Result};
use blake3::Hasher as Blake3Hasher;
use common::pb::ChatMessage;
use futures::StreamExt;
use libp2p::{
    gossipsub, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux,
    PeerId, SwarmBuilder,
};
use prost::Message;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{error, info, warn};

/// Maximum allowed size in bytes for a single chat message content.
const MAX_MESSAGE_BYTES: usize = 4096;

#[derive(NetworkBehaviour)]
pub struct ChatBehavior {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

/// Load an Ed25519 keypair from `path`, creating and persisting it if absent.
pub fn load_or_create_identity(path: &Path) -> Result<(identity::Keypair, PeerId)> {
    let local_key = if path.exists() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading keypair from {}", path.display()))?;
        identity::Keypair::from_protobuf_encoding(&bytes)
            .context("decoding keypair from file")?  
    } else {
        let kp = identity::Keypair::generate_ed25519();
        let bytes = kp.to_protobuf_encoding()
            .context("encoding keypair")?;
        std::fs::write(path, &bytes)
            .with_context(|| format!("writing keypair to {}", path.display()))?;
        info!(path = %path.display(), "Generated and saved new identity");
        kp
    };
    let peer_id = PeerId::from(local_key.public());
    Ok((local_key, peer_id))
}

pub async fn start_node(port: u16, relay_addr: Option<String>) -> Result<()> {
    // Validate relay address early for a clear error message.
    let relay_multiaddr: Option<libp2p::Multiaddr> = relay_addr
        .as_deref()
        .map(|s| s.parse().context("invalid relay multiaddr"))
        .transpose()?;

    let identity_path = Path::new("identity.key");
    let (local_key, local_peer_id) = load_or_create_identity(identity_path)?;
    info!(peer_id = %local_peer_id, "Local identity loaded");

    let message_id_fn = |message: &gossipsub::Message| {
        if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
            gossipsub::MessageId::from(msg.id)
        } else {
            // Deterministic fallback using blake3 instead of DefaultHasher.
            let mut h = Blake3Hasher::new();
            h.update(&message.data);
            gossipsub::MessageId::from(h.finalize().to_string())
        }
    };

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(1))
        .validation_mode(gossipsub::ValidationMode::Strict)
        .message_id_fn(message_id_fn)
        .build()
        .expect("Valid gossipsub config");

    let mut gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .expect("Correct gossipsub configuration");

    let topic = gossipsub::IdentTopic::new("global-chat");
    gossipsub.subscribe(&topic)?;

    let store = kad::store::MemoryStore::new(local_peer_id);
    let mut kademlia = kad::Behaviour::new(local_peer_id, store);

    // Pre-add relay to Kademlia routing table so bootstrap has an entry point.
    if let Some(ref addr) = relay_multiaddr {
        if let Some(peer_id) = addr.iter().find_map(|p| {
            if let libp2p::multiaddr::Protocol::P2p(id) = p {
                Some(id)
            } else {
                None
            }
        }) {
            kademlia.add_address(&peer_id, addr.clone());
            let _ = kademlia.bootstrap();
            info!("Kademlia bootstrap initiated via relay peer");
        }
    }

    let behaviour = ChatBehavior {
        gossipsub,
        kademlia,
    };

    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_| behaviour)?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    let listen_addr = format!("/ip4/0.0.0.0/tcp/{}", port).parse()?;
    swarm.listen_on(listen_addr)?;

    if let Some(addr) = relay_multiaddr {
        info!(addr = %addr, "Dialing relay");
        swarm.dial(addr)?;
    }

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();

    loop {
        tokio::select! {
            Ok(Some(line)) = stdin.next_line() => {
                if line.len() > MAX_MESSAGE_BYTES {
                    warn!(len = line.len(), max = MAX_MESSAGE_BYTES, "Message too large, dropping");
                    continue;
                }
                let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                let chat_msg = ChatMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    sender_id: local_peer_id.to_string(),
                    recipient_id: "global".to_string(),
                    content: line,
                    timestamp,
                };

                let mut buf = Vec::new();
                chat_msg.encode(&mut buf)?;

                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), buf) {
                    error!(err = ?e, "Publish error");
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!(addr = %address, "Listening");
                }
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    info!(peer = %peer_id, "Connected");
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, endpoint.get_remote_address().clone());
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    error!(peer = ?peer_id, err = ?error, "Outgoing connection error");
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    error!(err = ?error, "Incoming connection error");
                }
                SwarmEvent::Behaviour(ChatBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                    message,
                    ..
                })) => {
                    if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                        info!(sender = %msg.sender_id, content = %msg.content);
                    }
                }
                _ => {}
            }
        }
    }
}
