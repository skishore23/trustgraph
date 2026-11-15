use clap::Parser;
use std::{error::Error, path::PathBuf};

/// Review native-agent GitHub issue proposals, then execute through Composio.
#[derive(Parser)]
#[command(name = "trustgraph-composio-issues", version)]
struct Cli {
    #[arg(long, default_value = "trustgraph-actions.local.json")]
    config: PathBuf,
    #[arg(long, default_value_t = 8084)]
    port: u16,
    #[arg(long, default_value_t = 8085)]
    agent_port: u16,
    /// Verify Connect MCP credentials, account and schema without starting or writing an issue.
    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let Cli {
        config,
        port,
        agent_port,
        check,
    } = Cli::parse();
    if check {
        tokio::task::spawn_blocking(move || trustgraph_composio_example::check(&config)).await??;
    } else {
        trustgraph_composio_example::serve(&config, port, agent_port).await?;
    }
    Ok(())
}
