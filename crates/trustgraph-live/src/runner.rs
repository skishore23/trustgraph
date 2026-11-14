use crate::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub fn run(
    command: &mut Command,
    input: Option<&str>,
    directory: &Path,
    seconds: u64,
) -> Result<(i32, String)> {
    fs::create_dir_all(directory)?;
    let out = directory.join(format!("output-{}.log", Uuid::new_v4()));
    let file = private_file(&out)?;
    command
        .stdout(Stdio::from(file.try_clone()?))
        .stderr(Stdio::from(file));
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command.spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or_else(|| err("child stdin unavailable"))?
            .write_all(input.as_bytes())?;
    }
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.code().unwrap_or(-1), fs::read_to_string(out)?));
        }
        if started.elapsed() > Duration::from_secs(seconds) || fs::metadata(&out)?.len() > 2_000_000
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err("process exceeded its time/output bound"));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

pub fn generate(prompt: &str, dir: &Path) -> Result<(AgentOutput, Value)> {
    let cwd = dir.join("isolated-context");
    fs::create_dir_all(&cwd)?;
    let schema = json!({"type":"object","additionalProperties":false,"required":["summary","files"],"properties":{"summary":{"type":"string"},"files":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["path","content"],"properties":{"path":{"type":"string"},"content":{"type":"string"}}}}}});
    let schema_path = dir.join("output-schema.json");
    fs::write(&schema_path, serde_json::to_vec(&schema)?)?;
    let result_path = dir.join("agent-output.json");
    let mut c = Command::new("codex");
    c.args([
        "exec",
        "--ignore-user-config",
        "--ignore-rules",
        "--ephemeral",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--json",
        "--color",
        "never",
    ])
    .args([
        "-c",
        "features.shell_tool=false",
        "-c",
        "suppress_unstable_features_warning=true",
        "-c",
        "features.unified_exec=false",
        "-c",
        "features.multi_agent=false",
        "-c",
        "features.apps=false",
        "-c",
        "features.skip_host_skill_discovery=true",
        "-c",
        "features.skill_search=false",
        "-c",
        "features.shell_snapshot=false",
        "-c",
        "project_doc_max_bytes=0",
        "-c",
        "web_search=\"disabled\"",
    ])
    .arg("--output-schema")
    .arg(schema_path)
    .arg("--output-last-message")
    .arg(&result_path)
    .arg("-")
    .current_dir(cwd);
    for key in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "HYDRADB_API_KEY",
        "TRUSTGRAPH_HYDRA_TOKEN",
        "COMPOSIO_API_KEY",
    ] {
        c.env_remove(key);
    }
    let (code, events) = run(&mut c, Some(prompt), dir, 240)?;
    if code != 0 {
        return Err(err(&format!(
            "Codex did not complete (exit {code}); inspect the private run log"
        )));
    }
    let mut complete = false;
    let mut usage = Value::Null;
    for line in events.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v["type"] == "turn.completed" {
                complete = true;
                usage = v["usage"].clone();
            }
            if v["type"] == "turn.failed" || v["type"] == "error" {
                return Err(err("Codex reported a failed turn"));
            }
            if let Some(kind) = v["item"]["type"].as_str()
                && !["agent_message", "reasoning"].contains(&kind)
            {
                return Err(err(
                    "agent used an uncaptured tool; lineage is not accepted",
                ));
            }
        }
    }
    if !complete {
        return Err(err("Codex completion event is missing"));
    }
    let bytes = fs::read(result_path)?;
    if bytes.len() > 131_072 {
        return Err(err("agent output too large"));
    }
    Ok((
        serde_json::from_slice(&bytes)?,
        json!({"provider":"codex-cli","usage":usage,"capture":"broker supplied sources and instructions; fresh turn; external tools disabled","events_hash":digest::content(events.as_bytes())}),
    ))
}

pub fn test(
    files: &[ChangedFile],
    base: &[ChangedFile],
    dir: &Path,
    patch: &Digest,
) -> Result<CheckReceipt> {
    if !cfg!(target_os = "macos") {
        return Err(err(
            "a configured isolated test runner is required on this platform",
        ));
    }
    let work = dir.join("checks");
    fs::create_dir_all(work.join("src"))?;
    for f in base.iter().chain(files) {
        if f.path == "src/lib.rs" {
            fs::write(work.join("src/lib.rs"), &f.content)?;
        }
    }
    let rustc = Command::new("rustup").args(["which", "rustc"]).output()?;
    if !rustc.status.success() {
        return Err(err("rustc unavailable"));
    }
    let rustc = String::from_utf8(rustc.stdout)?.trim().to_owned();
    let toolchain = Path::new(&rustc)
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| err("invalid Rust toolchain"))?;
    let realwork = fs::canonicalize(&work)?;
    let ancestors: BTreeSet<_> = [toolchain, &realwork]
        .into_iter()
        .flat_map(|p| p.ancestors().skip(1))
        .map(|p| {
            format!(
                "(literal {})",
                serde_json::to_string(&p.to_string_lossy()).expect("path string")
            )
        })
        .collect();
    let directory_reads = ancestors.into_iter().collect::<Vec<_>>().join(" ");
    let profile = format!(
        "(version 1) (deny default) (allow file-read-metadata) (allow process*) (allow sysctl-read) (allow mach-lookup) (allow file-read* {directory_reads} (subpath \"/usr\") (subpath \"/System\") (subpath \"/Library\") (subpath \"{}\") (subpath \"{}\") (literal \"/dev/null\") (literal \"/dev/urandom\")) (allow file-write* (subpath \"{}\") (literal \"/dev/null\"))",
        toolchain.display(),
        realwork.display(),
        realwork.display()
    );
    let mut compile = Command::new("/usr/bin/sandbox-exec");
    compile
        .args([
            "-p",
            &profile,
            &rustc,
            "--edition=2024",
            "--test",
            "src/lib.rs",
            "-o",
            "test-binary",
        ])
        .current_dir(&work)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", &realwork);
    let (compile_code, compile_output) = run(&mut compile, None, &work, 60)?;
    if compile_code != 0 {
        return Ok(CheckReceipt {
            patch_digest: patch.clone(),
            passed: false,
            command: "rustc --edition=2024 --test src/lib.rs; ./test-binary".into(),
            output: format!("Compiler exit {compile_code}\n{compile_output}"),
        });
    }
    let mut tests = Command::new("/usr/bin/sandbox-exec");
    tests
        .args(["-p", &profile])
        .arg(realwork.join("test-binary"))
        .current_dir(&work)
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    let (code, output) = run(&mut tests, None, &work, 30)?;
    Ok(CheckReceipt {
        patch_digest: patch.clone(),
        passed: code == 0,
        command: "rustc --edition=2024 --test src/lib.rs; ./test-binary".into(),
        output: format!("{compile_output}{output}"),
    })
}
