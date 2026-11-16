# trustgraph gateway

`trustgraph gateway` sits between agents and the MCP servers they use. It
holds those servers' credentials, records what each agent reads, and checks
every action against what was read before it forwards the call. An agent
cannot skip it as long as two things hold: the gateway alone holds the
upstream credentials, and the agent has no other route to the upstream
servers. If an agent has the credentials, or can reach a server some other
way, the gateway does not see what it does there.

The gateway is [`trustgraph serve`](serve.md) plus one MCP endpoint,
`/gateway/mcp`. It is the same process, with the same state directory, agent
tokens, operator page, HTTP API and limits. Everything it records goes
through the same runtime and journal.

## Protected version 2

Run `./examples/typed-gateway/run.sh` from the repository root. It builds the
fake MCP server and runs the real HTTP/stdio gateway test, including independent
upstream call-log assertions. It exits nonzero on failure and prints typed
ancestry, denied forwarding, authorized tasks, and restart behavior.
The runnable [configuration](../examples/typed-gateway/gateway.yaml) and
[policy](../examples/typed-gateway/policy.yaml) are the test fixtures themselves.

Version 2 requires durable `session: agent` context (also its default). Every
upstream tool has a capability manifest:

| Capability | Supported semantics |
| --- | --- |
| `source` | A fixed source with no outbound arguments. Requires `reads` and `otherwise`. |
| `read_and_sink` | All outbound arguments require a policy `egress` action. The response is registered and carried before disclosure. Supports `reads`, or tracked `passthrough` classified as `unclassified`. |
| `sink` | Requires `acts`, an argument allowlist, and `destination_arguments`. Each destination argument must have an exact matcher in every rule. The upstream response is withheld and replaced by a local acknowledgement. |
| `local_pure` | Reserved for implemented local transformations. Rejected on upstream MCP tools; an upstream's annotation does not establish purity. |
| `disabled` | Hidden and uncallable. This is the default when a version 2 tool omits its capability. |

Protected decisions also bind to the tool manifest and a keyed fingerprint of
the resolved upstream endpoint, command, environment and account credentials.
Changing that deployment context invalidates old approvals without exposing
credential hashes. Review adapter code upgrades and revoke outstanding approvals
when their semantics change; the fingerprint does not attest executable bytes.

An outbound call with no recorded context is denied. Bootstrap a task with a
fixed zero-argument source. `allow_without_reads` is unavailable for protected
sinks. A search query, URL, header, or path can contain secrets, so a read tool
with arguments is `read_and_sink`, even if its name sounds harmless.
An `egress` action describes the actual service receiving **all** its arguments,
including URLs it dereferences and any secondary recipients. Tools with dynamic
secondary destinations require a trusted service adapter or must be disabled.

Without a resource-specific attestation, protected reads always use
`otherwise`, even when a request path matches a more permissive rule. Choose a
fallback at least as restrictive as every resource accessible through that tool.
The gateway cannot authenticate a resource merely from its supplied path or from
an untrusted MCP response claiming that it is public.

### Typed execution

A read tool can declare `schema: SourceCode@1` and a local transform:

```yaml
transform:
  implementation: first_line_v1
  input: SourceCode@1
  output: Summary@1
```

`first_line_v1` requires one successful text content item and returns its first
line. This deliberately small deterministic transform is not an LLM summarizer
or a declassifier. The source and output are registered in the existing
`trustgraph-framework::Graph`, through journaled runtime events. Input schemas
must match exactly. Changing the output schema never widens its readers.

Schemas are versioned nominal contracts, not JSON Schema payload validators.
The trusted adapter establishes the source type. Opaque agent outputs infer the
schemas of all carried inputs and retain every dependency. Their default output
type is `any`; typing alone conveys no trust or clearance.

Typed read results expose graph evidence in `_meta.trustgraph`; denied actions
include it in `structuredContent.error.lineage`. The artifact explanation API
also exposes typed lineage. Evidence contains identities, schemas, operation
contracts, and edges; the engine stores content digests rather than payloads.
The gateway executes the transform and supplies its receipt; the receipt is a
host attestation, not a proof certificate.

Old events omit optional schema/contract fields. They still serialize and hash
as before, replay as `any`, and preserve existing source/output identities.
Typed records replay the registered contracts, artifacts, and decisions.
Connection-scoped contexts present during an upgrade are merged into their
agent's durable context before retirement; an impossible merge fails startup.

### Protected preview threat model

The tested deployment is the local fake GitHub MCP adapter. It has fixed source
tools, two immutable repository IDs, and no secondary destinations. The test
client intentionally follows an injected instruction; there is no probabilistic
prompt-injection detector and no claim about any model's behavior. The independent
upstream log proves the tested denied calls did not run. This is a reproducible
execution test, not a production GitHub integration or a comparative benchmark.

Trusted components are the gateway/host, policy administrator, authenticated
identity assignment, service adapters and their resource/audience semantics,
OS isolation, and durable state. The agent, arguments, and tool-response prose
are untrusted. Configuration cannot make a destination identity canonical by
assertion: the adapter must resolve and use that exact immutable identity.
The fake adapter does this; arbitrary Slack aliases, GitHub names, redirects,
webhooks, search plugins and other production tool semantics are not certified.

For a protected deployment, the operator must arrange:

- Separate OS identities, containers, or an equivalent boundary for gateway and
  agent. The agent must not read gateway environment, process memory, policy,
  journal, signing keys, operator launch files, or other agents' token files.
- Upstream credentials available only to the gateway. Give each agent only its
  own gateway token. Filesystem permissions on a shared OS identity do not
  isolate that agent from the gateway.
- Deny direct agent network routes to protected APIs and other exfiltration
  destinations. A gateway does not itself install a network sandbox.
- Sandbox untrusted MCP subprocesses separately, restricting their filesystem
  and network access. Environment scrubbing and a private HOME are not process
  isolation; an MCP subprocess otherwise has the gateway's OS privileges.
- Keep the journal/state directory durable and outside agent workspaces. Startup
  fails on corrupt state, but deletion/truncation of an entire journal cannot be
  detected without an external trusted head anchor.
- Resume using the same authenticated agent identity. Hand-offs must pass through
  the authenticated gateway token flow before information is disclosed. The host
  must not give an agent another principal's token or restore confidential memory
  into an unrelated fresh identity. Missing/invalid hand-off tokens block action;
  out-of-band information exchange remains outside gateway observation.

Within that scope, tested guarantees are authorization before protected tool
forwarding, denial without an upstream call, reader non-widening through typed
transforms, argument/destination-bound approvals, and durable inherited context.
External policy changes are not an atomic transaction with a remote API call.
An upstream timeout can have an unknown side-effect outcome; it is not a policy
denial. Retirement by the trusted operator means the host must discard the
corresponding model context and memory; it does not erase what a model remembers.

Integrity labels, privileged argument influence tracking, field-level dependency
verification, declassification, and persistent-memory/checkpoint SDK guarantees
remain future work. Session-wide tracking is intentionally conservative and may
deny an unrelated public write after a confidential read. Bounded kernel proofs
do not prove gateway coverage or host isolation.

## Legacy version 1 quickstart

The remaining version 1 configuration examples describe legacy behavior.
Version 1 can send read/passthrough arguments without an egress check and can
use connection-scoped context. Use version 2 for the protected preview above.


The example in [examples/gateway](../examples/gateway) runs with no external
services. Its upstreams are a small fake MCP server. Run it from the
repository root:

```sh
cargo build -p trustgraph-cli --bins --examples
SLACK_BOT_TOKEN=demo-slack-token-0001 target/debug/trustgraph gateway \
  --policy examples/gateway/gateway.policy.yaml --operator nina \
  --config examples/gateway/gateway.yaml --state ~/tg-gateway
```

```text
warning: tools.slack.post_message.acts[0].arguments: this rule's `when` does not constrain: text; each is hashed into the checked output and shown to the approver, but can hold anything, so it must not be able to choose who the call reaches
upstream `slack`: fake_upstream: starting with credential [redacted]
trustgraph gateway: upstream `slack`: connected, 12 tools
trustgraph gateway: upstream `drive`: connected, 12 tools
TrustGraph gateway: http://127.0.0.1:8787
MCP endpoint: http://127.0.0.1:8787/gateway/mcp
Operator sign-in (single use): /Users/you/tg-gateway/operator.html
Agent tokens: /Users/you/tg-gateway/tokens
Policy in force: revision 1 (from the policy file), digest a95856e0f71a
Operator: nina
warning: the policy file is inside the Git repository /Users/you/TrustGraph; agents working there can edit it. Changes to it never apply without the operator.
Sessions: connection; tools exposed: 4
```

The fake `slack` server logs its own credential on purpose; the gateway
replaces every credential value in what it prints. In another terminal, two
agents use the gateway. The researcher reads the salary file and hands off.
The planner receives the hand-off, reads the roadmap and tries to post, then
tries to slip an extra argument past the check:

```sh
target/debug/examples/gateway_demo --tokens ~/tg-gateway/tokens
```

```text
researcher drive__read_file HR/Compensation/salaries.csv: nina: 120k, sam: 95k
researcher trustgraph__handoff: a hand-off token
planner trustgraph__receive: {"carries":["salaries"],"received":true}
planner drive__read_file Product/roadmap.md: Q3: ship the new billing page
planner slack__post_message #engineering: error: TrustGraph gateway: `post_to_engineering` was not forwarded: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included (denied)
planner slack__post_message #engineering, plus Channel #all-company: error: TrustGraph gateway: `Channel` is not an argument the gateway forwards for this tool (it forwards: channel, text); the call was not forwarded (invalid_arguments)
```

Neither `post_message` call reached the chat server. The warning is
expected: `text` is the message, which the approver reads; it does not
choose who the post reaches (`channel` does, and the rule matches it
exactly). Stop the gateway with Ctrl-C or SIGTERM: it stops its stdio
upstreams and removes their private `HOME` directories.

Check a config against a policy before you start the gateway. When the
variables an upstream needs are set, `gateway check` also connects to it
and compares the arguments the config forwards with the ones the upstream
tool takes. Connecting to a stdio upstream means **running its configured
`command`** on this machine; pass `--offline` to check only the config and
the policy (the upstream checks are then skipped, and the output says so):

```sh
SLACK_BOT_TOKEN=demo-slack-token-0001 trustgraph gateway check \
  --config examples/gateway/gateway.yaml --policy examples/gateway/gateway.policy.yaml
```

```text
upstream `slack`: fake_upstream: starting with credential [redacted]
Sessions: connection (each MCP connection is one session; it assumes the host opens a new connection per task with a fresh agent context, and that the agent cannot reconnect at will: a client that reconnects by itself starts with an empty session. Use `session: agent` when the host cannot guarantee that).
Upstreams: drive (runs target/debug/examples/fake_upstream), slack (runs target/debug/examples/fake_upstream).
Agents see 4 tools; every other upstream tool is invisible and uncallable:
  drive__read_file reads a source; its result is recorded before it is returned:
    when `path` matches `HR/Compensation/**`: `salaries` (usable by planner, researcher)
    when `path` matches `Product/**`: `roadmap` (usable by planner)
    when `path` matches `Public/**`: `benchmarks` (usable by planner, researcher)
    otherwise: `unmatched` (usable by planner)
    forwards only the arguments: path; a call with any other is refused
  slack__post_message is an action, checked against everything the session read before it is forwarded; the output checked is every forwarded argument:
    when `channel` is exactly `#engineering`: `post_to_engineering`, content `text` (reaches ravi, sam)
    otherwise: blocked, never forwarded
    a call from a session that has read nothing is refused (nothing_read)
    forwards only the arguments: channel, text; a call with any other is refused
    upstream arguments NOT forwardable: blocks, thread
  trustgraph__handoff and trustgraph__receive pass what a session carries to another agent's session
warning: tools.slack.post_message.acts[0].arguments: this rule's `when` does not constrain: text; each is hashed into the checked output and shown to the approver, but can hold anything, so it must not be able to choose who the call reaches
```

`gateway check` runs each stdio upstream whose variables are set here, to
ask for its tools, so run it where you would run the gateway, and only on a
config whose commands you trust; with `--offline` it runs nothing.

## Connecting agents

Each agent connects to one endpoint, `http://127.0.0.1:<port>/gateway/mcp`
(streamable HTTP), with its own token from `<state>/tokens/`. The gateway
knows an agent only by its token.

```json
{
  "mcpServers": {
    "gateway": {
      "type": "http",
      "url": "http://127.0.0.1:8787/gateway/mcp",
      "headers": { "Authorization": "Bearer <contents of tokens/planner.token>" }
    }
  }
}
```

For an MCP client that can only launch a command, `trustgraph gateway stdio`
relays between its stdin and stdout and the gateway. It adds nothing but
transport. It refuses a token file that its group or other users can read,
and a plain `http://` URL to a host that is not loopback unless you pass
`--allow-insecure` (the token would travel in clear):

```json
{
  "mcpServers": {
    "gateway": {
      "command": "trustgraph",
      "args": ["gateway", "stdio", "--url", "http://127.0.0.1:8787/gateway/mcp",
               "--token-file", "/path/to/state/tokens/planner.token"]
    }
  }
}
```

The gateway offers MCP protocol versions 2025-11-25, 2025-06-18 and
2025-03-26, which have sessions.

## The config

`gateway.yaml` is strict. An unknown key is an error, and every problem is
reported at once, with its path. The gateway does not start with a config
that has a problem, with one that names a source or action the active
policy does not have, or with one whose arguments a reachable upstream does
not take.

```yaml
version: 1
session: agent                 # agent or connection (the default); see Sessions
max_result_bytes: 1048576      # largest upstream tool result (default 1 MiB)
timeout_seconds: 30            # every upstream call (default 30)
upstreams:
  drive:
    url: https://drive.example.com/mcp          # streamable HTTP
    headers: { Authorization: "Bearer ${env:DRIVE_TOKEN}" }
  slack:
    command: [npx, -y, "@modelcontextprotocol/server-slack"]   # stdio
    env: { SLACK_BOT_TOKEN: "${env:SLACK_BOT_TOKEN}" }
tools:
  drive.read_file:
    reads:                      # forwards only `path`, the argument the rules name
      - when: { path: "HR/Compensation/**" }
        source: salaries
      - when: { path: "Product/**" }
        source: roadmap
    otherwise: unmatched        # required: a source at least as restrictive as every rule's
  slack.post_message:
    arguments: [channel, text]  # required for acts: a call with any other argument is refused
    acts:
      - when: { channel: "#engineering" }   # exact: the value must be `#engineering`
        action: post_to_engineering
        content: text           # shown to the operator; the check covers every argument
    otherwise: block            # the only value: an unmatched call is never forwarded
  slack.send_email:
    arguments: [to, subject, body]
    acts:
      - when: { to: { pattern: "*@corp.com", type: email } }   # a strict address at corp.com
        action: email_staff
        content: body
  drive.search:
    passthrough: true
    arguments: [query]
```

- **Upstreams.** An upstream has a `url` (streamable HTTP) or a `command`
  (a program and its arguments, run with stdio), not both. Names are
  lowercase letters, digits, `-` and single `_`; `trustgraph` is reserved.
  A URL may not hold a user name or password; put credentials in `headers`.
  The gateway and `gateway check` warn about a plain `http://` URL to a host
  that is not loopback: its credentials and every call travel in clear.
- **Secrets.** `${env:NAME}` is the only interpolation, and only in `headers`
  and `env` values. A variable that is not set, or whose value is shorter
  than 12 characters, stops the gateway from starting: every such value is
  treated as a secret and redacted by its text, which is meaningless for a
  short one. The gateway never prints, logs, returns or journals a resolved
  value. An upstream result that contains one, as is or JSON-escaped, is
  withheld. A stdio upstream gets the variables in its `env`, a minimal
  `PATH` (the program's own directory, `/usr/bin` and `/bin`) and a private,
  empty `HOME` directory, `$TMPDIR/trustgraph-upstream-*` (mode 0700). It is
  removed when the connection ends, and when the gateway exits on SIGINT or
  SIGTERM, which stops every stdio upstream first. Beside each one is a lock
  file (`….lock`) the gateway keeps locked while it uses the directory. One
  that a killed gateway left behind (owned by your user, lock file present
  and not locked) is removed when a gateway next starts; one whose lock a
  running gateway holds, or one with no lock file, is never touched. The
  child never gets the gateway's `HOME` or the rest of its environment.
  Directories left by versions before the lock file existed
  (`$TMPDIR/trustgraph-upstream-*` with no lock file) are never removed
  automatically; delete them by hand when no gateway is running.
  That is not isolation: see [Threat model](#threat-model). Its stderr is
  printed with every secret value replaced by `[redacted]`.
- **Tools.** Only the tools listed under `tools:` exist for agents. They are
  named `upstream.tool` in the config, and agents see them as
  `upstream__tool`. Every other tool on an upstream is invisible and
  uncallable. There is no switch that exposes unlisted tools.
- **arguments.** The only argument names a call to the tool may carry. A
  call with any other argument, with two argument names equal ignoring case,
  or with arguments that are not a JSON object is refused with
  `invalid_arguments`, which names the argument; it is never forwarded, and
  nothing is silently dropped. `arguments` is required on an `acts` tool.
  On a `reads` or passthrough tool it defaults to the argument names its
  `when` rules use, and a list adds to them. Every listed argument must be
  one the upstream tool takes (`inputSchema.properties`) when the upstream
  publishes a schema; the gateway checks this at start, on every call and
  in `gateway check`, which also prints the upstream arguments that are not
  forwardable. When the schema declares a `type` for an argument (`string`,
  `number`, `integer`, `boolean`, `array`, `object` or `null`, or a list of
  them, and an `array`'s `items.type`), a call whose value has another JSON
  type is refused with `invalid_arguments`: an object or a list where the
  upstream takes a string (`text: {"channel": "#all-company"}`) never
  reaches it. This is a small, strict check of the top-level arguments, not
  a JSON Schema validator; an argument with no declared `type` is not
  checked.
- **reads.** The tool's results are a policy source. The first rule whose
  `when` matches gives the source; otherwise `otherwise` does. `otherwise`
  must be at least as restrictive as every rule's source: the people who may
  read it, and the agents that may use it, must each be among those of every
  source the rules name. A config that breaks this is an error at start and
  in `gateway check`; if a policy activation breaks it later, the tool is
  closed (`tool_misconfigured`) until it is fixed.
- **acts.** The tool is a policy action. The first rule whose `when` matches
  gives the action; a `when` value matches exactly unless the rule gives a
  `type` (see [How `when` matches](#how-when-matches)). A call that matches
  no rule is blocked. `content` names
  the argument the operator sees as the content; it is only a label. The
  output that is checked is always every forwarded argument (see
  [What a call does](#what-a-call-does)). `allow_without_reads: true` lets a
  call from a session that has read nothing through unchecked; it needs
  `session: agent`, because in connection mode a new connection would be a
  way to shed what was read.
- **passthrough.** The tool is forwarded untouched. Its results are still
  content an agent reads, of unknown origin. If the policy defines a source
  named `unclassified`, they are registered as that source. Otherwise the
  tool needs `untracked: true`, which forwards its results with no tracking.

The gateway and `gateway check` warn about a rule no call can reach, because
an earlier rule of the same tool matches every call it would.

### How `when` matches

**The policy audience is only as true as the `when` rule that maps a call
to an action.** The policy says `post_to_engineering` reaches engineering;
the gateway believes that only because a rule says a call with `channel:
"#engineering"` is `post_to_engineering`. A rule loose enough to match a
value that reaches someone else sends the output to people the policy never
checked. Match recipient arguments exactly, or with `type: email`.

A `when` rule names arguments and a pattern for each, as `argument:
pattern` or `argument: {pattern: …, type: …}`. All must match. The types:

| `type` | matches | default for |
|---|---|---|
| `exact` | the value equals the pattern byte for byte; no wildcards | `acts` rules |
| `email` | a strict email address: `name@domain` (that address; the domain ignoring case) or `*@domain` (any valid local part at exactly that domain) | — |
| `glob` | a glob (below) | — |
| `path` | a glob over a path; odd path forms never match (below) | `reads` rules |

On an `acts` rule, a pattern that contains `*` or `?` without an explicit
`type: glob`, `email` or `path` is a config error, so a wildcard on a
recipient is always a choice you made:

```yaml
  slack.send_email:
    arguments: [to, subject, body]
    acts:
      - when: { to: { pattern: "*@corp.com", type: email } }   # not `to: "*@corp.com"`
        action: email_staff
        content: body
```

`type: email` accepts only ASCII, exactly one `@`, a local part of letters,
digits and `._+-` with no leading, trailing or doubled dot, and a domain of
dot-separated labels of letters, digits and `-`. Anything a mail parser
could read as another address never matches: comments
(`x@evil.com(@corp.com`), a second `@` (`x@evil.com@corp.com`), `%` and `!`
routes, quoted local parts, encoded words (`=?utf-8?q?…?=@corp.com`),
address literals, and subdomains (`*@corp.com` does not match
`x@mail.corp.com`).

A glob is case-sensitive. `*` matches any run of characters and `?` one
character, but neither matches `/`, `,`, `;`, whitespace, `<`, `>`, `"` or a
control character; `**` is `*` that also matches `/`. Every other character
is itself. A glob is weak for a recipient: `*@corp.com` matches
`x@evil.com@corp.com`, which a mail server may deliver to `evil.com`. The
gateway and `gateway check` warn about every `type: glob` on an `acts` rule.

An argument value matches when it is a string that matches, or a non-empty
list whose every element is a string that matches. An object, a number, or
a list with anything else in it never matches.

Matching fails closed, for every type. A value does not match if it is
longer than 4,096 bytes, if it has a `..` segment (split on `/` and `\`), or
if it contains a control, format or separator character (Unicode categories
Cc, Cf, Zl and Zp: NUL, zero-width spaces and joiners, bidirectional
controls, the soft hyphen and the like), a Hangul filler (U+115F, U+1160,
U+3164, U+FFA0) or a tag character (U+E0000 to U+E007F). A pattern that
holds one is a config error. An argument of `type: path` also does not
match if it contains `%`, `~`, `\` or a character that is not ASCII, if it
starts with `/` or `./`, or if it has an empty segment (`//`) or a `.`
segment. Such a call falls to `otherwise`. The upstream may decode, expand
or normalise a path the gateway saw as `Product/%2e%2e/HR/x` into one no
rule meant.

The gateway and `gateway check` also warn about an `acts` rule with no
`when` (every call to the tool is that action, whatever its arguments), and
name, for each `acts` rule separately, every allow-listed argument of the
tool that this rule's own `when` does not constrain (another rule naming it
does not count): it is part of the checked output and shown to the approver,
but it can hold anything, so it must not be one that chooses who the call
reaches.

Classification trusts the upstream's own naming. A link, alias, symlink or
id on the upstream can make `Product/roadmap.md` return other content. That
is why `otherwise` must be the most restrictive source, and why a `reads`
rule should name only paths whose content you know.

## What a call does

- **A read** (a `reads` tool, or a tracked passthrough). The gateway checks
  the arguments and resolves the source. If the policy does not list this
  agent for that source, the call is refused and the upstream is not called.
  The refusal is the same `tool_not_available` error as for a tool that does
  not exist. The gateway then forwards the call, hashes the result,
  registers it as the source and adds it to the agent's session. Only then is
  the result returned. If it cannot be recorded, the result is withheld
  (`not_recorded`).
- **An action** (an `acts` tool). What is checked is exactly what is
  forwarded: the gateway takes the canonical JSON of every argument the call
  carries (keys sorted, no whitespace) and records it as an output of
  everything the session carries. Then it checks the action. It forwards
  the call only on **allow**. On **deny** the agent gets a tool error with
  the reason. On **needs approval** the error names the decision, and the
  request appears on the operator page.
- **An approval releases one call.** Once a reviewer approves, the agent
  retries exactly the same call; the gateway forwards it and uses the
  approval up (a journaled revoke), so the same call again needs a new
  approval. A call with any argument changed is a different output and a
  new decision. Two retries at once release one call; the other is held
  again, with its arguments, and can be approved like the first.
- **Nothing read.** An action from a session that has read nothing is refused
  with `nothing_read`. With `allow_without_reads: true` on the tool (agent
  mode only), it is forwarded without a check, because the runtime cannot
  record an output that comes from nothing. The gateway prints a line for
  each such call, and `gateway check` lists the tool as "not checked when
  nothing was read".
- **Untracked passthrough.** Forwarded and returned as is.

For a call the gateway holds for approval, the operator page shows the tool,
the action and the recipients; then the arguments the matched rule's `when`
names, by their real values (`channel: #engineering`), because they decide
who the call reaches; then every other forwarded argument, sorted by name,
one per row, each value cut at 2,000 characters with a "truncated — N more
characters" marker; and the SHA-256 of the full canonical arguments. The
confirmation step repeats the matched arguments. Argument text is shown as
text: newline, carriage return and tab as `\n`, `\r` and `\t` (and a
backslash doubled), and control, bidirectional and invisible characters
(the soft hyphen, Hangul fillers, tag characters and other format
characters included) as escapes such as `‮` or `\u{e0041}`. The
arguments are kept in memory only, never in the journal. After a restart
the gateway no longer has them: the page says the agent must retry the call,
and offers no Approve for it (the server refuses one too) until it does.

A tool's result is returned as the upstream sent it. All text an upstream
provides is untrusted: tool descriptions, titles, input schemas and results.
Descriptions and every `description` and `title` inside an input schema are
shown to agents with control characters removed and at most 1,024
characters each. A tool whose input schema is larger than 64 KiB is not
offered and refuses calls; the gateway does not start with one.

## Sessions

A session is what one agent has read, or received in a hand-off, through the
gateway. Reads accumulate for the session's lifetime and attach to every
later action. No tool or request lets an agent reset its session.

- **`session: agent`**. One session per agent token, for the gateway's
  lifetime. This is the strictest mode: it survives reconnects and restarts,
  because the gateway rebuilds it from the journal. Only the operator can
  end it, which is how such a session is reset.
- **`session: connection`** (the default). Each MCP session (the
  `Mcp-Session-Id` of the streamable HTTP transport) is one gateway session,
  bound to the agent that opened it. It ends when the client closes the
  connection, when the operator ends it, or when the gateway restarts. It
  does not end because it is idle. This mode assumes that the host starts a
  new connection, with a fresh agent context, for each task, and that the
  agent cannot reconnect at will. An agent that can open a new connection
  starts a new, empty session, and so does a client that reconnects by
  itself: a session the operator ended, and every connection after a
  restart, answers 404, and MCP clients such as rmcp then initialise again
  without telling the agent. Use `session: agent` when the host cannot
  guarantee a fresh agent context per connection.

In both modes an MCP session id belongs to the agent whose token opened it:
another agent's token can neither use it nor close it (404).

The operator page has a **Sessions** section: each live session with its
agent, start time and the sources it carries, and an **End session** button
that asks first. Ending a session forgets what it carries and closes every
open connection of it (in agent mode, all of that agent's connections), so
the client must initialise again and starts an empty session. The agent
keeps whatever is already in its own context, so stop or restart the agent
first; the page says so next to the button. If a policy activation removes
a source or action that `gateway.yaml` names, or makes an `otherwise` less
restrictive than a rule, the page lists the problem, and the affected tool
answers `tool_misconfigured` until it is fixed.

### Open connections

An agent may hold at most 64 open connections to the gateway. In connection
mode connections never idle out, because idling out would shed what they
read; at the cap a new connection is refused (HTTP 429), and no connection
is ever closed to make room. A client that leaks connections therefore
reaches the cap. The operator page lists every open connection with its
agent, session, start and last activity, and an **End connection** button
(the session it serves keeps what it read), and it names each agent at the
cap. In agent mode an idle connection may close: the session is the
agent's, and is kept.

## Hand-offs

Every agent sees two more tools:

- `trustgraph__handoff` returns an opaque token for what this session
  carries.
- `trustgraph__receive {token}` adds a token's contents to this session. It
  is refused if the token is expired, forged or edited, if its issuer is no
  longer an agent of the policy (`invalid_handoff`), or if this agent is not
  cleared for a source the token carries (`not_cleared`). Nothing is added
  then, and the receiving session may no longer act: every later action is
  refused with `session_blocked` until the operator ends the session.
  Refusing a hand-off never leaves the receiver freer than accepting it.
  Only a bad or not-permitted token blocks (forged, edited, expired, issuer
  removed, not cleared). A refusal that is not about the token (a journal or
  artifact budget used up, a rate limit, an engine or journal error) does
  not block: it is a plain tool error saying the hand-off did not happen,
  and the receiving session is unchanged.

What a refusal blocks depends on the session mode:

- **`session: agent`.** The session is the agent's, so the agent may not act
  on any connection until the operator ends its session.
- **`session: connection`.** A session is one connection, so blocking only
  it would let the agent act freely from a new connection. The gateway
  therefore also blocks the agent as a whole: none of its connections,
  current or new, may act (`agent_blocked`) until the operator clears the
  block with **Clear block** on the operator page. The refusing connection's
  own session stays blocked until it is ended. Use `session: agent` when
  agents hand off: there the block and what was read both follow the agent.

Blocks on agents (and agent-mode sessions) are kept in
`<state>/gateway.blocked`, written atomically (a new 0600 file, never
through a link, renamed into place), so they survive a restart. The gateway
does not start if that file exists but cannot be read or is not a JSON list
of session ids: ignoring it would unblock. Fix it, or remove it to unblock
everything it held.

If the file cannot be written later, the block stays in memory and the
gateway refuses every call from every agent (`gateway_degraded`) until a
write succeeds; each later call retries the write, and the operator page
shows the degraded state.

The gateway makes and checks tokens with an HMAC key kept 0600 in
`<state>/gateway.secret`. A token names the agent that issued it and the
journal position, and it expires after 24 hours. An agent cannot forge or
edit one. Dropping a token cannot be detected: an agent that never passes it
on simply does not pass on what it read. The next agent then starts with
less than it should carry.

## Failures

The gateway fails closed and keeps serving. An upstream that cannot be
reached, a call that takes longer than `timeout_seconds`, an upstream error,
and an engine or journal error each become a short tool error for that call.
A result larger than `max_result_bytes`, a malformed result and a result
that holds a credential all get the same `upstream_result_withheld` error,
so the error says nothing about a credential. The error names the upstream,
never the transport detail. After a journal failure the whole server
answers `restart_required`, as `serve` does. The gateway connects to an
upstream on first use and again after it fails.

A timed-out action may already have happened on the upstream: the gateway
stopped waiting, not the upstream. A retry sends it again, and an approved
call that timed out has used its approval.

Gateway calls count against each agent's rate limit and journal and artifact
budgets, as HTTP API calls do; see [Limits and quotas](serve.md#limits-and-quotas).

## What it covers and what it does not

Covered: tool calls through `/gateway/mcp` to the listed tools, with only
their listed arguments, and the gateway's two hand-off tools.

Not covered:

- Classification is by the request's arguments, not by the content an
  upstream returns. A read is the source its rule names, whatever comes back.
- The result of an action tool (for example "posted, id 123") is returned
  untracked. Links inside any result are not followed or tracked, and an
  upstream tool's side effects beyond its result (a file it writes, a
  request it makes) are not tracked.
- A dropped hand-off token cannot be detected.
- MCP resources, prompts, sampling and elicitation are not exposed. Only
  tools are. Tools that ask for input mid-call or that run as tasks are
  refused.
- In `connection` mode, an agent or client that can open new connections
  starts fresh sessions; use `agent` mode where it can.
- Ending a session does not clear what the agent already holds in its own
  context.

## Threat model

Everything in the [serve threat model](serve.md#threat-model) applies. In
addition:

- **The gateway holds upstream credentials.** Protect its environment, its
  config and its state directory. Secrets supplied through the environment
  are readable by any process running as the same OS user, and that
  includes a stdio upstream the gateway launches: it runs as the gateway's
  user, so it can read the gateway's environment (for example through
  `/proc` on Linux) and the state directory, with its agent tokens and the
  operator sign-in file. The private `HOME` and minimal `PATH` keep an
  honest server from picking them up by accident; they do not contain a
  hostile one. Run the gateway as its own OS user, keep the state directory
  unreadable to others, and run untrusted stdio servers (anything fetched
  with `npx` or `uvx`) as a different user or in a sandbox, or reach them
  over HTTP instead.
- **Agents must have no other route to the upstreams.** The gateway can be
  skipped by any agent that holds the upstream's credentials or can reach it
  some other way. Give the credentials only to the gateway.
- **Upstream text is untrusted.** Tool descriptions, titles, schemas and
  results come from the upstream servers. The gateway removes control
  characters from descriptions and schema text and withholds a result that
  contains a credential, but it does not judge what the upstream says.
- **What a tool reads is what the config says.** A `reads` rule that names
  the wrong source lets content through under a weaker label, and the
  upstream decides what a path or id really returns. Review `gateway.yaml`
  like the policy, make `otherwise` the most restrictive source, and read
  the `gateway check` summary.
