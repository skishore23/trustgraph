# trustgraph gateway

`trustgraph gateway` sits between agents and the MCP servers they use. It
holds those servers' credentials, so agents can reach them only through it.
It records what each agent reads, and it checks every action against what
was read before it forwards the call. An agent cannot skip it, because the
gateway is its only route to the upstream servers.

The gateway is [`trustgraph serve`](serve.md) plus one MCP endpoint,
`/gateway/mcp`. It is the same process, with the same state directory, agent
tokens, operator page, HTTP API and limits. Everything it records goes
through the same runtime and journal.

## Quickstart

The example in [examples/gateway](../examples/gateway) runs with no external
services. Its upstreams are a small fake MCP server. Run it from the
repository root:

```sh
cargo build -p trustgraph-cli --bins --examples
SLACK_BOT_TOKEN=demo-secret target/debug/trustgraph gateway \
  --policy examples/policies/handoff.policy.yaml --operator nina \
  --config examples/gateway/gateway.yaml --state ~/tg-gateway
```

```text
TrustGraph gateway: http://127.0.0.1:8787
MCP endpoint: http://127.0.0.1:8787/gateway/mcp
Operator sign-in (single use): /Users/you/tg-gateway/operator.html
Agent tokens: /Users/you/tg-gateway/tokens
Policy in force: revision 1 (from the policy file), digest d004fcf413c1
Operator: nina
warning: the policy file is inside the Git repository /Users/you/TrustGraph; agents working there can edit it. Changes to it never apply without the operator.
Sessions: connection; tools exposed: 4
upstream `slack`: fake_upstream: starting with credential [redacted]
trustgraph gateway: upstream `drive`: connected, 7 tools
trustgraph gateway: upstream `slack`: connected, 7 tools
```

The fake `slack` server logs its own credential on purpose; the gateway
replaces every credential value in what it prints. In another terminal, two
agents use the gateway. The researcher reads the salary file and hands off.
The planner receives the hand-off, reads the roadmap and tries to post:

```sh
target/debug/examples/gateway_demo --tokens ~/tg-gateway/tokens
```

```text
researcher drive__read_file HR/Compensation/salaries.csv: nina: 120k, sam: 95k
researcher trustgraph__handoff: a hand-off token
planner trustgraph__receive: {"carries":["salaries"],"received":true}
planner drive__read_file Product/roadmap.md: Q3: ship the new billing page
planner slack__post_message #engineering: error: TrustGraph gateway: `post_to_engineering` was not forwarded: denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included (denied)
```

The `post_message` call never reached the chat server.

Check a config against a policy before you start the gateway:

```sh
trustgraph gateway check --config examples/gateway/gateway.yaml --policy examples/policies/handoff.policy.yaml
```

```text
Sessions: connection (each MCP connection is one session; it assumes the host opens a new connection per task and the agent cannot reconnect at will).
Upstreams: drive (runs target/debug/examples/fake_upstream), slack (runs target/debug/examples/fake_upstream).
Agents see 4 tools; every other upstream tool is invisible and uncallable:
  drive__read_file reads a source; its result is recorded before it is returned:
    when `path` matches `HR/Compensation/**`: `salaries` (usable by planner, researcher)
    when `path` matches `Product/**`: `roadmap` (usable by planner)
    when `path` matches `Public/**`: `benchmarks` (usable by planner, researcher)
    otherwise: `salaries` (usable by planner, researcher)
  slack__post_message is an action, checked against everything the session read before it is forwarded:
    when `channel` matches `#engineering`: `post_to_engineering` with `text` as the output (reaches ravi, sam)
    otherwise: blocked, never forwarded
    a call from a session that has read nothing is refused (nothing_read)
  trustgraph__handoff and trustgraph__receive pass what a session carries to another agent's session
warning: not set here, and required where the gateway runs: SLACK_BOT_TOKEN
```

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
transport:

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
that has a problem, or with one that names a source or action the active
policy does not have.

```yaml
version: 1
session: connection            # connection (default) or agent
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
    reads:
      - when: { path: "HR/Compensation/**" }
        source: salaries
      - when: { path: "Product/**" }
        source: roadmap
    otherwise: unclassified     # required: the source of a call that matches no rule
  slack.post_message:
    acts:
      - when: { channel: "#engineering" }
        action: post_to_engineering
        content: text           # the argument that is the output (default: all arguments)
    otherwise: block            # the only value: an unmatched call is never forwarded
  drive.search:
    passthrough: true
```

- **Upstreams.** An upstream has a `url` (streamable HTTP) or a `command`
  (a program and its arguments, run with stdio), not both. Names are
  lowercase letters, digits, `-` and single `_`; `trustgraph` is reserved.
- **Secrets.** `${env:NAME}` is the only interpolation, and only in `headers`
  and `env` values. A variable that is not set stops the gateway from
  starting. The gateway never prints, logs, returns or journals a resolved
  value. An upstream result that contains one is withheld. A stdio upstream
  gets only the variables in its `env`, plus `PATH` and `HOME`. Its stderr is
  printed with every secret value replaced by `[redacted]`.
- **Tools.** Only the tools listed under `tools:` exist for agents. They are
  named `upstream.tool` in the config, and agents see them as
  `upstream__tool`. Every other tool on an upstream is invisible and
  uncallable. There is no switch that exposes unlisted tools.
- **reads.** The tool's results are a policy source. The first rule whose
  `when` matches gives the source; otherwise `otherwise` does.
- **acts.** The tool is a policy action. The first rule whose `when` matches
  gives the action and the content argument. A call that matches no rule is
  blocked. `allow_without_reads: true` lets a call from a session that has
  read nothing through unchecked (see below).
- **passthrough.** The tool is forwarded untouched. Its results are still
  content an agent reads, of unknown origin. If the policy defines a source
  named `unclassified`, they are registered as that source. Otherwise the
  tool needs `untracked: true`, which forwards its results with no tracking.

### How `when` matches

A `when` rule names arguments and a glob for each. All must match. A glob is
case-sensitive: `**` matches anything, `*` anything except `/`, `?` one
character except `/`, and every other character is itself.

Matching fails closed. An argument does not match if it is missing, if it is
not a string, if it is longer than 4,096 bytes, if it contains a control
character (NUL included), or if it has a `..` path segment (split on `/` and
`\`). Such a call falls to `otherwise`. The upstream may normalise a path
the gateway saw as `Product/../HR/salaries.csv` into one no rule meant. This
is why `otherwise` must be restrictive: for a reads tool, name the most
restricted source the tool can return; for an acts tool it is always
`block`.

## What a call does

- **A read** (a `reads` tool, or a tracked passthrough). The gateway resolves
  the source. If the policy does not list this agent for that source, the
  call is refused and the upstream is not called. The refusal is the same
  `tool_not_available` error as for a tool that does not exist. The gateway
  then forwards the call, hashes the result, registers it as the source and
  adds it to the agent's session. Only then is the result returned. If it
  cannot be recorded, the result is withheld (`not_recorded`).
- **An action** (an `acts` tool). The gateway takes the content argument and
  records it as an output of everything the session carries. Then it checks
  the action. It forwards the call only on **allow**. On **deny** the agent
  gets a tool error with the reason. On **needs approval** the error names
  the decision; the request appears on the operator page, and once a
  reviewer approves it the agent may retry the same call. The same content
  from the same session is the same output, so the approval applies.
- **Nothing read.** An action from a session that has read nothing is refused
  with `nothing_read`. With `allow_without_reads: true` on the tool, it is
  forwarded without a check, because the runtime cannot record an output
  that comes from nothing. The gateway prints a line for each such call, and
  `gateway check` lists the tool as "not checked when nothing was read".
- **Untracked passthrough.** Forwarded and returned as is.

A tool's result is returned as the upstream sent it. Upstream tool
descriptions are shown to agents with control characters removed and at most
1,024 characters. They come from the upstream, so treat them as untrusted.

## Sessions

A session is what one agent has read, or received in a hand-off, through the
gateway. Reads accumulate for the session's lifetime and attach to every
later action. No tool or request lets an agent reset its session.

- **`session: connection`** (the default). Each MCP session (the
  `Mcp-Session-Id` of the streamable HTTP transport) is one gateway session,
  bound to the agent that opened it. It ends when the client closes the
  connection, when the operator ends it, or when the gateway restarts. It
  does not end because it is idle. This mode assumes that the host starts a
  new connection for each task, and that the agent cannot reconnect at will.
  An agent that can open a new connection starts a new, empty session.
  An agent may hold at most 64 open connections.
- **`session: agent`**. One session per agent token, for the gateway's
  lifetime. This is the strictest mode: it survives reconnects and restarts,
  because the gateway rebuilds it from the journal. Only the operator can
  end it, which is how such a session is reset.

The operator page has a **Sessions** section: each live session with its
agent, start time and the sources it carries, and an **End session** button
that asks first. Ending a session forgets what it carries; the agent's next
call starts an empty one. If a policy activation removes a source or action
that `gateway.yaml` names, the page lists the problem, and the affected tool
answers `tool_misconfigured` until it is fixed.

## Hand-offs

Every agent sees two more tools:

- `trustgraph__handoff` returns an opaque token for what this session
  carries.
- `trustgraph__receive {token}` adds a token's contents to this session. It
  is refused (`not_cleared`) if this agent is not cleared for a source the
  token carries; nothing is added then.

The gateway makes and checks tokens with an HMAC key kept 0600 in
`<state>/gateway.secret`. A token names the agent that issued it and the
journal position, and it expires after 24 hours. An agent cannot forge or
edit one. Dropping a token cannot be detected: an agent that never passes it
on simply does not pass on what it read. The next agent then starts with
less than it should carry.

## Failures

The gateway fails closed and keeps serving. An upstream that cannot be
reached, a call that takes longer than `timeout_seconds`, a result larger
than `max_result_bytes`, a result the gateway cannot use, an upstream error,
and an engine or journal error each become a short tool error for that call.
The error names the upstream, never the transport detail. After a journal
failure the whole server answers `restart_required`, as `serve` does. The
gateway connects to an upstream on first use and again after it fails.

Gateway calls count against each agent's rate limit and journal and artifact
budgets, as HTTP API calls do; see [Limits and quotas](serve.md#limits-and-quotas).

## What it covers and what it does not

Covered: tool calls through `/gateway/mcp` to the listed tools, and the
gateway's two hand-off tools.

Not covered:

- MCP resources, prompts, sampling and elicitation are not exposed. Only
  tools are.
- The result of an action tool (for example "posted, id 123") is returned
  untracked.
- An upstream tool's own side effects beyond its result (a file it writes, a
  request it makes) are not tracked.
- In `connection` mode, an agent that can open new connections can start
  fresh sessions; use `agent` mode where it can.
- Tools that ask for input mid-call or that run as tasks are refused.

## Threat model

Everything in the [serve threat model](serve.md#threat-model) applies. In
addition:

- **The gateway holds upstream credentials.** Protect its environment, its
  config and its state directory. Any process that can read the gateway's
  environment, or run as its OS user, can use the upstreams directly.
- **Agents must have no other route to the upstreams.** The gateway can be
  skipped by any agent that holds the upstream's credentials or can reach it
  some other way. Give the credentials only to the gateway.
- **Upstream text is untrusted.** Tool descriptions and results come from the
  upstream servers. The gateway removes control characters from
  descriptions and withholds a result that contains a credential, but it does
  not judge what the upstream says.
- **What a tool reads is what the config says.** A `reads` rule that names
  the wrong source, or an `otherwise` that is not restrictive, lets content
  through under a weaker label. Review `gateway.yaml` like the policy, and
  read the `gateway check` summary.
