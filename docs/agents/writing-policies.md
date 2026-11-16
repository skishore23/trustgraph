# Writing a TrustGraph policy

Read this before you write or change a `trustgraph.policy.yaml`. Every command below was run as written against `examples/policies/slack-digest.policy.yaml`. Build the binary with `cargo build -p trustgraph-cli`; it is `./target/debug/trustgraph`.

## 1. What a policy decides

A policy says where an AI agent's output may go, based on what the agent read. For each attempt it answers `allow`, `needs_approval` or `deny`. Two rules do all the work:

1. **Output readers are the intersection of every input's readers.** If an agent read two sources, only people who may read both may receive the output.
2. **Approval only adds a requirement; it never widens access.** Anything the policy does not name is denied. A reviewer must be allowed to read the output and cannot be the agent that asked.

## 2. One annotated example

```yaml
# Header comment: say what the policy is for, in two sentences.
version: 1                       # must be 1

groups:                          # named lists of people
  everyone: [maya, omar, sam, ravi, nina]
  leadership: [maya, omar]
  engineering: [sam, ravi]
  hr: [nina]

agents: [digest_bot]             # the agents this policy governs

sources:                         # data an agent may read
  handbook:
    readers: [everyone]          # who may receive anything derived from it
    agents: [digest_bot]         # which agents may use it as input
  eng_updates:
    readers: [engineering, leadership]
    agents: [digest_bot]
  comp_review:
    readers: [hr, leadership]
    agents: [digest_bot]

actions:                         # places an agent may send output
  post_all_hands:
    agents: [digest_bot]         # who may attempt it
    audience: [everyone]         # who receives the output
    approval: { reviewers: [hr], valid_for: 1h }   # optional; 30m, 2h, 1d
  post_eng_channel:
    agents: [digest_bot]
    audience: [engineering]

tests:                           # examples of what you expect
  - name: engineering notes can go to the engineering channel
    output_from: [eng_updates]   # what the agent read
    agent: digest_bot
    action: post_eng_channel
    expect: allow                # allow | needs_approval | deny
  - name: compensation data never reaches engineering
    output_from: [comp_review, eng_updates]
    agent: digest_bot
    action: post_eng_channel
    expect: deny
```

An action is allowed only when its audience is inside the output's readers, the agent is listed on the action, and the agent is listed on every source it read. Names must match `^[a-z0-9][a-z0-9_.@-]{0,63}$`. Unknown fields are errors. The optional `recipients` field on a test narrows an action's audience to a subset.

## 3. The loop

Work in this order. Do not skip steps.

1. **init** (new policy only): `trustgraph policy init --path trustgraph.policy.yaml`. It prints `Wrote <path>`. Use `--force` to overwrite. Pass `--json` for a JSON result.
2. **Edit** the file. Start from the starter or from a file in `examples/policies/` that matches your scenario.
3. **check**: `trustgraph policy check --json FILE`. Exit code 0 when `ok` is true, 1 otherwise. Fix every error, then read the warnings.

   ```json
   {
     "diagnostics": [
       {
         "code": "unknown_name",
         "message": "`leadrship` is not a group or person in this policy",
         "path": "sources.comp_review.readers[1]",
         "severity": "error",
         "suggestion": "did you mean `leadership`?"
       }
     ],
     "ok": false
   }
   ```

   `path` points into the file, `code` is stable, `suggestion` is `null` when there is none. Without `--json`, check prints `ok: 0 errors, 0 warnings` or lines like `error sources.comp_review.readers[1] unknown_name: ...`.
4. **test**: `trustgraph policy test --json FILE`. Exit code 1 if any test fails or the file has errors (then `results` is empty). Each result:

   ```json
   {
     "expect": "deny",
     "name": "compensation data never reaches engineering",
     "outcome": {
       "blocking": [
         { "excluded": ["ravi", "sam"], "readers": ["hr", "leadership"], "source": "comp_review" }
       ],
       "code": "audience_outside_readers",
       "readers": ["maya", "omar"],
       "reason": "denied: `comp_review` is readable by hr, leadership only; engineering (ravi, sam) is not included",
       "verdict": "deny"
     },
     "pass": true
   }
   ```

   The top level also has `diagnostics` and `ok`. `blocking` names the sources that hold the output back and the people they exclude. Plain output looks like `PASS <name>` or `FAIL <name>: expected deny, got allow — allowed`, then `5 passed, 0 failed`.
5. **simulate**: `trustgraph policy simulate FILE [--seed 0] [--count 64] [--steps 200] [--json]` runs the policy through seeded random scenarios on the real runtime (random agents, policy changes, crashes, approval expiry) and checks the safety invariants. Limits: `--count` 1-10000, `--steps` 1-5000, else exit 2. It exits 1 on any violation or if the file has errors. A clean run prints:

   ```text
   64 runs, 13,406 events: 2,461 allow, 3,432 deny, 913 needs_approval — 0 violations
   ```

   `--json` (here with `--count 2 --steps 30`) returns:

   ```json
   {
     "count": 2,
     "coverage": {
       "agent_denied_source": 0,
       "approval_expired_recheck": 0,
       "approved_allow": 0,
       "crash_clean": 2,
       "crash_lost_write": 2,
       "crash_torn_tail": 2,
       "crash_without_resend": 1,
       "expiry_boundary_at": 0,
       "expiry_boundary_before": 0,
       "explicit_recipients_check": 10,
       "identical_reactivation_with_live_approvals": 0,
       "narrowing_with_derived_outputs": 0,
       "recheck_after_identical_reactivation": 0,
       "revoke_effective": 0,
       "source_removed_with_derived_outputs": 0,
       "torn_activate": 0,
       "torn_approve": 0,
       "torn_revoke": 0
     },
     "decisions": { "allow": 14, "deny": 17, "needs_approval": 0 },
     "digest": "20c1962e51e9bca9b54bfad484011e87b8ac142b59d11dff4e14a7b42b07da52",
     "events": 61,
     "first_seed": 0,
     "ok": true,
     "steps": 30,
     "violations": []
   }
   ```

   `coverage` counts how often each kind of situation (crashes, approval expiry, revocation, and so on) occurred across the runs; a key at 0 means that path was not exercised, so raise `--count` or `--steps`. It appears in `--json` only.

   Each violation has `seed`, `invariant`, `step` and `detail`; human output adds a `replay:` line with the exact command to reproduce it. A violation means a bug in TrustGraph or an unsafe interaction worth reporting. Do not loosen the policy to make it go away. The same seed and options always give the same `digest`. `--seed` plus `--count` must not exceed 18446744073709551615. Argument-parsing errors (for example a non-numeric `--count`) are reported by the argument parser as plain text on stderr with exit code 2, even with `--json`.
6. **explain**: `trustgraph policy explain FILE` prints one plain-English line per source and action:

   ```text
   comp_review: output can reach hr (nina), leadership (maya, omar). Usable by agent digest_bot.
   post_all_hands: digest_bot may send to everyone (maya, nina, omar, ravi, sam) after approval by hr (nina), valid 1h.
   ```

   With `--json` it returns `{"ok": true, "text": "..."}`.
7. **diff**: `trustgraph policy diff OLD NEW` compares two files. With no change it prints `No changes.` A change that gives more people access prints a `WIDENS` line:

   ```text
   WIDENS  action post_eng_channel: audience gained maya, nina, omar
   ```

   `--json` returns `{"ok": true, "widens": true, "changes": [{"kind": "widens", "subject": "action post_eng_channel", "gained": ["maya","nina","omar"], "lost": [], "summary": "audience gained maya, nina, omar"}]}`. Use `git show HEAD:trustgraph.policy.yaml > old.yaml` to get OLD.
8. **Hand to a human**: give the file, the `explain` output and the `diff` output. Say which lines say `WIDENS`.

`trustgraph policy schema` prints the JSON Schema of the file (it is JSON already; `--json` is accepted).

## 4. Diagnostic codes

| Code | Meaning | Fix |
|---|---|---|
| `yaml_syntax` | The file is not valid YAML. | Fix indentation, quotes or brackets at the reported position. |
| `duplicate_key` | A key appears twice in one mapping (the later one would silently win). | Delete or rename one; merge the lists. |
| `too_large` | The file is over 262,144 bytes. | Split the policy by scenario or remove unused entries. |
| `too_many_errors` | More than 100 errors; the rest are hidden. | Fix the first ones and re-run. |
| `unsupported_version` | `version` is not 1. | Set `version: 1`. |
| `missing_field` | A required field is absent. | Add it at the reported `path`. |
| `unknown_field` | A key the format does not have. | Remove it or fix the spelling (see `suggestion`). |
| `wrong_type` | A value has the wrong shape, such as a string where a list belongs. | Use the type the schema shows. |
| `invalid_name` | A name does not match `^[a-z0-9][a-z0-9_.@-]{0,63}$`. | Use lowercase letters, digits and `_ . @ -`. |
| `name_collision` | A name is used both as an agent and as a group or person. | Rename one of them. |
| `empty_group` | A group has no members. | Add members or remove the group. |
| `empty_set` | A list that must have entries is empty. | Fill it. An empty `readers` or `audience` is never silently "nobody". |
| `unknown_name` | A reader, audience, reviewer, agent or source is not defined. | Define it, or use the name in `suggestion`. |
| `invalid_duration` | `valid_for` is not like `30m`, `2h` or `1d`, or is longer than 30 days. | Write a number and `m`, `h` or `d`. |
| `duplicate_test` | Two tests share a `name`. | Give each test a unique name. |
| `no_tests` (warning) | The policy has no tests. | Add tests (section 5). |
| `source_unusable` (warning) | No agent may use the source. | Add an agent to `agents`, or delete the source. |
| `action_unsatisfiable` (warning) | No single source can ever flow to the action's audience, so the action can never be allowed. | Check that some source's `readers` include the whole audience and that an agent of the action may use it. |
| `reviewers_cannot_read` (error) | No reviewer can read any source that could reach the action, so approval can never be given. | Choose reviewers who are readers of the data, or narrow the audience. Do not widen a source to fix this without a human. |
| `unused_group` (warning) | A group is never referenced. | Use it or delete it. |
| `unused_agent` (warning) | An agent is not allowed on any source or action. | Grant it something or delete it. |

Decision codes you will see in test results: `allowed`, `needs_approval`, `unknown_action`, `unknown_agent`, `unknown_source`, `empty_output`, `agent_not_listed_for_action`, `agent_cannot_use_source`, `recipients_outside_audience`, `audience_outside_readers`.

## 5. Writing good tests

Tests are how a human checks that you understood the request. Include:

- **A deny test for every sensitive source.** Pair it with the action it must never reach, and mix it with a harmless source to show one sensitive input is enough to block (`output_from: [comp_review, eng_updates]`).
- **A deny test for every action.** Show who or what the action must refuse: a source its audience cannot read, an agent not listed, or a source an agent may not use.
- **An allow test** for each legitimate path, so a policy that denies everything cannot pass.
- **A `needs_approval` test** for every action with an `approval` rule.
- Name each test as a sentence that states the rule ("compensation data never reaches engineering").
- Test the smallest case that proves the rule: one agent, the fewest sources.

## 6. Rules of the road

- **Agents propose; humans activate.** You write and test a policy file. A person reads it and turns it on. Never treat your own edit as live.
- **Never widen access to make a test pass.** If a test fails, first assume the policy is right and the test or your edit is wrong. Change a test's `expect` to a wider result only when the human asked for that wider access.
- **Flag every `WIDENS` line.** Quote it to the human with the people it adds and ask for a yes.
- **Protect the policy file with code review.** Put it behind CODEOWNERS and branch protection so a change needs a human owner's approval before merge.
- Do not remove tests to get a green run.

## 7. Known limits

- TrustGraph only knows the inputs the host reports. If an agent reads something and the host does not say so in `output_from`, the policy cannot account for it.
- The host must act on the decision. TrustGraph answers `allow`, `needs_approval` or `deny`; it does not send, block or approve anything itself.
- Group members are people by name. Make sure the names match the identities your host uses.
