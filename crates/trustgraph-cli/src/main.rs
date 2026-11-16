use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "trustgraph",
    version,
    about = "Policy-as-code for what agents produce"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Write, check, test, explain and diff a trustgraph.policy.yaml.
    Policy {
        #[command(subcommand)]
        command: trustgraph_cli::policy_cmd::PolicyCommand,
    },
    /// Serve the policy runtime to agents over HTTP and MCP, with an operator page.
    Serve {
        /// The policy file in force at start (it re-applies only when it changed).
        #[arg(long)]
        policy: PathBuf,
        /// The person who approves, revokes and activates; must be in a group of the policy.
        #[arg(long)]
        operator: String,
        /// State directory: journal, agent tokens, launch file [default: ~/.trustgraph/serve].
        #[arg(long)]
        state: Option<PathBuf>,
        #[arg(long, default_value_t = trustgraph_cli::serve::DEFAULT_PORT)]
        port: u16,
        /// Allow (with a warning) a state directory inside a git repository.
        #[arg(long)]
        allow_state_in_repository: bool,
        /// Most artifacts agents may create.
        #[arg(long, default_value_t = trustgraph_cli::serve::DEFAULT_MAX_ARTIFACTS)]
        max_artifacts: usize,
        /// Journal size, in bytes, past which agent writes are refused.
        #[arg(long, default_value_t = trustgraph_cli::serve::DEFAULT_MAX_JOURNAL_BYTES)]
        max_journal_bytes: u64,
    },
}
#[tokio::main]
async fn main() {
    let code = match Cli::parse().command {
        Command::Policy { command } => trustgraph_cli::policy_cmd::run(command),
        Command::Serve {
            policy,
            operator,
            state,
            port,
            allow_state_in_repository,
            max_artifacts,
            max_journal_bytes,
        } => {
            let options = trustgraph_cli::serve::Options {
                policy,
                operator,
                state,
                allow_state_in_repository,
                limits: trustgraph_cli::serve::Limits {
                    max_artifacts,
                    max_journal_bytes,
                    ..Default::default()
                },
            };
            trustgraph_cli::serve::main(options, port).await
        }
    };
    std::process::exit(code);
}
