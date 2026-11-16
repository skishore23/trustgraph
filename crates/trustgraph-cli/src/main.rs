use clap::{Args, Parser, Subcommand};
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
    Serve(ServeArgs),
    /// Serve, plus one MCP endpoint between agents and their MCP servers: it holds
    /// the servers' credentials, records what each agent reads, and checks every
    /// action before forwarding it. Agents use /gateway/mcp with their token.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Gateway {
        #[command(subcommand)]
        command: Option<GatewayCommand>,
        /// The gateway config: upstream MCP servers and what each exposed tool is.
        #[arg(long, required = true)]
        config: Option<PathBuf>,
        #[command(flatten)]
        serve: ServeArgs,
    },
}

#[derive(Subcommand)]
enum GatewayCommand {
    /// Relay MCP between stdin/stdout and a running gateway, for MCP clients that
    /// can only launch a command. It adds nothing but transport.
    Stdio {
        /// The gateway's MCP endpoint.
        #[arg(long, default_value = "http://127.0.0.1:8787/gateway/mcp")]
        url: String,
        /// This agent's token file, from the gateway's state directory.
        #[arg(long)]
        token_file: PathBuf,
        /// Allow a plain http:// URL to a host that is not loopback (the token travels in clear).
        #[arg(long)]
        allow_insecure: bool,
    },
    /// Validate a gateway config against a policy and say what it does.
    Check {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        policy: PathBuf,
    },
}

#[derive(Args)]
struct ServeArgs {
    /// The policy file. It is applied on the first start only; later, a changed
    /// file is stored as a proposal for the operator.
    #[arg(long, required = true)]
    policy: Option<PathBuf>,
    /// Apply the policy file at start even when it differs from the active policy.
    #[arg(long)]
    activate_policy_file: bool,
    /// The person who approves, revokes and activates; must be in a group of the policy.
    #[arg(long, required = true)]
    operator: Option<String>,
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
    /// Journal bytes each agent may cause for the life of the state directory.
    #[arg(long, default_value_t = trustgraph_cli::serve::DEFAULT_AGENT_JOURNAL_BYTES)]
    agent_journal_bytes: u64,
    /// Artifacts each agent may create for the life of the state directory.
    #[arg(long, default_value_t = trustgraph_cli::serve::DEFAULT_AGENT_ARTIFACTS)]
    agent_artifacts: u64,
}

impl ServeArgs {
    fn options(self) -> (trustgraph_cli::serve::Options, u16) {
        let options = trustgraph_cli::serve::Options {
            // clap requires both unless a gateway subcommand is given.
            policy: self.policy.unwrap_or_default(),
            operator: self.operator.unwrap_or_default(),
            state: self.state,
            allow_state_in_repository: self.allow_state_in_repository,
            activate_policy_file: self.activate_policy_file,
            limits: trustgraph_cli::serve::Limits {
                max_artifacts: self.max_artifacts,
                max_journal_bytes: self.max_journal_bytes,
                agent_journal_bytes: self.agent_journal_bytes,
                agent_artifacts: self.agent_artifacts,
                ..Default::default()
            },
        };
        (options, self.port)
    }
}

#[tokio::main]
async fn main() {
    use trustgraph_cli::serve::gateway;
    let code = match Cli::parse().command {
        Command::Policy { command } => trustgraph_cli::policy_cmd::run(command),
        Command::Serve(args) => {
            let (options, port) = args.options();
            trustgraph_cli::serve::main(options, port).await
        }
        Command::Gateway {
            command:
                Some(GatewayCommand::Stdio {
                    url,
                    token_file,
                    allow_insecure,
                }),
            ..
        } => gateway::bridge::run(&url, &token_file, allow_insecure).await,
        Command::Gateway {
            command: Some(GatewayCommand::Check { config, policy }),
            ..
        } => gateway::check(&config, &policy).await,
        Command::Gateway {
            command: None,
            config,
            serve,
        } => {
            let (options, port) = serve.options();
            gateway::main(options, config.unwrap_or_default(), port).await
        }
    };
    std::process::exit(code);
}
