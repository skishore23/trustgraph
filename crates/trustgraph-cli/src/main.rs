use clap::{Parser, Subcommand, ValueEnum};
use std::{
    error::Error,
    path::{Path, PathBuf},
};
use trustgraph_core::bundle::{self, Bundle};
use trustgraph_runtime::{hydra::HydraClient, projection};
use trustgraph_sim::{Scenario, Trace};

#[derive(Parser)]
#[command(
    name = "trustgraph",
    version,
    about = "A Rust governance gateway for synthetic agent work"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Compile a typed plan against the synthetic Northstar source snapshot.
    CompilePlan { file: Option<PathBuf> },
    /// Validate a small bank policy and run five isolated authorization cases.
    PolicyCheck { file: PathBuf },
    /// Explore all reachable states in the declared finite authorization model.
    CheckModel {
        #[arg(long, default_value_t = 6)]
        depth: u8,
        #[arg(long)]
        broken_rule: bool,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Run both policy failures and the successful controlled publication.
    Demo {
        #[arg(long)]
        export: Option<PathBuf>,
        #[arg(long)]
        json: bool,
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
    /// Serve a synthetic review desk on loopback. No real publication occurs.
    Serve {
        #[arg(long)]
        data: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "bank")]
        scenario: AppScenario,
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// Serve MCP over stdio by connecting to the running local TrustGraph app.
    Mcp {
        #[arg(long, default_value = "http://127.0.0.1:8080/mcp")]
        url: String,
        #[arg(long, default_value = ".trustgraph-bank/mcp-token")]
        token_file: PathBuf,
    },
    /// Write and validate an evidence graph in an explicitly owned Hydra ID range.
    HydraRoundtrip {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8443")]
        url: String,
        #[arg(long, default_value = "default")]
        graph: String,
        #[arg(long, default_value = "cell-0")]
        cell: String,
        #[arg(long)]
        base_id: u64,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum AppScenario {
    Bank,
    Consulting,
    Northstar,
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
        Command::PolicyCheck { file } => {
            let yaml = std::fs::read_to_string(file)?;
            let mut w = trustgraph_runtime::Coordinator::open_with_evidence(
                trustgraph_runtime::journal::MemoryJournal::default(),
                trustgraph_runtime::publisher::LocalPublisher::default(),
                trustgraph_runtime::fixtures::bank(),
            )?;
            let version = w.propose_policy(&"agent".into(), &yaml, 0)?;
            let report = trustgraph_runtime::policy::test_policy(
                &w.state().evidence,
                &version.spec,
                &w.state().source_authority,
                &w.state().destination_authority,
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.passed {
                return Err("policy did not satisfy the five bank cases".into());
            }
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
        Command::Demo { export, json } => {
            let (trace, bundles) = trustgraph_sim::run_with_bundles(7, Scenario::Demo)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&trace)?);
            } else {
                println!("TrustGraph — synthetic local demonstration");
                for step in &trace.steps {
                    println!(
                        "t={:<4} {:<48} {}",
                        step.at,
                        format!("{:?}", step.action),
                        serde_json::to_string(&step.outcome)?
                    );
                }
                println!(
                    "{} controlled publication; no external messages sent.",
                    trace.publications
                );
            }
            if let Some(path) = export {
                save(&path, bundles.last().ok_or("no publication bundle")?)?;
                println!("Evidence exported to {}", path.display());
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
        Command::Serve {
            data,
            port,
            scenario,
        } => {
            let data = data.unwrap_or_else(|| {
                PathBuf::from(match scenario {
                    AppScenario::Bank => ".trustgraph-bank",
                    AppScenario::Consulting => ".trustgraph-consulting",
                    AppScenario::Northstar => ".trustgraph",
                })
            });
            let app = match scenario {
                AppScenario::Bank => trustgraph_cli::api::bank_router(&data)?,
                AppScenario::Consulting => trustgraph_cli::api::consulting_router(&data)?,
                AppScenario::Northstar => trustgraph_cli::api::router(&data)?,
            };
            let listener =
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
            println!("TrustGraph review desk: http://{}", listener.local_addr()?);
            println!(
                "Synthetic identities and publishing only. Journal: {}",
                data.display()
            );
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
        Command::HydraRoundtrip {
            bundle,
            url,
            graph,
            cell,
            base_id,
        } => {
            let bundle: Bundle = serde_json::from_slice(&std::fs::read(bundle)?)?;
            let token = std::env::var("TRUSTGRAPH_HYDRA_TOKEN")
                .map_err(|_| "set TRUSTGRAPH_HYDRA_TOKEN for your development server")?;
            let expected = projection::build(&bundle, base_id)?;
            let namespace =
                std::env::var("TRUSTGRAPH_HYDRA_NAMESPACE").unwrap_or_else(|_| "default".into());
            let mut hydra =
                HydraClient::new(&url, &graph, &cell, &token)?.with_namespace(&namespace)?;
            hydra.project(&expected).await?;
            let loaded = hydra.load_bundle(&bundle.digest, base_id).await?;
            if loaded != bundle {
                return Err("Hydra readback reconstructed a different bundle".into());
            }
            println!(
                "Hydra roundtrip passed: {} immutable records, {} typed references; completion marker {}.",
                expected.nodes.len(),
                expected.edges.len(),
                expected.marker.id
            );
        }
    }
    Ok(())
}
