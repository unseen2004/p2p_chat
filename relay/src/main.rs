use anyhow::Result;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use base64::Engine;
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
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{error, info, warn};

const MAX_STORED_MESSAGES: usize = 500;
const MAX_MESSAGE_BYTES: usize = 4096;
const SAVE_BATCH_SIZE: usize = 1;

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

fn load_or_create_identity(path: &Path) -> Result<identity::Keypair> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_file) => {
            let kp = identity::Keypair::generate_ed25519();
            let bytes = kp.to_protobuf_encoding()?;
            fs::write(path, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            info!(path = %path.display(), "Generated and saved new relay identity");
            Ok(kp)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = fs::read(path)?;
            Ok(identity::Keypair::from_protobuf_encoding(&bytes)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Format a unix timestamp (seconds) as "YYYY-MM-DD HH:MM:SS UTC".
fn fmt_timestamp(ts: u64) -> String {
    if ts == 0 {
        return "—".to_string();
    }
    let mut days = ts / 86400;
    let time_of_day = ts % 86400;
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    let ss = time_of_day % 60;

    let mut year = 1970u32;
    loop {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [31u64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    for &md in &month_days {
        if days < md { break; }
        days -= md;
        month += 1;
    }
    let day = days + 1;
    format!("{year}-{month:02}-{day:02} {hh:02}:{mm:02}:{ss:02} UTC")
}

// ── Shared state ────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    messages: Arc<RwLock<VecDeque<ChatMessage>>>,
    password: String,
}

// ── HTTP handlers ─────────────────────────────────────────────────────────

fn check_auth(headers: &HeaderMap, password: &str) -> bool {
    let Some(auth) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(val) = auth.to_str() else { return false };
    let Some(encoded) = val.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return false;
    };
    let Ok(creds) = std::str::from_utf8(&decoded) else {
        return false;
    };
    creds == format!("admin:{password}") || creds == format!(":{password}")
}

async fn inbox_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !check_auth(&headers, &state.password) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"Inbox\"")],
            "Unauthorized",
        )
            .into_response();
    }

    let msgs = state.messages.read().unwrap();
    let rows: String = if msgs.is_empty() {
        "<tr><td colspan='3' style='color:#8b949e;text-align:center;padding:2rem'>No messages yet.</td></tr>".to_string()
    } else {
        msgs.iter()
            .rev()
            .map(|m| {
                let sender  = html_escape(&m.sender_id);
                let content = html_escape(&m.content);
                let ts      = fmt_timestamp(m.timestamp);
                format!(
                    "<tr><td class='ts'>{ts}</td><td class='sender'>{sender}</td><td class='body'>{content}</td></tr>"
                )
            })
            .collect()
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="refresh" content="30">
<title>Inbox</title>
<style>
*{{box-sizing:border-box;margin:0;padding:0}}
body{{background:#0d1117;color:#c9d1d9;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;padding:2rem}}
h1{{font-size:1.1rem;margin-bottom:0.4rem;color:#e6edf3}}
.subtitle{{font-size:0.72rem;color:#8b949e;margin-bottom:1.2rem}}
table{{width:100%;border-collapse:collapse;max-width:900px}}
th{{text-align:left;font-size:0.72rem;color:#8b949e;padding:0.4rem 0.6rem;border-bottom:1px solid rgba(255,255,255,0.08)}}
td{{padding:0.55rem 0.6rem;border-bottom:1px solid rgba(255,255,255,0.05);font-size:0.85rem;vertical-align:top}}
td.ts{{color:#8b949e;font-size:0.7rem;white-space:nowrap;width:160px}}
td.sender{{color:#79c0ff;font-size:0.7rem;word-break:break-all;width:220px}}
td.body{{color:#e6edf3;white-space:pre-wrap;word-break:break-word}}
tr:hover td{{background:rgba(255,255,255,0.03)}}
</style>
</head>
<body>
<h1>&#128274; Inbox &mdash; {count} message(s)</h1>
<div class="subtitle">Auto-refreshes every 30s</div>
<table>
<thead><tr><th>Time (UTC)</th><th>From (Peer ID)</th><th>Message</th></tr></thead>
<tbody>{rows}</tbody>
</table>
</body>
</html>
"#,
        count = msgs.len(),
        rows  = rows
    );

    Html(html).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ── Main ──────────────────────────────────────────────────────────────────

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
    let http_port: u16 = std::env::var("INBOX_HTTP_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);
    let inbox_password = std::env::var("INBOX_PASSWORD")
        .unwrap_or_else(|_| "changeme".to_string());

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
        .mesh_n_low(1)
        .mesh_n(2)
        .mesh_n_high(4)
        .mesh_outbound_min(0)
        .build()
        .expect("Valid gossipsub config");

    let mut gossipsub_behaviour = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    let topic = gossipsub::IdentTopic::new("global-chat");
    gossipsub_behaviour.subscribe(&topic)?;

    let store = kad::store::MemoryStore::new(local_peer_id);
    let kademlia = kad::Behaviour::new(local_peer_id, store);

    let behaviour = RelayBehavior {
        gossipsub: gossipsub_behaviour,
        kademlia,
    };

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

    let stored_messages = Arc::new(RwLock::new(load_messages(&storage_path)));
    info!(count = stored_messages.read().unwrap().len(), "Loaded messages from storage");

    let app_state = AppState {
        messages: Arc::clone(&stored_messages),
        password: inbox_password,
    };
    let app = Router::new()
        .route("/inbox", get(inbox_handler))
        .with_state(app_state);
    let http_addr: std::net::SocketAddr = format!("0.0.0.0:{http_port}").parse()?;
    info!(addr = %http_addr, "HTTP inbox server listening");
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    let mut unsaved: usize = 0;

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!(addr = %address, "Relay listening");
            }
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                message, ..
            })) => {
                if message.data.len() > MAX_MESSAGE_BYTES {
                    warn!(len = message.data.len(), "Dropping oversized raw message");
                    continue;
                }
                if let Ok(msg) = ChatMessage::decode(&message.data[..]) {
                    info!(sender = %msg.sender_id, content = %msg.content, "Message received");
                    let mut msgs = stored_messages.write().unwrap();
                    if !msgs.iter().any(|m| m.id == msg.id) {
                        msgs.push_back(msg);
                        while msgs.len() > MAX_STORED_MESSAGES {
                            msgs.pop_front();
                        }
                        unsaved += 1;
                        if unsaved >= SAVE_BATCH_SIZE {
                            save_messages(&storage_path, &msgs);
                            unsaved = 0;
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(RelayBehaviorEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic: t },
            )) => {
                if t == topic.hash() {
                    info!(peer = %peer_id, "Peer subscribed");
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
