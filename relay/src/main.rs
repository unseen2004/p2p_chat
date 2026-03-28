use anyhow::Result;
use common::pb::ChatMessage;
use futures::StreamExt;
use libp2p::{
    gossipsub, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux,
    PeerId, SwarmBuilder,
};
use prost::Message;
use std::collections::VecDeque;
use std::fs;
use std::time::Duration;

/// Maximum number of messages kept in the relay's history.
const MAX_STORED_MESSAGES: usize = 500;

#[derive(NetworkBehaviour)]
pub struct RelayBehavior {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

fn load_messages(path: &str) -> VecDeque<ChatMessage> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return VecDeque::new(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_messages(path: &str, msgs: &VecDeque<ChatMessage>) {
    match serde_json::to_string(msgs) {
        Ok(json) => {
            // Write to a temp file first, then rename for atomic replacement.
            let tmp_path = format!("{}.tmp", path);
            if let Err(e) = fs::write(&tmp_path, json.as_bytes()) {
                eprintln!("Failed to write temp storage file: {e}");
                return;
            }
            if let Err(e) = fs::rename(&tmp_path, path) {
                eprintln!("Failed to rename temp storage file: {e}");
            }
        }
        Err(e) => eprintln!("Failed to serialize messages: {e}"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let storage_path = "storage.json";

    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("Relay PeerId: {}", local_peer_id);

    let message_id_fn = |message: &gossipsub::Message| {
        if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
            gossipsub::MessageId::from(msg.id)
        } else {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            gossipsub::MessageId::from(s.finish().to_string())
        }
    };

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(1))
        .validation_mode(gossipsub::ValidationMode::Strict)
        .message_id_fn(message_id_fn)
        .build()
        .expect("Valid config");

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

    let mut stored_messages = load_messages(storage_path);
    println!("Loaded {} messages from storage", stored_messages.len());

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Relay listening on: {}", address);
            }
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                    println!("[{}]: {}", msg.sender_id, msg.content);

                    // Only store if we haven't seen this message ID before.
                    if !stored_messages.iter().any(|m| m.id == msg.id) {
                        stored_messages.push_back(msg);
                        // Enforce bounded history.
                        while stored_messages.len() > MAX_STORED_MESSAGES {
                            stored_messages.pop_front();
                        }
                        save_messages(storage_path, &stored_messages);
                    }
                }
            }
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic: t },
            )) => {
                if t == topic.hash() {
                    println!("Peer subscribed: {} — sending {} history messages", peer_id, stored_messages.len());
                    for msg in &stored_messages {
                        let mut buf = Vec::new();
                        if msg.encode(&mut buf).is_ok() {
                            let _ = swarm
                                .behaviour_mut()
                                .gossipsub
                                .publish(topic.clone(), buf);
                        }
                    }
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("Peer connected: {}", peer_id);
            }
            SwarmEvent::IncomingConnectionError { error, .. } => {
                eprintln!("Incoming connection error: {:?}", error);
            }
            SwarmEvent::OutgoingConnectionError { error, .. } => {
                eprintln!("Outgoing connection error: {:?}", error);
            }
            _ => {}
        }
    }
}
