use anyhow::Result;
use common::pb::ChatMessage;
use futures::StreamExt;
use libp2p::{
    gossipsub, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux,
    PeerId, SwarmBuilder,
};
use prost::Message;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(NetworkBehaviour)]
pub struct ChatBehavior {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

pub fn create_identity() -> Result<(identity::Keypair, PeerId)> {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    Ok((local_key, local_peer_id))
}

pub async fn start_node(port: u16, relay_addr: Option<String>) -> Result<()> {
    let (local_key, local_peer_id) = create_identity()?;
    println!("Local PeerId: {}", local_peer_id);

    let message_id_fn = |message: &gossipsub::Message| {
        // Use the protobuf message UUID as gossipsub ID to avoid
        // deduplication of different messages with identical content.
        if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
            gossipsub::MessageId::from(msg.id)
        } else {
            // Fallback: use raw data hash
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
    .expect("Correct configuration");

    let topic = gossipsub::IdentTopic::new("global-chat");
    gossipsub.subscribe(&topic)?;

    let store = kad::store::MemoryStore::new(local_peer_id);
    let kademlia = kad::Behaviour::new(local_peer_id, store);

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

    if let Some(addr_str) = relay_addr {
        let addr: libp2p::Multiaddr = addr_str.parse()?;
        println!("Dialing: {}", addr);
        swarm.dial(addr)?;
    }

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();

    loop {
        tokio::select! {
            Ok(Some(line)) = stdin.next_line() => {
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
                    println!("Publish error: {:?}", e);
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("Listening on: {}", address);
                }
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    println!("Connected to: {}", peer_id);
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, endpoint.get_remote_address().clone());
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    println!("Outgoing connection error to {:?}: {:?}", peer_id, error);
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    println!("Incoming connection error: {:?}", error);
                }
                SwarmEvent::Behaviour(ChatBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                    message,
                    ..
                })) => {
                    if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                        println!("[{}]: {}", msg.sender_id, msg.content);
                    }
                }
                _ => {}
            }
        }
    }
}
