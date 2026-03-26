use anyhow::Result;
use libp2p::{identity, PeerId};

pub fn create_identity() -> Result<(identity::Keypair, PeerId)> {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());

    Ok((local_key, local_peer_id))
}

pub async fn start_node(port: u16, _relay_addr: Option<String>) -> Result<()> {
    let (local_key, local_peer_id) = create_identity()?;

    println!("Your PeerId is: {}", local_peer_id);
    Ok(())
}
