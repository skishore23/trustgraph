use crate::*;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
};

/// Start `command` as the leader of a new process group so a timeout can stop
/// every descendant, not only the direct child.
fn isolate_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}
/// SIGKILL the whole process group led by `child`, then the child itself.
/// Call this before reaping the child so its PID cannot have been reused.
fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-s", "KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
/// Feed stdin from a separate thread so a child that never reads cannot stall
/// the caller past its deadline. The pipe closes when the bytes are written or
/// the child dies.
fn feed(child: &mut Child, input: Option<Vec<u8>>) -> Result<()> {
    if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| err("child stdin unavailable"))?;
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    Ok(())
}

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
    isolate_group(command);
    let mut child = command.spawn()?;
    let started = std::time::Instant::now();
    if let Err(e) = feed(&mut child, input.map(|s| s.as_bytes().to_vec())) {
        kill_group(&mut child);
        return Err(e);
    }
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.code().unwrap_or(-1), fs::read_to_string(out)?));
        }
        if started.elapsed() > Duration::from_secs(seconds) || fs::metadata(&out)?.len() > 2_000_000
        {
            kill_group(&mut child);
            return Err(err("process exceeded its time/output bound"));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Run a short external command (e.g. `gh`) with piped, size-capped stdout, a
/// hard deadline and process-group cleanup. Stderr is discarded so provider
/// error bodies and credentials are never echoed.
pub fn bounded(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    max_output: usize,
) -> Result<(i32, Vec<u8>)> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    isolate_group(command);
    let mut child = command.spawn()?;
    let deadline = std::time::Instant::now() + timeout;
    if let Err(e) = feed(&mut child, input) {
        kill_group(&mut child);
        return Err(e);
    }
    let Some(stdout) = child.stdout.take() else {
        kill_group(&mut child);
        return Err(err("child stdout unavailable"));
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut bytes = Vec::new();
        let result = stdout
            .take(max_output as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    loop {
        if let Some(status) = child.try_wait()? {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let bytes = receiver
                .recv_timeout(remaining.max(Duration::from_millis(100)))
                .map_err(|_| err("process output did not close before its deadline"))??;
            if bytes.len() > max_output {
                return Err(err("process output exceeded its bound"));
            }
            return Ok((status.code().unwrap_or(-1), bytes));
        }
        if std::time::Instant::now() > deadline {
            kill_group(&mut child);
            return Err(err("process exceeded its time bound"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Environment variables passed to Codex. Everything else, including every
/// TRUSTGRAPH_* capability and publication credential, is cleared.
const CODEX_ENVIRONMENT: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "CODEX_HOME",
    "CODEX_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
];
fn codex_environment(
    vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    vars.into_iter()
        .filter(|(k, _)| k.to_str().is_some_and(|k| CODEX_ENVIRONMENT.contains(&k)))
        .collect()
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
    .current_dir(cwd)
    .env_clear()
    .envs(codex_environment(std::env::vars_os()));
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
    let quoted = |p: &Path| serde_json::to_string(&p.to_string_lossy()).expect("path string");
    // The linker driver is located outside the sandbox. Only standard developer
    // locations are accepted; an unexpected location simply fails the compile.
    let developer = Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|p| fs::canonicalize(p.trim()).ok())
        .filter(|p| p.starts_with("/Library/Developer/") || p.starts_with("/Applications/"))
        .map(|p| format!("(subpath {})", quoted(&p)))
        .unwrap_or_default();
    let reads = format!(
        "(allow file-read* {directory_reads} (subpath \"/usr\") (subpath \"/System\") (subpath \"/Library\") (subpath {}) (subpath {}) (literal \"/dev/null\") (literal \"/dev/urandom\")) (allow file-write* (subpath {}) (literal \"/dev/null\"))",
        quoted(toolchain),
        quoted(&realwork),
        quoted(&realwork)
    );
    // No blanket process* or mach-lookup: compilation may fork and exec only the
    // Rust toolchain and the system linker driver; the test binary may exec only
    // itself and cannot fork, so it cannot launch open, osascript or other tools.
    let compile_profile = format!(
        "(version 1) (deny default) (allow file-read-metadata) (allow sysctl-read) (allow process-fork) (allow process-exec (subpath {}) (literal \"/usr/bin/cc\") (literal \"/usr/bin/ld\") (literal \"/usr/bin/xcrun\") {developer}) {reads}",
        quoted(toolchain)
    );
    let test_profile = format!(
        "(version 1) (deny default) (allow file-read-metadata) (allow sysctl-read) (allow process-exec (literal {})) {reads}",
        quoted(&realwork.join("test-binary"))
    );
    let mut compile = Command::new("/usr/bin/sandbox-exec");
    compile
        .args([
            "-p",
            &compile_profile,
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
        .args(["-p", &test_profile])
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn alive(pid: &str) -> bool {
        Command::new("/bin/kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn timeout_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("grandchild.pid");
        let mut c = Command::new("/bin/sh");
        c.arg("-c")
            .arg(format!("sleep 30 & echo $! > {}; wait", pidfile.display()));
        let started = std::time::Instant::now();
        assert!(run(&mut c, None, dir.path(), 1).is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = fs::read_to_string(&pidfile).unwrap();
        let pid = pid.trim();
        for _ in 0..50 {
            if !alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("forked grandchild {pid} survived the timeout");
    }

    #[test]
    fn unread_stdin_cannot_outlive_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let input = "x".repeat(4 * 1024 * 1024);
        let started = std::time::Instant::now();
        let mut c = Command::new("/bin/sleep");
        c.arg("20");
        assert!(run(&mut c, Some(&input), dir.path(), 1).is_err());
        let mut c = Command::new("/bin/sleep");
        c.arg("20");
        assert!(
            bounded(
                &mut c,
                Some(input.into_bytes()),
                Duration::from_secs(1),
                1024
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn bounded_commands_return_capped_output() {
        let mut c = Command::new("/bin/echo");
        c.arg("hello");
        let (code, out) = bounded(&mut c, None, Duration::from_secs(5), 64).unwrap();
        assert_eq!((code, out.as_slice()), (0, b"hello\n".as_slice()));
        let mut c = Command::new("/bin/cat");
        assert!(bounded(&mut c, Some(vec![b'y'; 200]), Duration::from_secs(5), 64).is_err());
    }

    #[test]
    fn codex_receives_only_allowlisted_environment() {
        let vars = [
            ("PATH", "/usr/bin"),
            ("HOME", "/Users/me"),
            ("CODEX_HOME", "/Users/me/.codex"),
            ("TRUSTGRAPH_HYDRA_TOKEN", "secret"),
            ("TRUSTGRAPH_AGENT_TOKEN", "secret"),
            ("TRUSTGRAPH_OWNER", "secret"),
            ("GH_TOKEN", "secret"),
            ("GITHUB_TOKEN", "secret"),
            ("COMPOSIO_API_KEY", "secret"),
            ("HYDRADB_API_KEY", "secret"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
        ]
        .map(|(k, v)| (k.into(), v.into()));
        let kept: Vec<_> = codex_environment(vars)
            .into_iter()
            .map(|(k, _)| k.into_string().unwrap())
            .collect();
        assert_eq!(kept, ["PATH", "HOME", "CODEX_HOME"]);
    }
}
