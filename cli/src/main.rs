use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Start {
        /// TCP port to listen on (0 = OS-assigned ephemeral port)
        #[arg(short, long, default_value_t = 0)]
        port: u16,

        #[arg(short, long)]
        relay: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialise structured logging; level controlled by RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Start { port, relay } => {
            tracing::info!("Starting p2p-chat node");
            network::start_node(*port, relay.clone()).await?;
        }
    }

    Ok(())
}
