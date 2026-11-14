# Consulting decision memo and permission simulator

The user wants a high-stakes business case and an intuitive demo, with existing agents submitting work. This extends the approved Rust plan compiler; it remains a single-workspace app.

## Business decision

Fictional Aster Advisory advises Atlas Capital on a $240m acquisition of Helios Manufacturing. The partner must send a recommendation to Atlas's investment committee. The agent can read the target data room, a shareable industry brief, and a previous client's confidential benchmark. Its ability to read the benchmark does not authorize Atlas to receive a recommendation derived from it. All names, values, source permissions and conclusions are synthetic.

The buyer is a consulting firm's risk/operations partner; the daily users are engagement partners and managers. The product enables controlled reuse of firm knowledge in client deliverables. It blocks a concrete unauthorized flow, offers usable alternatives, and binds approval and a delivery receipt to the actual result. It does not validate the investment thesis, guarantee semantic sanitization, or establish legal compliance.

## Approaches and implementation call

An expanded technical graph would preserve capability visibility but leave the business task obscure. A generic inbox would obscure the reason work is blocked. Build a decision-focused simulator as the default screen, with the existing reviews, sources, plan compiler and finite checker available behind it. This is the implementation choice within the user's requested direction.

## Scenario and controls

Documents: market-brief and helios-data-room are readable by Maya (partner), Arun (manager), Sam (Atlas CFO), and the diligence agent. orion-benchmark is readable only by Maya, Arun and the agent. A recorded task-prompt is always included. Destinations: atlas-committee (Maya, Arun, Sam) and engagement-team (Maya, Arun). The input authorization is configured synthetic engagement access, not a claim that cross-client internal use is generally permissible.

The screen explains the decision, lets users toggle three documents and select a destination, shows a document/recipient permission matrix, and names conflicts. It offers two explicit actions: omit the unshareable source and generate in a fresh context, or keep the evidence and restrict delivery to the internal team. Neither changes ACLs or silently declassifies an existing output. A safe plan can prepare an actual tracked memo, then enters the existing human-review and guarded local-delivery flow. Approval remains required even after a green preview. A reset restores the blocked scenario. Advanced details disclose the typed plan and snapshot.

## Boundaries

The server builds and analyzes the same typed plan executed by the runtime; frontend code does not decide access. Preview is stateless. Selection validation and source authorization precede any returned source metadata or compiler report. Sam cannot inspect or infer the private benchmark via scenario endpoints. Prepare requires a reviewer/agent, checks expected digests before any proposal or generation, and uses immutable plan/run IDs. Duplicate preparation reuses the same run. Current permissions are rechecked at release.

Use a separate .trustgraph-consulting data directory by default; preserve legacy Northstar journal data and regression fixtures. Never seed a different fixture into an existing workspace. A fixture renderer produces an evidence-backed decision brief from only declared inputs, with explicit human judgment still required. It is not a live LLM. Destinations remain simulated and no external messages are sent.

## Validation

HTTP tests cover blocked cross-client disclosure, both safe alternatives, empty/unknown/duplicate input, private source visibility, stateless preview, stale digest rejection, blocked preparation without effects, exact generated lineage, authorized approval and simulated delivery, and repeated preparation. Run the full Rust checks and verify real browser controls, narrow layout, review handoff and screenshot. Write a business case and a two-minute demo script with no invented market metrics.
