use anyhow::Result;
use futures::StreamExt;
use libp2p::{identity, noise, ping, tcp, yamux, PeerId, SwarmBuilder};

pub fn create_identity() -> Result<(identity::Keypair, PeerId)> {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    Ok((local_key, local_peer_id))
}

pub async fn start_node(port: u16, relay_addr: Option<String>) -> Result<()> {
    let (local_key, local_peer_id) = create_identity()?;
    println!("Your unique PeerId is: {}", local_peer_id);

    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_| {
            ping::Behaviour::default()
        })?
        .with_swarm_config(|cfg| {
            cfg.with_idle_connection_timeout(std::time::Duration::from_secs(60))
        })
        .build();

    let listen_addr = format!("/ip4/0.0.0.0/tcp/{}", port).parse()?;
    swarm.listen_on(listen_addr)?;

    if let Some(addr_str) = relay_addr {
        let addr: libp2p::Multiaddr = addr_str.parse()?;
        println!("Trying to connect to node: {}", addr);
        swarm.dial(addr)?;
    }
    println!("Starting P2P engine... Waiting for events!");

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                match event {
                    libp2p::swarm::SwarmEvent::NewListenAddr { address, .. } => {
                        println!("Listening and ready at address: {}", address);
                    }
                    libp2p::swarm::SwarmEvent::Behaviour(event) => {
                        println!("Behavior event: {:?}", event);
                    }
                    libp2p::swarm::SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        println!("CONNECTED! Found peer: {}", peer_id);
                        println!("Peer address: {:?}", endpoint.get_remote_address());
                    }
                    _ => {}
                }
            }
        }
    }
}
