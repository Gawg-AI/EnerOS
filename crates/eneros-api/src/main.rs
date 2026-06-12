use clap::{Parser, Subcommand};
use eneros_api::server::ApiServer;

#[derive(Parser)]
#[command(name = "eneros")]
#[command(about = "EnerOS - Power-Native Agent Operating System")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the EnerOS API server
    Serve {
        /// Host address
        #[arg(short, long, default_value = "0.0.0.0")]
        host: String,

        /// Port number
        #[arg(short, long, default_value = "8080")]
        port: u16,
    },

    /// Run power flow calculation
    PowerFlow {
        /// Network case file
        #[arg(short, long)]
        case: String,
    },

    /// Check system constraints
    CheckConstraints,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { host, port } => {
            tracing::info!("Starting EnerOS server on {}:{}", host, port);
            let server = ApiServer::new(&host, port);
            server.start().await?;
        }
        Commands::PowerFlow { case } => {
            tracing::info!("Running power flow for case: {}", case);
            // Placeholder - would load case and run power flow
        }
        Commands::CheckConstraints => {
            tracing::info!("Checking system constraints");
            // Placeholder - would check all constraints
        }
    }

    Ok(())
}
