use std::fs;
use std::path::Path;

use trustgraph_engine::{
    Event, FileJournal, Journal, JournalError, MemoryJournal, OpenError, Record, Reply, Runtime,
};
use trustgraph_policy::Verdict;

const SPEC: &str = include_str!("../../trustgraph-policy/tests/fixtures/spec-example.policy.yaml");

fn sha(n: u32) -> String {
    format!("{n:064x}")
}

/// The spec example with `roadmap` readable by hr only, and its tests updated.
fn narrowed() -> String {
    SPEC.replace("readers: [hr, engineering]", "readers: [hr]")
        .replace(
            "action: post_to_slack\n    expect: allow",
            "action: post_to_slack\n    expect: deny",
        )
}

fn file_journal(dir: &Path) -> FileJournal {
    FileJournal::open(dir).expect("open journal")
}

fn text(dir: &Path) -> String {
    fs::read_to_string(dir.join("journal.jsonl")).unwrap()
}

fn artifact(reply: Reply) -> String {
    match reply {
        Reply::Artifact { id } => id,
        other => panic!("expected an artifact, got {other:?}"),
    }
}

fn check_event(artifact: &str) -> Event {
    Event::Check {
        agent: "planner".into(),
        artifact: artifact.into(),
        action: "post_to_slack".into(),
        recipients: None,
    }
}

fn verdict(reply: &Reply) -> Verdict {
    match reply {
        Reply::Decision { verdict, .. } => *verdict,
        other => panic!("expected a decision, got {other:?}"),
    }
}

/// Ten events; returns the roadmap-derived output's artifact id.
fn ten_events<J: Journal>(rt: &mut Runtime<J>) -> String {
    let mut at = 1;
    let mut send = |rt: &mut Runtime<J>, event: Event| {
        at += 1;
        rt.handle(at, event).unwrap()
    };
    let roadmap = artifact(send(
        rt,
        Event::RegisterSource {
            source: "roadmap".into(),
            sha256: sha(2),
        },
    ));
    let salaries = artifact(send(
        rt,
        Event::RegisterSource {
            source: "salaries".into(),
            sha256: sha(1),
        },
    ));
    let output = |inputs: Vec<String>, n| Event::RecordOutput {
        agent: "planner".into(),
        step: "summarize".into(),
        inputs,
        sha256: sha(n),
    };
    let only = artifact(send(rt, output(vec![roadmap.clone()], 10)));
    let both = artifact(send(rt, output(vec![roadmap, salaries], 11)));
    assert_eq!(verdict(&send(rt, check_event(&only))), Verdict::Allow);
    assert_eq!(verdict(&send(rt, check_event(&both))), Verdict::Deny);
    let unknown = send(rt, check_event("nope"));
    assert!(matches!(unknown, Reply::Rejected { .. }));
    send(
        rt,
        Event::Revoke {
            decision: "nope".into(),
            reviewer: "nina".into(),
        },
    );
    send(rt, check_event(&only));
    send(rt, check_event(&both));
    only
}

#[test]
fn reopen_reaches_the_same_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
    ten_events(&mut rt);
    let digest = rt.engine().state_digest();
    drop(rt);

    let rt = Runtime::open(file_journal(dir.path()), SPEC, 100).unwrap();
    assert_eq!(rt.engine().state_digest(), digest);
    let genesis = text(dir.path())
        .lines()
        .filter(|l| l.contains("\"type\":\"genesis\""))
        .count();
    assert_eq!(genesis, 1);
}

#[test]
fn new_policy_file_appends_genesis() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
    let only = ten_events(&mut rt);
    assert_eq!(rt.engine().policy_revision(), 1);
    drop(rt);

    let mut rt = Runtime::open(file_journal(dir.path()), &narrowed(), 100).unwrap();
    assert_eq!(rt.engine().policy_revision(), 2);
    let reply = rt.handle(101, check_event(&only)).unwrap();
    assert_eq!(verdict(&reply), Verdict::Deny);
}

fn register_roadmap() -> Event {
    Event::RegisterSource {
        source: "roadmap".into(),
        sha256: sha(2),
    }
}

#[test]
fn failed_append_applies_nothing() {
    let rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    let before = rt.engine().state_digest();
    let mut journal = rt.into_journal();
    // The policy matches, so `open` appends nothing and the failure stays
    // armed for the first `handle`.
    journal.fail_next_append();
    let mut rt = Runtime::open(journal, SPEC, 0).unwrap();
    assert!(matches!(
        rt.handle(1, register_roadmap()),
        Err(JournalError::Io(_))
    ));
    assert_eq!(rt.engine().state_digest(), before);
    // The runtime stays usable, and the failed event left no trace.
    assert!(matches!(
        rt.handle(1, register_roadmap()),
        Ok(Reply::Artifact { .. })
    ));
    let journal = rt.into_journal();
    assert_eq!(journal.records().len(), 2);
}

#[test]
fn second_open_is_locked() {
    let dir = tempfile::tempdir().unwrap();
    let first = file_journal(dir.path());
    assert_eq!(
        FileJournal::open(dir.path()).unwrap_err(),
        JournalError::Locked
    );
    drop(first);
    FileJournal::open(dir.path()).unwrap();
}

#[test]
fn torn_tail_is_dropped() {
    for tail in ["{\"seq\":99,\"at\":1,\"ev", "not json", "{"] {
        let dir = tempfile::tempdir().unwrap();
        let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
        ten_events(&mut rt);
        let digest = rt.engine().state_digest();
        drop(rt);
        let good = text(dir.path());
        fs::write(dir.path().join("journal.jsonl"), format!("{good}{tail}")).unwrap();

        let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 100).unwrap();
        assert_eq!(rt.engine().state_digest(), digest, "tail {tail:?}");
        assert_eq!(text(dir.path()), good, "the file is truncated: {tail:?}");
        // and appending after the truncation continues the chain
        rt.handle(101, register_roadmap()).unwrap();
        drop(rt);
        Runtime::open(file_journal(dir.path()), SPEC, 200).unwrap();
    }
}

#[test]
fn corrupt_middle_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
    for n in 0..5 {
        rt.handle(
            n + 1,
            Event::RegisterSource {
                source: "roadmap".into(),
                sha256: sha(n as u32 + 2),
            },
        )
        .unwrap();
    }
    drop(rt);
    let content = text(dir.path());
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 6);
    let at = lines[2].find("\"at\":").unwrap() + 5;
    let digit = lines[2].as_bytes()[at];
    assert!(digit.is_ascii_digit());
    let flipped = if digit == b'9' { "8" } else { "9" };
    lines[2].replace_range(at..at + 1, flipped);
    fs::write(dir.path().join("journal.jsonl"), lines.join("\n") + "\n").unwrap();

    match Runtime::open(file_journal(dir.path()), SPEC, 100) {
        Err(OpenError::Journal(JournalError::Corrupt { seq: 3, .. })) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn seq_gap_and_oversize_line_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let genesis = Event::Genesis {
        policy: SPEC.into(),
    };
    let first = Record::seal(1, 0, genesis, &"0".repeat(64));
    let third = Record::seal(3, 0, register_roadmap(), &first.hash);
    let lines = format!(
        "{}\n{}\n",
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&third).unwrap()
    );
    fs::write(dir.path().join("journal.jsonl"), lines).unwrap();
    assert!(matches!(
        FileJournal::open(dir.path()).unwrap().load(),
        Err(JournalError::Corrupt { seq: 2, .. })
    ));

    let dir = tempfile::tempdir().unwrap();
    let huge = "x".repeat((1 << 20) + 10);
    fs::write(dir.path().join("journal.jsonl"), format!("{huge}\n")).unwrap();
    assert!(matches!(
        FileJournal::open(dir.path()).unwrap().load(),
        Err(JournalError::Corrupt { seq: 1, .. })
    ));
}

fn sealed_journal_dir(records: &[(u64, Event)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut prev = "0".repeat(64);
    let mut out = String::new();
    for (seq, (at, event)) in records.iter().enumerate() {
        let record = Record::seal(seq as u64 + 1, *at, event.clone(), &prev);
        prev = record.hash.clone();
        out.push_str(&serde_json::to_string(&record).unwrap());
        out.push('\n');
    }
    fs::write(dir.path().join("journal.jsonl"), out).unwrap();
    dir
}

fn genesis_event() -> Event {
    Event::Genesis {
        policy: SPEC.into(),
    }
}

#[test]
fn reordered_time_fails_closed() {
    let dir = sealed_journal_dir(&[(5, genesis_event()), (3, register_roadmap())]);
    match Runtime::open(file_journal(dir.path()), SPEC, 10) {
        Err(OpenError::Replay { seq: 2, .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn invalid_policy_file_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    match Runtime::open(file_journal(dir.path()), "version: 1\nnot: [valid", 0) {
        Err(OpenError::InvalidPolicy(d)) => assert!(!d.is_empty()),
        other => panic!("{other:?}"),
    }
    assert_eq!(text(dir.path()), "", "nothing is journaled");

    let failing = SPEC.replace("expect: deny", "expect: allow");
    match Runtime::open(MemoryJournal::new(), &failing, 0) {
        Err(OpenError::InvalidPolicy(d)) => {
            assert!(d.iter().any(|d| d.code == "test_failed"), "{d:?}")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn rejected_and_check_events_are_journaled_and_replayed() {
    let mut rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    let sheet = artifact(
        rt.handle(
            1,
            Event::RegisterSource {
                source: "price_sheet".into(),
                sha256: sha(3),
            },
        )
        .unwrap(),
    );
    let out = artifact(
        rt.handle(
            2,
            Event::RecordOutput {
                agent: "mailer".into(),
                step: "draft".into(),
                inputs: vec![sheet],
                sha256: sha(10),
            },
        )
        .unwrap(),
    );
    let rejected = rt.handle(7, check_event("missing")).unwrap();
    assert!(matches!(rejected, Reply::Rejected { .. }));
    let pending = rt
        .handle(
            9,
            Event::Check {
                agent: "mailer".into(),
                artifact: out,
                action: "email_customer".into(),
                recipients: None,
            },
        )
        .unwrap();
    assert_eq!(verdict(&pending), Verdict::NeedsApproval);
    let digest = rt.engine().state_digest();
    let journal = rt.into_journal();
    assert_eq!(journal.records().len(), 5, "genesis + 4 handled events");

    let rt = Runtime::open(journal, SPEC, 0).unwrap();
    assert_eq!(rt.engine().state_digest(), digest);
}

#[test]
fn genesis_through_handle_is_refused() {
    let mut rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    let digest = rt.engine().state_digest();
    let reply = rt.handle(5, genesis_event()).unwrap();
    assert!(matches!(&reply, Reply::Rejected { code, .. } if code == "not_allowed"));
    assert_eq!(rt.engine().state_digest(), digest);
    assert_eq!(rt.into_journal().records().len(), 1);
}

#[test]
fn clock_regression_is_not_journaled() {
    let mut rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    rt.handle(10, register_roadmap()).unwrap();
    let digest = rt.engine().state_digest();
    let reply = rt.handle(4, register_roadmap()).unwrap();
    assert!(matches!(&reply, Reply::Rejected { code, .. } if code == "clock_regressed"));
    assert_eq!(rt.engine().state_digest(), digest);
    assert!(matches!(
        rt.handle(10, register_roadmap()),
        Ok(Reply::Artifact { .. })
    ));
    let journal = rt.into_journal();
    assert_eq!(journal.records().len(), 3, "genesis + two accepted events");
    let rt = Runtime::open(journal, SPEC, 0).unwrap();
    assert_eq!(rt.engine().state_digest(), digest);
}

#[test]
fn append_rejects_a_forked_chain() {
    let genesis = Record::seal(1, 0, genesis_event(), &"0".repeat(64));
    let forks = [
        Record::seal(1, 1, register_roadmap(), &genesis.hash), // repeated seq
        Record::seal(2, 1, register_roadmap(), &"f".repeat(64)), // wrong prev
        Record::seal(3, 1, register_roadmap(), &genesis.hash), // gap
        Record {
            hash: "0".repeat(64),
            ..Record::seal(2, 1, register_roadmap(), &genesis.hash)
        }, // bad hash
    ];

    let mut memory = MemoryJournal::new();
    memory.append(&genesis).unwrap();
    for fork in &forks {
        assert!(matches!(
            memory.append(fork),
            Err(JournalError::Corrupt { .. })
        ));
    }
    assert_eq!(memory.records().len(), 1);

    let dir = tempfile::tempdir().unwrap();
    let mut file = file_journal(dir.path());
    file.load().unwrap();
    file.append(&genesis).unwrap();
    for fork in &forks {
        assert!(matches!(
            file.append(fork),
            Err(JournalError::Corrupt { .. })
        ));
    }
    drop(file);
    assert_eq!(text(dir.path()).lines().count(), 1);
}

#[test]
fn append_before_load_on_a_non_empty_file_is_an_error() {
    let dir = sealed_journal_dir(&[(0, genesis_event())]);
    let mut file = file_journal(dir.path());
    let next = Record::seal(2, 1, register_roadmap(), "x");
    assert_eq!(
        file.append(&next),
        Err(JournalError::Io("journal not loaded".into()))
    );
}

#[test]
fn memory_journal_tear_tail_then_reopen() {
    let mut rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    ten_events(&mut rt);
    let before = rt.engine().state_digest();
    rt.handle(500, register_roadmap()).unwrap();
    assert_ne!(rt.engine().state_digest(), before);
    let mut journal = rt.into_journal();
    journal.tear_tail();

    let rt = Runtime::open(journal, SPEC, 0).unwrap();
    assert_eq!(rt.engine().state_digest(), before);
}

#[test]
fn first_record_must_be_genesis() {
    let mut journal = MemoryJournal::new();
    journal
        .append(&Record::seal(1, 0, register_roadmap(), &"0".repeat(64)))
        .unwrap();
    match Runtime::open(journal, SPEC, 0) {
        Err(OpenError::Replay { seq: 1, .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn file_permissions_are_private() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("state");
        let _journal = file_journal(&sub);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&sub), 0o700);
        assert_eq!(mode(&sub.join("journal.jsonl")), 0o600);
    }
}

#[test]
fn terminated_bad_final_line_is_corrupt_not_torn() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
    rt.handle(1, register_roadmap()).unwrap();
    drop(rt);
    let good = text(dir.path());

    // Terminated but unparseable.
    fs::write(
        dir.path().join("journal.jsonl"),
        format!("{good}not json\n"),
    )
    .unwrap();
    assert!(matches!(
        FileJournal::open(dir.path()).unwrap().load(),
        Err(JournalError::Corrupt { seq: 3, .. })
    ));

    // Valid JSON, wrong hash.
    let mut last: Record = serde_json::from_str(good.lines().last().unwrap()).unwrap();
    last.hash = "0".repeat(64);
    let mut lines: Vec<String> = good.lines().map(str::to_string).collect();
    *lines.last_mut().unwrap() = serde_json::to_string(&last).unwrap();
    fs::write(dir.path().join("journal.jsonl"), lines.join("\n") + "\n").unwrap();
    assert!(matches!(
        FileJournal::open(dir.path()).unwrap().load(),
        Err(JournalError::Corrupt { seq: 2, .. })
    ));
}

#[test]
fn a_later_rejected_genesis_does_not_brick_the_journal() {
    let bad = Event::Genesis {
        policy: "not: [yaml".into(),
    };
    let dir = sealed_journal_dir(&[(0, genesis_event()), (1, bad.clone())]);
    let rt = Runtime::open(file_journal(dir.path()), SPEC, 5).unwrap();
    assert_eq!(rt.engine().policy_revision(), 1);
    assert_eq!(text(dir.path()).lines().count(), 2, "nothing appended");
    drop(rt);

    // A rejected FIRST record is still fatal.
    let dir = sealed_journal_dir(&[(0, bad)]);
    assert!(matches!(
        Runtime::open(file_journal(dir.path()), SPEC, 5),
        Err(OpenError::Replay { seq: 1, .. })
    ));
}

fn activate_in_band(rt: &mut Runtime<MemoryJournal>, at: u64, policy: &str) {
    let id = match rt
        .handle(
            at,
            Event::ProposePolicy {
                agent: None,
                policy: policy.into(),
            },
        )
        .unwrap()
    {
        Reply::Proposed { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let reply = rt
        .handle(
            at,
            Event::ActivatePolicy {
                proposal: id,
                operator: "leo".into(),
            },
        )
        .unwrap();
    assert!(matches!(reply, Reply::Activated { .. }), "{reply:?}");
}

#[test]
fn restart_keeps_an_in_band_activation() {
    let mut rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    activate_in_band(&mut rt, 1, &narrowed());
    assert_eq!(rt.engine().policy_revision(), 2);
    // An approval issued after the activation.
    let sheet = artifact(
        rt.handle(
            2,
            Event::RegisterSource {
                source: "price_sheet".into(),
                sha256: sha(3),
            },
        )
        .unwrap(),
    );
    let out = artifact(
        rt.handle(
            3,
            Event::RecordOutput {
                agent: "mailer".into(),
                step: "draft".into(),
                inputs: vec![sheet],
                sha256: sha(10),
            },
        )
        .unwrap(),
    );
    let email = Event::Check {
        agent: "mailer".into(),
        artifact: out,
        action: "email_customer".into(),
        recipients: None,
    };
    let decision = match rt.handle(4, email.clone()).unwrap() {
        Reply::Decision { id, verdict, .. } => {
            assert_eq!(verdict, Verdict::NeedsApproval);
            id
        }
        other => panic!("{other:?}"),
    };
    let approved = rt
        .handle(
            5,
            Event::Approve {
                decision,
                reviewer: "leo".into(),
            },
        )
        .unwrap();
    assert!(matches!(approved, Reply::Approved { .. }), "{approved:?}");
    let digest = rt.engine().state_digest();
    let active = rt.engine().policy().unwrap().digest();
    let journal = rt.into_journal();
    let len = journal.records().len();

    // The ORIGINAL file again: it did not change, so nothing is appended.
    let rt = Runtime::open(journal, SPEC, 100).unwrap();
    assert_eq!(rt.engine().state_digest(), digest);
    assert_eq!(rt.engine().policy().unwrap().digest(), active);
    assert_eq!(rt.engine().policy_revision(), 2);
    let journal = rt.into_journal();
    assert_eq!(journal.records().len(), len);

    // A reformatted copy of the same policy is not a change either.
    let reformatted = format!("# edited\n\n{SPEC}\n\n# end\n");
    let rt = Runtime::open(journal, &reformatted, 100).unwrap();
    let journal = rt.into_journal();
    assert_eq!(journal.records().len(), len);

    // A different file wins: one new Genesis, revision + 1.
    let third = SPEC.replace("valid_for: 30m", "valid_for: 20m");
    assert_ne!(third, SPEC);
    let rt = Runtime::open(journal, &third, 100).unwrap();
    assert_eq!(rt.engine().policy_revision(), 3);
    assert_eq!(rt.into_journal().records().len(), len + 1);
}

fn rejected_code(reply: &Reply) -> &str {
    match reply {
        Reply::Rejected { code, .. } => code,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

/// Agent input that breaks a field bound is refused before the journal sees
/// it: nothing is written, the clock does not move and the state is unchanged.
#[test]
fn shape_rejected_events_are_not_journaled() {
    let mut rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    let roadmap = artifact(rt.handle(1, register_roadmap()).unwrap());
    let digest = rt.engine().state_digest();
    let bad = [
        (
            Event::ProposePolicy {
                agent: Some("planner".into()),
                policy: format!("{SPEC}#{}", "x".repeat(trustgraph_policy::MAX_BYTES)),
            },
            "invalid_policy",
        ),
        (
            Event::RecordOutput {
                agent: "planner".into(),
                step: "summarize".into(),
                inputs: vec![roadmap.clone(); 65],
                sha256: sha(10),
            },
            "invalid_inputs",
        ),
        (
            Event::Check {
                agent: "a".repeat(10_000),
                artifact: roadmap.clone(),
                action: "post_to_slack".into(),
                recipients: None,
            },
            "invalid_field",
        ),
        (
            Event::RegisterSource {
                source: "roadmap".into(),
                sha256: "not hex".into(),
            },
            "invalid_digest",
        ),
    ];
    for (at, (event, code)) in (100..).zip(bad) {
        let reply = rt
            .handle(at, event)
            .expect("a bad shape is not an I/O error");
        assert_eq!(rejected_code(&reply), code);
        assert_eq!(rt.engine().state_digest(), digest, "{code}");
    }
    // The clock did not move: an event at time 2 is still accepted.
    assert!(matches!(
        rt.handle(2, register_roadmap()),
        Ok(Reply::Artifact { .. })
    ));
    let journal = rt.into_journal();
    assert_eq!(journal.records().len(), 3, "genesis + two registrations");
}

/// A policy of escaped control characters just under the size limit encodes
/// to a journal line past the line cap. That is bad input, so the runtime
/// answers `Rejected` (or accepts it whole), never an I/O error.
#[test]
fn control_character_policy_near_the_limit_never_errors() {
    let filler = trustgraph_policy::MAX_BYTES - SPEC.len() - 1;
    let policy = format!("{SPEC}#{}", "\u{1}".repeat(filler));
    assert_eq!(policy.len(), trustgraph_policy::MAX_BYTES);
    let event = Event::ProposePolicy {
        agent: Some("planner".into()),
        policy,
    };

    let mut rt = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    let digest = rt.engine().state_digest();
    let reply = rt.handle(1, event.clone()).expect("never an I/O error");
    let journaled = rt.into_journal().records().len();
    match reply {
        Reply::Rejected { .. } => {
            assert_eq!(journaled, 1, "a rejected oversize record is not written")
        }
        Reply::Proposed { .. } => assert_eq!(journaled, 2),
        other => panic!("{other:?}"),
    }

    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
    let reply = rt.handle(1, event).expect("never an I/O error");
    if matches!(reply, Reply::Rejected { .. }) {
        assert_eq!(rt.engine().state_digest(), digest);
    }
    // The runtime is still healthy afterwards.
    assert!(matches!(
        rt.handle(2, register_roadmap()),
        Ok(Reply::Artifact { .. })
    ));
}

#[test]
fn policy_source_survives_reopen_until_the_file_changes() {
    use trustgraph_engine::PolicySource;
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
    assert_eq!(rt.policy_source(), PolicySource::File);
    let proposal = match rt
        .handle(
            1,
            Event::ProposePolicy {
                agent: None,
                policy: narrowed(),
            },
        )
        .unwrap()
    {
        Reply::Proposed { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let activated = rt
        .handle(
            2,
            Event::ActivatePolicy {
                proposal,
                operator: "leo".into(),
            },
        )
        .unwrap();
    assert!(matches!(activated, Reply::Activated { revision: 2, .. }));
    assert_eq!(rt.policy_source(), PolicySource::Activated);
    assert_eq!(rt.last_at(), 2);
    let size = rt.journal().len_bytes().unwrap();
    assert_eq!(size, text(dir.path()).len() as u64);
    drop(rt);

    let rt = Runtime::open(file_journal(dir.path()), SPEC, 10).unwrap();
    assert_eq!(rt.policy_source(), PolicySource::Activated, "same file");
    assert_eq!(rt.engine().policy_revision(), 2);
    drop(rt);

    let rt = Runtime::open(file_journal(dir.path()), &narrowed(), 20).unwrap();
    assert_eq!(
        rt.policy_source(),
        PolicySource::File,
        "a changed file wins"
    );
    assert_eq!(rt.engine().policy_revision(), 3);
}

#[test]
fn memory_journal_size_matches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = Runtime::open(file_journal(dir.path()), SPEC, 0).unwrap();
    let mut memory = Runtime::open(MemoryJournal::new(), SPEC, 0).unwrap();
    file.handle(1, register_roadmap()).unwrap();
    memory.handle(1, register_roadmap()).unwrap();
    assert_eq!(
        file.journal().len_bytes().unwrap(),
        memory.journal().len_bytes()
    );
}
