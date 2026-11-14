# Business case: permission-safe acquisition recommendations

## The decision on the partner's desk

Aster Advisory has been retained by Atlas Capital to assess a **$240 million acquisition of Helios Manufacturing**. The investment committee needs a recommendation: commit, renegotiate, or pause. Aster's engagement partner, Maya, is accountable for the memo. Sam, Atlas's CFO, will use it in the capital-allocation discussion.

The target reports $180m revenue and $27m adjusted EBITDA. The proposed enterprise value implies approximately 8.9x EBITDA. One customer represents 32% of revenue and renews in six months. The committee needs to understand how much of the earnings base is dependable before making a binding offer. These are fictional figures, not an actual transaction or investment recommendation.

The consulting agent finds three useful documents:

| Evidence | Why it matters | Permitted use in this case |
| --- | --- | --- |
| Industrial market outlook | Frames demand cyclicality and diligence questions | May inform the Atlas deliverable |
| Helios diligence pack | Records the offer, earnings and customer-renewal exposure | May inform the Atlas deliverable |
| Orion operating benchmark | Contains a previous client's private downside assumptions | Explicitly permitted for internal engagement analysis, but cannot inform a deliverable to Atlas |

The benchmark is tempting because it makes the recommendation look more specific. A recommendation based on it might reveal confidential assumptions even if the memo never names Orion. Aster's right to read a document does not, by itself, give Atlas a right to receive the work derived from it. Internal cross-client use is an explicit synthetic permission in this demo, not a general authorization for consulting firms.

## The business failure

The problem appears at the boundary between research and client delivery. The agent has enough access to assemble a credible memo, but its audience has narrower rights than its source context. A partner reading polished prose may not know every document, prompt or previous draft behind it. Removing a client name from the wording does not establish that the information dependency has disappeared.

Two assets are exposed: the current client's decision process and the previous client's entrusted information. The $240m decision explains why an accountable partner needs traceable evidence. It is **not** a claim that TrustGraph prevents an investment loss or that a confidentiality incident would cost $240m.

The underlying need is grounded in existing professional practice: McKinsey's published client-service policies describe confidentiality obligations and controls against misuse of client information. This is evidence of the kind of obligation consulting firms manage, not an endorsement or customer claim. [Client service policies](https://www.mckinsey.com/about-us/overview/our-governance/client-service-policies).

## What TrustGraph changes

An existing agent proposes the work before acting: read these sources, prepare this memo, obtain a partner's approval, and deliver to these recipients. TrustGraph checks that composition against current source permissions.

In the initial plan, it identifies the exact boundary: **Sam, Atlas's CFO, cannot receive a memo derived from Orion's confidential benchmark.** The simulator shows the selected source and blocked recipient in a simple table. It does not ask the partner to interpret a security graph.

The partner has two commercially useful choices:

1. **Prepare a client memo from shareable evidence.** Exclude the Orion benchmark and generate in a fresh context using the market outlook and Helios diligence pack. The pack still supports a concrete diligence question: resolve the material customer renewal and quality-of-earnings review before a binding offer. The partner must assess that conclusion; TrustGraph does not certify it. Excluding evidence may change the recommendation or leave uncertainty.
2. **Keep all evidence for an internal discussion.** Restrict recipients to Maya and Arun, the Aster engagement team. This lets them assess the analysis internally under the case's configured permissions. It does not produce a client-ready memo or a basis for later sanitizing one by editing it.

When a plan passes, the runtime records the actual inputs and generated content. A human approves the exact memo and destination. Before delivery, the runtime checks the current permissions and the approval binding again. The receipt and evidence remain attached to that version. A green simulator is permission to prepare work, not advance approval of an unknown output.

## Why a firm would buy it

**Economic buyer:** the risk, operations or managing partner responsible for enabling AI-assisted client delivery. **Champion:** the AI/knowledge leader. **Daily users:** engagement partners and managers. **Initial unit of value:** a client decision memo that can be reviewed, shared and later explained with its evidence intact.

The buying case has three parts. First, a firm can apply an enforceable boundary to a workflow it otherwise might keep manual or forbid. Second, a reviewer sees a specific conflict and a practical alternative, reducing time spent reconstructing the source context. Third, if a recommendation is challenged later, the firm can identify the actual source versions, audience, approver and delivered bytes for that release. Each of these is a testable value hypothesis; none is a measured customer result yet.

The product belongs where an authorized agent's output becomes a consequential client deliverable. Source ACLs remain necessary; they do not themselves express which recipients may receive a new artifact built from several sources. Text scanning can complement the boundary; this implementation tracks recorded dependencies rather than guessing whether sensitive phrases remain. An approval inbox becomes useful only when approval is tied to the actual evidence and delivery path.

## A pilot worth paying for

Start with one advisory practice, one existing agent, a small permissioned source set and one controlled delivery destination. Deploy a single workspace within the firm's environment. No multitenancy, billing platform or generalized connector marketplace is needed to prove the workflow.

Run a shadow period with representative historical memos and deliberately seeded permission conflicts. Then allow a narrow set of actual deliveries only after source identity, authorization and complete agent-context capture are qualified. The current implementation is the local demonstration of that contract; it does not yet connect to firm repositories or send client documents.

Measure:

- **Control effectiveness:** seeded unauthorized recipient/source combinations must be blocked, with an understandable reason; track false blocks separately.
- **Review effort:** minutes to establish source permission and recipient scope, before and during the pilot. Separate document-content judgment from access review.
- **Completion:** how often a blocked plan becomes a permitted deliverable or an explicitly internal analysis, and how often work remains unresolved.
- **Evidence quality:** every controlled delivery must retain exact content, source lineage, recipient set, reviewer and receipt.
- **Adoption:** which engagement managers return to the workflow without specialist help.

A commercial decision can use measured reviewer time saved × measured memo volume × the firm's loaded reviewer cost, minus integration, administration and product cost. Use the firm's data; do not present hypothetical incident losses or unmeasured productivity percentages as ROI. Confidentiality protection is a buying rationale, not a defensible promise of an avoided dollar loss.

## What the demo proves, and the next investment

The UI demonstrates real Rust compiler decisions, exact recorded input lineage, mandatory human approval and controlled local publication. The existing finite checker and seeded race/failure simulator exercise additional authorization and recovery properties within stated bounds. They do not establish correctness of financial analysis or protection against every possible information channel.

Before a real pilot, qualify firm identity, repository permissions, complete capture of model/tool/memory inputs, provider retention controls, and a delivery integration that cannot be bypassed by the agent. Recorded source rights must model authorized **use**, not merely technical readability, or the workflow must exclude sources whose use cannot be established. A permission-only model cannot infer contractual purpose restrictions or certify semantic independence. These are concrete next-stage requirements, not UI controls the demo pretends to implement.

NIST's Generative AI Profile is a relevant voluntary risk-management reference for evaluating the broader deployment. TrustGraph's local authorization checks do not constitute certification against that framework. [NIST AI 600-1](https://www.nist.gov/publications/artificial-intelligence-risk-management-framework-generative-artificial-intelligence).
