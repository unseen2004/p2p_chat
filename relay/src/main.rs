use anyhow::Result;
use blake3::Hasher as Blake3Hasher;
use common::pb::ChatMessage;
use futures::StreamExt;
use libp2p::{
    gossipsub, identity, kad, noise, request_response,
    swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux,
    PeerId, SwarmBuilder,
};
use prost::Message;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info};

/// Maximum number of messages kept in the relay's history.
const MAX_STORED_MESSAGES: usize = 500;
/// Maximum allowed size in bytes for a single chat message content.
const MAX_MESSAGE_BYTES: usize = 4096;

#[derive(NetworkBehaviour)]
pub struct RelayBehavior {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

fn load_messages(path: &Path) -> VecDeque<ChatMessage> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return VecDeque::new(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_messages(path: &Path, msgs: &VecDeque<ChatMessage>) {
    match serde_json::to_string(msgs) {
        Ok(json) => {
            // Write to a temp file first, then rename for atomic replacement.
            let tmp_path = path.with_extension("tmp");
            if let Err(e) = fs::write(&tmp_path, json.as_bytes()) {
                error!("Failed to write temp storage file: {e}");
                return;
            }
            if let Err(e) = fs::rename(&tmp_path, path) {
                error!("Failed to rename temp storage file: {e}");
            }
        }
        Err(e) => error!("Failed to serialize messages: {e}"),
    }
}

/// Load an Ed25519 keypair from `path`, creating and persisting it if absent.
fn load_or_create_identity(path: &Path) -> Result<identity::Keypair> {
    if path.exists() {
        let bytes = fs::read(path)?;
        Ok(identity::Keypair::from_protobuf_encoding(&bytes)?)
    } else {
        let kp = identity::Keypair::generate_ed25519();
        let bytes = kp.to_protobuf_encoding()?;
        fs::write(path, &bytes)?;
        info!(path = %path.display(), "Generated and saved new relay identity");
        Ok(kp)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Allow overriding storage path via env var for flexibility.
    let storage_path: PathBuf = std::env::var("STORAGE_PATH")
        .unwrap_or_else(|_| "storage.json".to_string())
        .into();

    let identity_path: PathBuf = std::env::var("IDENTITY_PATH")
        .unwrap_or_else(|_| "relay_identity.key".to_string())
        .into();

    let local_key = load_or_create_identity(&identity_path)?;
    let local_peer_id = PeerId::from(local_key.public());
    info!(peer_id = %local_peer_id, "Relay identity loaded");

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
    .map_err(|e| anyhow::anyhow!(e))?;

    let topic = gossipsub::IdentTopic::new("global-chat");
    gossipsub.subscribe(&topic)?;

    let store = kad::store::MemoryStore::new(local_peer_id);
    let kademlia = kad::Behaviour::new(local_peer_id, store);

    let behaviour = RelayBehavior {
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
        .with_websocket(
            noise::Config::new,
            yamux::Config::default,
        )
        .await?
        .with_behaviour(|_| behaviour)?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/8000".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/8001/ws".parse()?)?;

    let mut stored_messages = load_messages(&storage_path);
    info!(count = stored_messages.len(), "Loaded messages from storage");

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!(addr = %address, "Relay listening");
            }
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                    // Drop oversized messages before storing.
                    if msg.content.len() > MAX_MESSAGE_BYTES {
                        error!(sender = %msg.sender_id, len = msg.content.len(), "Dropping oversized message");
                        continue;
                    }
                    info!(sender = %msg.sender_id, "Message received");

                    if !stored_messages.iter().any(|m| m.id == msg.id) {
                        stored_messages.push_back(msg);
                        while stored_messages.len() > MAX_STORED_MESSAGES {
                            stored_messages.pop_front();
                        }
                        save_messages(&storage_path, &stored_messages);
                    }
                }
            }
            // NOTE: History replay via gossipsub broadcast floods the entire mesh.
            // A proper fix requires a request-response protocol so history is sent
            // directly to the newly-subscribed peer only. This is tracked as a
            // follow-up task. For now, history replay on subscription is removed
            // to prevent mesh flooding.
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic: t },
            )) => {
                if t == topic.hash() {
                    info!(peer = %peer_id, count = stored_messages.len(),
                          "Peer subscribed — history replay via gossipsub disabled to prevent mesh flood. \
                           Implement request-response protocol for targeted delivery.");
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                info!(peer = %peer_id, "Peer connected");
            }
            SwarmEvent::IncomingConnectionError { error, .. } => {
                error!(err = ?error, "Incoming connection error");
            }
            SwarmEvent::OutgoingConnectionError { error, .. } => {
                error!(err = ?error, "Outgoing connection error");
            }
            _ => {}
        }
    }
}
