use anyhow::Result;
use blake3::Hasher as Blake3Hasher;
use common::pb::ChatMessage;
use futures::StreamExt;
use libp2p::{
    gossipsub, identity, kad, noise,
    swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux,
    PeerId, SwarmBuilder,
};
use prost::Message;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info, warn};

/// Maximum number of messages kept in the relay's history.
const MAX_STORED_MESSAGES: usize = 500;
/// Maximum allowed size in bytes for a single encoded protobuf message.
const MAX_MESSAGE_BYTES: usize = 4096;
/// Persist storage after this many new messages (batched writes).
const SAVE_BATCH_SIZE: usize = 10;

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
/// Uses O_CREAT|O_EXCL (create_new) to avoid TOCTOU race between concurrent instances.
/// Sets file permissions to 0o600 (owner read/write only).
fn load_or_create_identity(path: &Path) -> Result<identity::Keypair> {
    // Try atomic creation first.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_file) => {
            // We won the race — generate and write.
            let kp = identity::Keypair::generate_ed25519();
            let bytes = kp.to_protobuf_encoding()?;
            fs::write(path, &bytes)?;
            // Restrict permissions to owner-only on Unix.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            info!(path = %path.display(), "Generated and saved new relay identity");
            Ok(kp)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // File already exists — load it.
            let bytes = fs::read(path)?;
            Ok(identity::Keypair::from_protobuf_encoding(&bytes)?)
        }
        Err(e) => Err(e.into()),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let storage_path: PathBuf = std::env::var("STORAGE_PATH")
        .unwrap_or_else(|_| "storage.json".to_string())
        .into();
    let identity_path: PathBuf = std::env::var("IDENTITY_PATH")
        .unwrap_or_else(|_| "relay_identity.key".to_string())
        .into();
    let tcp_port: u16 = std::env::var("RELAY_TCP_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8000);
    let ws_port: u16 = std::env::var("RELAY_WS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8001);

    let local_key = load_or_create_identity(&identity_path)?;
    let local_peer_id = PeerId::from(local_key.public());
    info!(peer_id = %local_peer_id, "Relay identity loaded");

    let message_id_fn = |message: &gossipsub::Message| {
        if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
            gossipsub::MessageId::from(msg.id)
        } else {
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

    let behaviour = RelayBehavior { gossipsub, kademlia };

    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_websocket(noise::Config::new, yamux::Config::default)
        .await?
        .with_behaviour(|_| behaviour)?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{tcp_port}").parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{ws_port}/ws").parse()?)?;

    let mut stored_messages = load_messages(&storage_path);
    info!(count = stored_messages.len(), "Loaded messages from storage");

    // Counter for batched saves.
    let mut unsaved: usize = 0;

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!(addr = %address, "Relay listening");
            }
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                message, ..
            })) => {
                // Enforce encoded payload size limit.
                if message.data.len() > MAX_MESSAGE_BYTES {
                    warn!(len = message.data.len(), "Dropping oversized raw message");
                    continue;
                }
                if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                    info!(sender = %msg.sender_id, "Message received");
                    if !stored_messages.iter().any(|m| m.id == msg.id) {
                        stored_messages.push_back(msg);
                        while stored_messages.len() > MAX_STORED_MESSAGES {
                            stored_messages.pop_front();
                        }
                        unsaved += 1;
                        if unsaved >= SAVE_BATCH_SIZE {
                            save_messages(&storage_path, &stored_messages);
                            unsaved = 0;
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic: t },
            )) => {
                if t == topic.hash() {
                    info!(
                        peer = %peer_id,
                        count = stored_messages.len(),
                        "Peer subscribed — history replay disabled (implement request-response for targeted delivery)"
                    );
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
