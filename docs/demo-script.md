# Two-minute consulting demo

Run `cargo run --locked -p trustgraph-cli -- serve`, then open http://127.0.0.1:8080/#/simulator as Maya. Everything is fictional; generation and delivery are local simulators. Reset restores the original selection without deleting reviews or delivery history.

**0:00 — Put the decision first.** “Aster is advising Atlas on a $240m acquisition. One customer drives 32% of the target's revenue. Maya needs to prepare a recommendation the investment committee can actually use.”

**0:20 — Show the conflict.** Start with all three documents selected and the Atlas investment committee as audience. “The agent can read all this evidence. Sam, the client's CFO, cannot receive work derived from the previous client's benchmark.” Point to the blocked verdict and Sam's blocked matrix cell. No generation or publication has occurred.

**0:45 — Compare the business choices.** Select the Aster engagement team. “The same evidence is permitted for this internal discussion under the case's configured permissions. It is not a client deliverable.” Select the Atlas committee again. Click **Keep the audience. Use shareable evidence.** “Now we are preparing a fresh memo from the industry brief and this deal's diligence pack. The private benchmark is not an input.” The matrix marks the omitted document as not used. A permission pass does not claim equal content or business correctness.

**1:10 — Prepare accountable work.** Click **Prepare memo for review**. Open the evidence list in the review if useful. “These are the actual inputs, including the recorded instructions. The partner sees the actual decision brief, its limits, and the exact recipients. The agent cannot approve itself.”

**1:35 — Approve the actual version.** Click **Approve & release**, inspect the confirmation and confirm. “Approval covers these bytes and this destination. TrustGraph checks the current permissions before the local publisher accepts the memo.” Download the evidence or open Deliveries. No external client message is sent.

**1:55 — Explain the purchase.** “The firm gets a controlled path from permissioned research to client advice, with practical ways to resolve blocked work and evidence of what was delivered. A pilot measures review effort and control effectiveness; it does not assume avoided investment losses.”

Optional: switch preview identity to Sam. The private source is absent and Sam cannot prepare or approve work. Switch back to Maya and Reset case for the next demo. Advanced verification retains the separate bounded authorization-model demonstration; it is not a simulation of the acquisition's financial outcome.


For a repeat demo that includes a brand-new preparation and approval, start the server with a fresh named data directory (`--data /tmp/trustgraph-consulting-demo-2 --port 8081`, for example). The app deliberately reuses an existing memo for the same plan and permission snapshot; Reset does not remove its review or receipt. Returned plan work must be revised through a new agent plan, or a changed evidence/audience selection. The local renderer records feedback but does not apply writing changes.
