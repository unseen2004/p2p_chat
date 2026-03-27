use anyhow::Result;
use common::pb::ChatMessage;
use futures::StreamExt;
use libp2p::{
    gossipsub, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux,
    PeerId, SwarmBuilder, websocket,
};
use prost::Message;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use std::fs::{OpenOptions, File};
use std::io::{Read, Write};

#[derive(NetworkBehaviour)]
pub struct RelayBehavior {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

fn load_messages(path: &str) -> Vec<ChatMessage> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap_or_default();
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_message(path: &str, msg: ChatMessage) {
    let mut msgs = load_messages(path);
    msgs.push(msg);
    if let Ok(json) = serde_json::to_string(&msgs) {
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(path).unwrap();
        let _ = file.write_all(json.as_bytes());
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = "storage.json";
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("Relay PeerId: {}", local_peer_id);

    let message_id_fn = |message: &gossipsub::Message| {
        let mut s = DefaultHasher::new();
        message.data.hash(&mut s);
        gossipsub::MessageId::from(s.finish().to_string())
    };

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(10))
        .validation_mode(gossipsub::ValidationMode::Strict)
        .message_id_fn(message_id_fn)
        .build()
        .expect("Valid config");

    let mut gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )?;

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
            websocket::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .await?
        .with_behaviour(|_| behaviour)?
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/8000".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/8001/ws".parse()?)?;

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
                    println!("Relay received: {}", msg.content);
                    save_message(config_path, msg);
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("Peer connected: {}", peer_id);
                let msgs = load_messages(config_path);
                for msg in msgs {
                    let mut buf = Vec::new();
                    if msg.encode(&mut buf).is_ok() {
                        let _ = swarm.behaviour_mut().gossipsub.publish(topic.clone(), buf);
                    }
                }
            }
            _ => {}
        }
    }
}
