use clap::{Parser, Subcommand, ValueEnum};
use std::{
    error::Error,
    path::{Path, PathBuf},
};
use trustgraph_core::bundle::{self, Bundle};
use trustgraph_sim::{Scenario, Trace};

#[derive(Parser)]
#[command(
    name = "trustgraph",
    version,
    about = "A Rust governance gateway for agent work"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Open the guided, deterministic company-governance demo. No sign-in or external effects.
    Story {
        #[arg(long, default_value_t = 9081)]
        port: u16,
    },
    /// Test a change against the current and proposed policies without applying changes.
    PolicySimulate {
        #[arg(long)]
        case: PathBuf,
        #[arg(long)]
        policy: Option<PathBuf>,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        url: String,
        #[arg(long, default_value = ".trustgraph-live/state/agent-token")]
        token_file: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long, value_parser = ["allowed", "blocked", "needs_approval"])]
        expect: Option<String>,
    },
    /// Submit real work through the restricted agent HTTP API and stream its activity.
    Run {
        #[arg(long)]
        goal: String,
        #[arg(long)]
        include_risk: bool,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        url: String,
        #[arg(long, default_value = ".trustgraph-live/state/agent-token")]
        token_file: PathBuf,
    },
    /// Start the live repository workspace with HydraDB and an authenticated coding agent.
    Start {
        #[arg(long, default_value = "trustgraph.local.json")]
        config: PathBuf,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// Allow (with a warning) a state directory inside the target or context repository.
        #[arg(long)]
        allow_state_in_repository: bool,
    },
    /// Compile a typed plan against the synthetic Northstar source snapshot.
    CompilePlan { file: Option<PathBuf> },
    /// Explore all reachable states in the declared finite authorization model.
    CheckModel {
        #[arg(long, default_value_t = 6)]
        depth: u8,
        #[arg(long)]
        broken_rule: bool,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Test the shared runtime under reproducible virtual schedules and faults.
    Simulate {
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long, default_value_t = 64)]
        count: u64,
        #[arg(long, value_enum)]
        scenario: Option<Case>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Replay saved historical evidence or a saved simulation campaign.
    Replay { file: PathBuf },
    /// Serve MCP over stdio by connecting to the running local TrustGraph app.
    Mcp {
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        url: String,
        #[arg(long, default_value = ".trustgraph-live/state/agent-token")]
        token_file: PathBuf,
    },
    /// Serve the restricted agent tools over authenticated HTTP for a hosted agent.
    McpHttp {
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        url: String,
        #[arg(long, default_value = ".trustgraph-live/state/agent-token")]
        token_file: PathBuf,
        /// Separate private 64-character hex capability for the remote MCP client.
        #[arg(long)]
        access_token_file: PathBuf,
        /// HTTPS origin of the tunnel; only this host and loopback hosts are accepted.
        #[arg(long)]
        public_url: Option<String>,
        #[arg(long, default_value_t = 8082)]
        port: u16,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Case {
    Demo,
    AclRace,
    RevocationRace,
    TimeoutRecovery,
    JournalRecovery,
    Expiry,
    AudienceChange,
    PolicyChange,
}
impl From<Case> for Scenario {
    fn from(c: Case) -> Self {
        match c {
            Case::Demo => Self::Demo,
            Case::AclRace => Self::AclRace,
            Case::RevocationRace => Self::RevocationRace,
            Case::TimeoutRecovery => Self::TimeoutRecovery,
            Case::JournalRecovery => Self::JournalRecovery,
            Case::Expiry => Self::Expiry,
            Case::AudienceChange => Self::AudienceChange,
            Case::PolicyChange => Self::PolicyChange,
        }
    }
}
fn save(path: &Path, value: &impl serde::Serialize) -> Result<(), Box<dyn Error>> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    match Cli::parse().command {
        Command::Story { port } => {
            let app = trustgraph_cli::story::router()?;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
            println!("TrustGraph guided demo: http://127.0.0.1:{port}");
            axum::serve(listener, app).await?;
        }
        Command::PolicySimulate {
            case,
            policy,
            url,
            token_file,
            output,
            json,
            expect,
        } => {
            trustgraph_cli::mcp::local_endpoint(&url)?;
            let mut request: serde_json::Value = serde_json::from_slice(&std::fs::read(case)?)?;
            if !request.is_object() {
                return Err("simulation case must be a JSON object".into());
            }
            if let Some(policy) = policy {
                request["policy"] = serde_json::from_slice(&std::fs::read(policy)?)?;
            }
            let token =
                trustgraph_cli::mcp::read_token(&trustgraph_cli::mcp::token_path(&token_file))?;
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let response = client
                .post(format!("{}/api/simulate", url.trim_end_matches('/')))
                .bearer_auth(token.trim())
                .json(&request)
                .send()
                .await?;
            let status = response.status();
            let bytes = response.bytes().await?;
            let result: serde_json::Value = serde_json::from_slice(&bytes).map_err(
                |_| "simulation request was rejected; check the case and supported policy schema",
            )?;
            if !status.is_success() {
                return Err(result["error"]
                    .as_str()
                    .unwrap_or("simulation failed")
                    .to_owned()
                    .into());
            }
            if let Some(path) = output {
                save(&path, &result)?;
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "Current policy: {} → {}\nProposed policy: {} → {}",
                    result["active_policy"]["name"]
                        .as_str()
                        .unwrap_or("current"),
                    result["baseline"]["status"].as_str().unwrap_or("unknown"),
                    result["candidate_policy"]["name"]
                        .as_str()
                        .unwrap_or("proposed"),
                    result["status"].as_str().unwrap_or("unknown")
                );
                if let Some(reason) = result["counterexample"]["explanation"].as_str() {
                    println!("Reason: {reason}");
                }
                println!(
                    "Hypothetical assumptions only · policy not activated · code not executed · publication effects: 0\nHydraDB evidence: {}",
                    result["graph_id"].as_str().unwrap_or("missing")
                );
            }
            if let Some(expected) = expect
                && result["status"].as_str() != Some(expected.as_str())
            {
                return Err(format!("expected {expected}, got {}", result["status"]).into());
            }
        }
        Command::Run {
            goal,
            include_risk,
            url,
            token_file,
        } => {
            trustgraph_cli::mcp::local_endpoint(&url)?;
            let token =
                trustgraph_cli::mcp::read_token(&trustgraph_cli::mcp::token_path(&token_file))?;
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let response = client
                .post(format!("{}/api/work", url.trim_end_matches('/')))
                .bearer_auth(token.trim())
                .json(&serde_json::json!({"goal":goal,"include_risk":include_risk}))
                .send()
                .await?;
            let status = response.status();
            let body: serde_json::Value = response.json().await?;
            if !status.is_success() {
                return Err(body["error"]
                    .as_str()
                    .unwrap_or("work submission failed")
                    .to_owned()
                    .into());
            }
            let id = body["id"].as_str().ok_or("work ID missing")?.to_owned();
            println!("Work accepted: {id}");
            let mut seen = 0;
            for _ in 0..360 {
                let response = client
                    .get(format!("{}/api/work", url.trim_end_matches('/')))
                    .bearer_auth(token.trim())
                    .send()
                    .await?
                    .error_for_status()?;
                let state: serde_json::Value = response.json().await?;
                let job = state["jobs"]
                    .as_array()
                    .and_then(|jobs| jobs.iter().find(|j| j["id"] == id))
                    .ok_or("work item missing")?;
                if let Some(events) = job["events"].as_array() {
                    for event in events.iter().skip(seen) {
                        println!(
                            "[{}] {}",
                            event["stage"].as_str().unwrap_or("work"),
                            event["message"].as_str().unwrap_or("")
                        );
                    }
                    seen = events.len();
                }
                let stage = job["status"].as_str().unwrap_or("");
                if !["queued", "running"].contains(&stage) {
                    println!(
                        "Status: {stage}\nReview: {}/agent/{id}",
                        url.trim_end_matches('/')
                    );
                    if stage == "blocked" || stage == "interrupted" {
                        return Err(job["error"]
                            .as_str()
                            .unwrap_or("work stopped")
                            .to_owned()
                            .into());
                    }
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            return Err("work is still running; inspect its status without resubmitting".into());
        }
        Command::Start {
            config,
            port,
            allow_state_in_repository,
        } => {
            let config: trustgraph_live::Config = serde_json::from_slice(&std::fs::read(config)?)?;
            // Bind first so the single-use launch link names the actual port.
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
            let started = trustgraph_live::router(
                config,
                trustgraph_live::Options {
                    port,
                    allow_state_in_repository,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            println!(
                "TrustGraph workspace: http://127.0.0.1:{port}\nOperator sign-in (single use, this start only): {}",
                started.launch_file.display()
            );
            axum::serve(listener, started.router).await?;
        }
        Command::CompilePlan { file } => {
            let plan = match file {
                Some(file) => serde_json::from_slice(&std::fs::read(file)?)?,
                None => trustgraph_core::plan::launch_plan(
                    "weekly-launch-update",
                    true,
                    "company".into(),
                ),
            };
            let evidence = trustgraph_runtime::fixtures::northstar();
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"scope":"synthetic Northstar snapshot","plan":plan,"analysis":trustgraph_core::plan::analyze(&evidence,&plan,&"agent".into()),"repairs":trustgraph_core::plan::repairs(&evidence,&plan,&"agent".into())})
                )?
            );
        }
        Command::CheckModel {
            depth,
            broken_rule,
            output,
        } => {
            let report = trustgraph_sim::checker::check(
                depth,
                if broken_rule {
                    trustgraph_sim::checker::Rule::ReuseApprovalMutant
                } else {
                    trustgraph_sim::checker::Rule::Production
                },
            )?;
            if let Some(path) = output {
                save(&path, &report)?;
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.passed {
                return Err("a counterexample was found within the declared model bounds".into());
            }
        }
        Command::Simulate {
            seed,
            count,
            scenario,
            output,
        } => {
            if count == 0 || count > 10_000 {
                return Err("count must be between 1 and 10000".into());
            }
            let end = seed.checked_add(count).ok_or("seed range overflow")?;
            let scenarios = scenario
                .map(|c| vec![c.into()])
                .unwrap_or_else(|| Scenario::ALL.to_vec());
            let mut traces = vec![];
            for case in &scenarios {
                for current in seed..end {
                    traces.push(trustgraph_sim::run(current, *case)?);
                }
            }
            if let Some(path) = output {
                save(&path, &serde_json::json!({"schema":1,"traces":traces}))?;
            }
            println!(
                "Passed {} scenario runs across {} seeds; every observed payload matches its approved evidence.",
                traces.len(),
                count
            );
        }
        Command::Replay { file } => {
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
            if value.get("traces").is_some() {
                let traces: Vec<Trace> = serde_json::from_value(value["traces"].clone())?;
                if traces.is_empty() {
                    return Err("empty campaign".into());
                }
                for trace in &traces {
                    if trustgraph_sim::run(trace.seed, trace.scenario)? != *trace {
                        return Err("simulation trace mismatch".into());
                    }
                }
                println!("Replayed {} deterministic scenario traces.", traces.len());
            } else if value.get("scheduler").is_some() {
                let trace: Trace = serde_json::from_value(value)?;
                if trustgraph_sim::run(trace.seed, trace.scenario)? != trace {
                    return Err("simulation trace mismatch".into());
                }
                println!("Simulation trace verified.");
            } else {
                let bundle: Bundle = serde_json::from_value(value)?;
                let verified = bundle::replay(&bundle)?;
                println!(
                    "Historical decision verified: {} → {}",
                    verified.context().artifact,
                    verified.context().audience.destination
                );
            }
        }
        Command::Mcp { url, token_file } => {
            trustgraph_cli::mcp::stdio_bridge(&url, &token_file).await?;
        }
        Command::McpHttp {
            url,
            token_file,
            access_token_file,
            public_url,
            port,
        } => {
            let app = trustgraph_cli::mcp::http_router(
                &url,
                &token_file,
                &access_token_file,
                public_url.as_deref(),
            )?;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
            println!("TrustGraph agent MCP: http://127.0.0.1:{port}/mcp");
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
    }
    Ok(())
}
