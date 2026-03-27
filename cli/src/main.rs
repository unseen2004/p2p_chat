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
        #[arg(short, long, default_value_t = 0)]
        port: u16,

        #[arg(short, long)]
        relay: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Start { port, relay } => {
            println!("Starting Unseen Chat node...");
            network::start_node(*port, relay.clone()).await?;
        }
    }

    Ok(())
}
