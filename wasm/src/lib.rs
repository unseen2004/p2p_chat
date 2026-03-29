use common::pb::ChatMessage;
use futures::channel::mpsc;
use futures::future::FutureExt;
use futures::select;
use futures::StreamExt;
use js_sys::Function;
use libp2p::{
    gossipsub, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, websocket_websys,
    yamux, PeerId, Transport,
};
use prost::Message;
use wasm_bindgen::prelude::*;

/// Maximum allowed size in bytes for a single chat message content.
const MAX_MESSAGE_BYTES: usize = 4096;

static SENDER: std::sync::OnceLock<mpsc::UnboundedSender<String>> = std::sync::OnceLock::new();

#[derive(NetworkBehaviour)]
pub struct ChatBehavior {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

/// Start the chat node and connect to the relay.
///
/// `on_message(local_peer_id, sender_id, content)` is called for every
/// incoming gossipsub message so the JS side can filter out its own echoes.
#[wasm_bindgen]
pub async fn start_chat(relay_addr: String, on_message: Function) -> Result<(), JsValue> {
    if SENDER.get().is_some() {
        return Err(JsValue::from_str(
            "Chat node already started. Reload the page to reconnect.",
        ));
    }

    // Validate relay address eagerly for a clear error.
    let addr: libp2p::Multiaddr = relay_addr
        .parse()
        .map_err(|e: libp2p::multiaddr::Error| JsValue::from_str(&format!("Invalid relay address: {e:?}")))?;

    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());

    let message_id_fn = |message: &gossipsub::Message| {
        if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
            gossipsub::MessageId::from(msg.id)
        } else {
            // Deterministic fallback using blake3 instead of DefaultHasher.
            let hash = blake3::hash(&message.data);
            gossipsub::MessageId::from(hash.to_string())
        }
    };

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(std::time::Duration::from_secs(1))
        .message_id_fn(message_id_fn)
        .build()
        .map_err(|e: gossipsub::ConfigBuilderError| JsValue::from_str(&format!("{e:?}")))?;

    let mut gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .map_err(|e: &str| JsValue::from_str(e))?;

    let topic = gossipsub::IdentTopic::new("global-chat");
    gossipsub
        .subscribe(&topic)
        .map_err(|e: gossipsub::SubscriptionError| JsValue::from_str(&format!("{e:?}")))?;

    let store = kad::store::MemoryStore::new(local_peer_id);
    let kademlia = kad::Behaviour::new(local_peer_id, store);

    let behaviour = ChatBehavior {
        gossipsub,
        kademlia,
    };

    let transport = websocket_websys::Transport::default()
        .upgrade(libp2p::core::upgrade::Version::V1)
        .authenticate(
            noise::Config::new(&local_key)
                .map_err(|e| JsValue::from_str(&format!("{e:?}")))
                .unwrap(),
        )
        .multiplex(yamux::Config::default())
        .boxed();

    let mut swarm = libp2p::Swarm::new(
        transport,
        behaviour,
        local_peer_id,
        libp2p::swarm::Config::with_wasm_executor(),
    );

    swarm
        .dial(addr)
        .map_err(|e: libp2p::swarm::DialError| JsValue::from_str(&format!("{e:?}")))?;

    let (tx, mut rx) = mpsc::unbounded::<String>();
    let _ = SENDER.set(tx);

    let peer_id_str = local_peer_id.to_string();

    loop {
        select! {
            line = rx.next().fuse() => {
                if let Some(content) = line {
                    if content.len() > MAX_MESSAGE_BYTES {
                        web_sys::console::warn_1(&JsValue::from_str(
                            &format!("Message too large ({} bytes), dropping", content.len())
                        ));
                        continue;
                    }
                    let chat_msg = ChatMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        sender_id: peer_id_str.clone(),
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
            event = swarm.select_next_some() => if let SwarmEvent::Behaviour(
                ChatBehaviorEvent::Gossipsub(gossipsub::Event::Message { message, .. })
            ) = event
                && let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                    let this = JsValue::null();
                    let peer = JsValue::from_str(&peer_id_str);
                    let sender = JsValue::from_str(&msg.sender_id);
                    let content = JsValue::from_str(&msg.content);
                    let _ = on_message.call3(&this, &peer, &sender, &content);
            }
        }
    }
}

#[wasm_bindgen]
pub fn send_message(content: String) -> Result<(), JsValue> {
    if let Some(tx) = SENDER.get() {
        tx.unbounded_send(content)
            .map_err(|e: futures::channel::mpsc::TrySendError<String>| {
                JsValue::from_str(&format!("{e:?}"))
            })?;
        Ok(())
    } else {
        Err(JsValue::from_str("Node not started"))
    }
}
