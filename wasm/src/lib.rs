use common::pb::ChatMessage;
use futures::channel::mpsc;
use futures::StreamExt;
use futures::future::FutureExt;
use futures::select;
use js_sys::Function;
use libp2p::{
    gossipsub, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, yamux,
    PeerId, SwarmBuilder, web_socket_websys,
};
use prost::Message;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use wasm_bindgen::prelude::*;

static SENDER: std::sync::OnceLock<mpsc::UnboundedSender<String>> = std::sync::OnceLock::new();

#[derive(NetworkBehaviour)]
pub struct ChatBehavior {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

#[wasm_bindgen]
pub async fn start_chat(relay_addr: String, on_message: Function) -> Result<(), JsValue> {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    
    let message_id_fn = |message: &gossipsub::Message| {
        let mut s = DefaultHasher::new();
        message.data.hash(&mut s);
        gossipsub::MessageId::from(s.finish().to_string())
    };

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .message_id_fn(message_id_fn)
        .build()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    let mut gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .map_err(|e| JsValue::from_str(&e.to_string()))?;

    let topic = gossipsub::IdentTopic::new("global-chat");
    gossipsub.subscribe(&topic).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let store = kad::store::MemoryStore::new(local_peer_id);
    let kademlia = kad::Behaviour::new(local_peer_id, store);

    let behaviour = ChatBehavior {
        gossipsub,
        kademlia,
    };

    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_wasm_bindgen()
        .with_other_transport(|key| {
            web_socket_websys::Transport::default()
                .upgrade(libp2p::core::upgrade::Version::V1)
                .authenticate(noise::Config::new(key).unwrap())
                .multiplex(yamux::Config::default())
        })
        .map_err(|e| JsValue::from_str(&e.to_string()))?
        .with_behaviour(|_| behaviour)
        .map_err(|e| JsValue::from_str(&e.to_string()))?
        .build();

    let addr: libp2p::Multiaddr = relay_addr.parse().map_err(|e| JsValue::from_str(&e.to_string()))?;
    swarm.dial(addr).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let (tx, mut rx) = mpsc::unbounded::<String>();
    let _ = SENDER.set(tx);

    loop {
        select! {
            line = rx.next().fuse() => {
                if let Some(content) = line {
                    let chat_msg = ChatMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        sender_id: local_peer_id.to_string(),
                        recipient_id: "global".to_string(),
                        content,
                        timestamp: js_sys::Date::now() as u64 / 1000,
                    };

                    let mut buf = Vec::new();
                    if chat_msg.encode(&mut buf).is_ok() {
                        let _ = swarm.behaviour_mut().gossipsub.publish(topic.clone(), buf);
                    }
                }
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(ChatBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                    message,
                    ..
                })) => {
                    if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                        let this = JsValue::null();
                        let content = JsValue::from_str(&msg.content);
                        let sender = JsValue::from_str(&msg.sender_id);
                        let _ = on_message.call2(&this, &sender, &content);
                    }
                }
                _ => {}
            }
        }
    }
}

#[wasm_bindgen]
pub fn send_message(content: String) -> Result<(), JsValue> {
    if let Some(tx) = SENDER.get() {
        tx.unbounded_send(content).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(())
    } else {
        Err(JsValue::from_str("Node not started"))
    }
}
