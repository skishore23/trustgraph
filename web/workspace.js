const app = document.querySelector("#app");
const e = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const clone = (x) => JSON.parse(JSON.stringify(x));
let state,
  graph,
  timer,
  candidate,
  result,
  suite,
  mapOpen = false;
let goal =
  "Implement the approved payment reconciliation behavior in src/lib.rs. Accept zero-value authorizations, reject negative amounts, preserve positive values and existing tests, and add a zero regression test. Use the supplied bank documents; do not reproduce confidential prose. Only change src/lib.rs.";
let draft = {
  paths: ["src/lib.rs"],
  tests_passed: true,
  lineage_complete: true,
  approval: "valid",
  approval_age_minutes: 0,
  include_risk: true,
  audience: "engineering",
};
const names = {
  queued: "Queued",
  running: "Agent working",
  awaiting_review: "Needs approval",
  approved: "Approved",
  published: "Draft PR opened",
  blocked: "Blocked",
  interrupted: "Interrupted",
  revising: "Rechecking revised patch",
  revision_failed: "Revision failed",
  outcome_unknown: "Publication unconfirmed",
  publication_stopped: "Publication stopped",
};
const route = () => location.pathname.split("/").filter(Boolean);
const selected = () => (route()[0] === "agent" ? route()[1] : undefined);
const page = () =>
  ["policy", "simulator"].includes(route()[0]) ? route()[0] : "workspace";
const badge = (text, kind = "") =>
  `<span class="badge ${kind}">${e(text)}</span>`;
const heading = (title, description) =>
  `<div class="heading"><span class="eyebrow">TRUSTGRAPH / ${page().toUpperCase()}</span><h1>${e(title)}</h1><p>${e(description)}</p></div>`;
async function api(path, body) {
  const r = await fetch("/api" + path, {
    method: body === undefined ? "GET" : "POST",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let text = "";
  try {
    text = await r.text();
  } catch {
    /* Treat an unreadable body as empty. */
  }
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = null;
  }
  if (!r.ok)
    throw Error(
      data && typeof data.error === "string"
        ? data.error
        : `Request failed (${r.status})${text ? `: ${text.slice(0, 160)}` : ""}`,
    );
  if (!data || typeof data !== "object")
    throw Error("The server returned an unreadable response.");
  return data;
}
function notify(text) {
  const n = document.querySelector("#notice");
  n.textContent = text;
  n.hidden = false;
  setTimeout(() => (n.hidden = true), 8000);
}
function go(path) {
  history.pushState({}, "", path);
  load();
}
function sources(job) {
  const inputs = job?.plan?.input_manifest;
  const all = inputs
    ? inputs.map((i) => state.sources.find((s) => s.id === i.id) || i)
    : state.sources;
  return `<details class="evidence sources"><summary>${all.length} captured inputs / HydraDB source records</summary>${all.map((s) => `<p>${e((s.path || s.name || s.id).split("@")[0])}${s.restricted ? " [RISK ONLY]" : ""}<small>SHA-256 ${e((s.hash || s.content_hash || "").slice(0, 20))}</small></p>`).join("")}<p class="muted">Generation uses exact bytes read back from HydraDB. The manifest records inputs actually supplied to this model turn.</p></details>`;
}
function architecture() {
  return `<details class="system-map" ${mapOpen ? "open" : ""}><summary>How it works ${badge(graph?.connected ? "HydraDB connected" : "HydraDB unavailable", graph?.connected ? "good" : "bad")}</summary><pre>CODING AGENT\n     |  HTTP / CLI\nPLAN + INPUT MANIFEST\n     |\nRUST POLICY + LINEAGE + APPROVAL\n     |                      |\nSIMULATOR                 RUNTIME\nimpact + counterexample     |\n                       PUBLICATION GATE\n                            |\n                       BRANCH + DRAFT PR\n\nHYDRADB: connected sources, generations, permissions, approvals, outcomes</pre><p class="muted">Rust evaluates the rules. HydraDB preserves the connected evidence and is read back before approval and publication. The publishing gate saves each publication attempt before sending it to GitHub.</p></details><footer class="hydra"><strong>[+] HYDRADB</strong><span>trustgraph / trustgraph-governance</span><span>${graph?.connected ? "CONNECTED EVIDENCE" : "CONNECTION UNAVAILABLE"}</span></footer>`;
}
function policyFields(p, id, advanced = false) {
  return `<label for="${id}-name">Policy name</label><input id="${id}-name" maxlength="100" required value="${e(p.name)}">${state.departments ? `<label>Who can access work created from the investigation?</label><div class="source-card"><strong>[x] Fraud &amp; Risk</strong><label class="check"><input id="${id}-engineering" type="checkbox" ${p.departments?.investigation_readers.includes("engineering") ? "checked" : ""}> Engineering may also read the investigation</label><small>Checking this grants source access. Human approval alone never grants it.</small></div>` : ""}<label for="${id}-minutes">Approval lifetime (minutes)</label><input id="${id}-minutes" type="number" min="1" max="1440" required value="${p.approval_minutes}"><label class="check"><input type="checkbox" id="${id}-tests" ${p.require_tests ? "checked" : ""}> Require passing Rust tests</label><details class="evidence" ${advanced ? "open" : ""}><summary>File scope</summary><label for="${id}-paths">Allowed files (comma-separated exact relative paths)</label><input id="${id}-paths" required value="${e(p.allowed_paths.join(", "))}"><label for="${id}-max">Maximum changed files</label><input id="${id}-max" type="number" min="1" max="10" value="${p.max_changed_files}" required></details>`;
}
function readPolicy(id) {
  return {
    name: document.querySelector(`#${id}-name`).value.trim(),
    allowed_paths: document
      .querySelector(`#${id}-paths`)
      .value.split(",")
      .map((s) => s.trim())
      .filter(Boolean),
    max_changed_files: Number(document.querySelector(`#${id}-max`).value),
    require_tests: document.querySelector(`#${id}-tests`).checked,
    approval_minutes: Number(document.querySelector(`#${id}-minutes`).value),
    ...(state.departments
      ? {
          departments: {
            investigation_readers: [
              "risk",
              ...(document.querySelector(`#${id}-engineering`).checked
                ? ["engineering"]
                : []),
            ],
          },
        }
      : {}),
  };
}
function policyPage() {
  return (
    heading(
      "Set the sharing boundary.",
      "Policies decide who can access derived work and what must happen before publication.",
    ) +
    `<div class="sim-layout"><section class="card"><div class="section-heading"><h2>Current policy</h2>${badge("REV " + state.policy_revision)}</div><form id="policy" data-revision="${state.policy_revision}">${policyFields(state.policy, "policy")}<div class="actions"><button type="button" class="secondary" id="test-policy">Test changes</button><button>Apply policy changes</button></div><p class="muted">Test first. Applying changes updates the authorization context; existing work needs a fresh run and review.</p></form></section><section class="card"><h2>One agent. Two departments.</h2><p class="muted">The agent may read both departments' sources. Engineering can access only work created from data it can access.</p><dl><div><dt>Engineering requirements</dt><dd>Engineering + Risk</dd></div><div><dt>Approved specification</dt><dd>Engineering + Risk</dd></div><div><dt>Confidential investigation</dt><dd>${state.policy.departments?.investigation_readers.includes("engineering") ? "Engineering + Risk" : "Risk only"}</dd></div></dl><h2>Always enforced</h2><ul><li>Derived work inherits source restrictions.</li><li>Missing lineage blocks publication.</li><li>Approval binds exact bytes, policy and repository.</li><li>A reviewer cannot override source access.</li></ul><p class="muted">These are system guarantees. The editable policy sets their parameters; arbitrary English rules are not executable policy.</p></section></div>`
  );
}
const presets = [
  ["risk", "Risk -> Engineering"],
  ["clean", "Shared sources"],
  ["risk_room", "Risk audience"],
  ["stale", "Patch changed"],
  ["destination", "Audience changed"],
  ["expired", "Expired approval"],
  ["lineage", "Missing lineage"],
  ["failed", "Failed tests"],
];
function preset(key) {
  return {
    paths: ["src/lib.rs"],
    tests_passed: key !== "failed",
    lineage_complete: key !== "lineage",
    approval: ["stale", "destination", "expired"].includes(key) ? key : "valid",
    approval_age_minutes: key === "expired" ? 1441 : 0,
    include_risk: ["risk", "risk_room"].includes(key),
    audience: key === "risk_room" ? "risk" : "engineering",
  };
}
function simulatorPage() {
  candidate ||= clone(state.policy);
  return (
    heading(
      "Test a policy. See the consequence.",
      "Run one case or compare the same policy across several scenarios. Every decision uses the Rust evaluator.",
    ) +
    `<p class="scope-note">What-if inputs and approvals are assumptions. No GitHub action runs here. Proposed policies and results are recorded in HydraDB; the current policy stays unchanged.</p><div class="presets">${presets.map(([id, label]) => `<button class="secondary" data-preset="${id}">${label}</button>`).join("")}</div><form id="simulation"><div class="sim-layout"><section class="card"><div class="section-heading"><h2>1 / Proposed policy</h2><button type="button" class="text-button" id="use-active">Reset</button></div>${policyFields(candidate, "candidate")}<details class="evidence"><summary>Import supported policy JSON</summary><label for="policy-json">Policy</label><textarea id="policy-json" rows="9" spellcheck="false">${e(JSON.stringify(candidate, null, 2))}</textarea><button type="button" class="secondary" id="import-json">Use JSON</button><p id="json-error" role="alert"></p></details></section><section class="card"><h2>2 / Scenario</h2>${state.departments ? `<label class="check"><input id="sim-risk" type="checkbox" ${draft.include_risk ? "checked" : ""}> Generation uses the Risk investigation</label><label for="sim-audience">Which department needs access?</label><select id="sim-audience"><option value="engineering" ${draft.audience === "engineering" ? "selected" : ""}>Engineering</option><option value="risk" ${draft.audience === "risk" ? "selected" : ""}>Fraud &amp; Risk</option></select>` : ""}<label for="sim-approval">Approval status for this test</label><select id="sim-approval">${[
      ["valid", "Approval for this version"],
      ["stale", "Patch changed after approval"],
      ["destination", "Audience changed after approval"],
      ["previous_policy", "Policy changed after approval"],
      ["expired", "Expired approval"],
      ["missing", "No approval"],
    ]
      .map(
        ([v, l]) =>
          `<option value="${v}" ${draft.approval === v ? "selected" : ""}>${l}</option>`,
      )
      .join(
        "",
      )}</select><label for="sim-age">Approval age (minutes)</label><input id="sim-age" type="number" min="0" max="10080" value="${draft.approval_age_minutes}"><label class="check"><input id="sim-tests" type="checkbox" ${draft.tests_passed ? "checked" : ""}> Tests passed</label><label class="check"><input id="sim-lineage" type="checkbox" ${draft.lineage_complete ? "checked" : ""}> All inputs are recorded</label><label for="sim-paths">Proposed files</label><input id="sim-paths" value="${e(draft.paths.join(", "))}"><div class="actions"><button>Compare this case</button><button type="button" class="secondary" id="run-suite">Run all scenarios</button></div></section></div></form><section id="results" aria-live="polite">${results()}</section>`
  );
}
const verdict = (x) =>
  x.status === "allowed"
    ? "Allowed"
    : x.status === "needs_approval"
      ? "Needs approval"
      : "Blocked";
function decisionCard(x, title) {
  return `<section class="card"><div class="section-heading"><h2>${e(title)}</h2>${badge(verdict(x), x.status === "allowed" ? "good" : "bad")}</div><dl><div><dt>Sharing permitted</dt><dd>${x.information_permitted ? "Yes" : "No"}</dd></div><div><dt>Policy requirements</dt><dd>${x.policy_passed ? "Pass" : "Fail"}</dd></div><div><dt>Approval for this version</dt><dd>${x.approval_matched ? "Approved for this test" : "No match"}</dd></div></dl>${x.counterexample ? `<div class="callout"><strong>${e(x.counterexample.rule)}</strong><p>${e(x.counterexample.explanation)}</p><p class="failure-path">${x.counterexample.path.map(e).join(" -> ")}</p></div>` : '<p class="muted">All modeled conditions pass. Live publication rechecks actual evidence.</p>'}</section>`;
}
function results() {
  if (suite)
    return `<section class="card"><div class="section-heading"><h2>Same scenarios. Two policies.</h2>${badge("NOTHING PUBLISHED")}</div><div class="table-scroll"><table><thead><tr><th>Scenario</th><th>Current policy</th><th>Proposed policy</th><th>Proposed policy explanation</th></tr></thead><tbody>${suite.map(([label, x]) => `<tr><td>${e(label)}</td><td>${verdict(x.baseline)}</td><td>${verdict(x)}</td><td>${e(x.counterexample?.rule || "All modeled checks pass")}</td></tr>`).join("")}</tbody></table></div><p class="muted">${suite.length} independently evaluated cases recorded in HydraDB. This tests supported policy conditions; it does not prove arbitrary code properties.</p><details class="evidence"><summary>Recorded case evidence</summary><pre>${e(
      JSON.stringify(
        suite.map(([scenario, x]) => ({
          scenario,
          graph_id: x.graph_id,
          candidate_policy_graph_id: x.candidate_policy_graph_id,
        })),
        null,
        2,
      ),
    )}</pre></details></section>`;
  if (!result) return "";
  return `<h2 class="comparison-heading">One proposal / current vs proposed policy</h2><div class="sim-layout">${decisionCard(result.baseline, "Current: " + result.active_policy.name)}${decisionCard(result, "Proposed: " + result.candidate_policy.name)}</div><p class="scope-note">Proposed changes tested only. Apply them on the Policy page if you intend to change source access.</p><details class="evidence"><summary>Rust decision + HydraDB evidence</summary><pre>${e(JSON.stringify(result, null, 2))}</pre></details>`;
}
function readSimulation() {
  candidate = readPolicy("candidate");
  draft = {
    paths: document
      .querySelector("#sim-paths")
      .value.split(",")
      .map((x) => x.trim())
      .filter(Boolean),
    tests_passed: document.querySelector("#sim-tests").checked,
    lineage_complete: document.querySelector("#sim-lineage").checked,
    approval: document.querySelector("#sim-approval").value,
    approval_age_minutes: Number(document.querySelector("#sim-age").value),
    include_risk: !!document.querySelector("#sim-risk")?.checked,
    audience: document.querySelector("#sim-audience")?.value || "engineering",
  };
}
function workspace() {
  const job = state.jobs.find((j) => j.id === selected());
  if (job) return detail(job);
  return (
    heading(
      "One agent. Two departments.",
      "The agent can help Engineering and Fraud & Risk. That does not give Engineering access to everything the agent learns.",
    ) +
    `<section class="card"><div class="section-heading"><h2>Payment reconciliation</h2>${badge("LIVE AGENT + HYDRA + GITHUB")}</div><div class="grid"><div class="source-card"><span>ENGINEERING</span><strong>Implementation requirements</strong><p>Accept zero-value authorizations. Approved specification available to both teams.</p></div><div class="source-card"><span>FRAUD &amp; RISK</span><strong>Confidential investigation</strong><p>The agent may read it. Engineering cannot access work derived from it.</p></div></div><form id="new-work"><label for="goal">Work order</label><textarea id="goal" rows="4" required maxlength="4000">${e(goal)}</textarea><label class="check"><input id="include-risk" type="checkbox" checked> Include the confidential Risk investigation</label><div class="actions"><button ${state.jobs.some((j) => ["queued", "running", "revising"].includes(j.status)) ? "disabled" : ""}>Run coding agent</button><a class="button secondary" href="/simulator">Test policy first</a></div><p class="muted">Fresh Codex turn. Actual Rust tests. Private Engineering repository. Bank documents are controlled test material; the integrations are live.</p></form></section><details class="evidence"><summary>Try from Codex or a terminal</summary><pre>cargo run --locked -p trustgraph-cli -- run --include-risk \\\n  --goal "$(cat examples/bankcase/work-order.txt)"</pre><p class="muted">The CLI uses a restricted agent token. It can submit work and test policies; approval remains with the operator.</p></details><section class="work-list"><h2>Recent work</h2>${state.jobs.length ? state.jobs.map((j) => `<button class="work-row" data-job="${e(j.id)}"><span><strong>${e(j.goal)}</strong><small>${j.include_risk ? "Risk investigation included" : "Shared sources only"}${j.repair_of ? " / fresh repair" : ""} / ${e(j.id.slice(0, 16))}</small></span>${badge(names[j.status] || j.status, j.status === "blocked" ? "bad" : j.status === "published" ? "good" : "")}</button>`).join("") : '<p class="muted">No agent runs yet.</p>'}</section>`
  );
}
function detail(j) {
  const c = j.change;
  const approved = j.decision?.status === "READY";
  const permission = j.information_permitted;
  const canReview =
    permission &&
    !j.violations.length &&
    c?.checks.passed &&
    !j.operation &&
    !["running", "queued", "revising", "revision_failed"].includes(j.status);
  const publish = approved && j.graph_approval_recorded && canReview;
  return (
    `<button class="back" id="back">&lt;- All work</button>` +
    heading(
      j.include_risk
        ? "The patch carries the Risk boundary."
        : "A fresh patch from shared sources.",
      j.goal,
    ) +
    `<div class="status-row">${badge(names[j.status] || j.status, j.status === "blocked" ? "bad" : j.status === "published" ? "good" : "")}<span>${j.include_risk ? "Risk context captured" : "Shared sources only"}</span>${j.repair_of ? "<span>FRESH REPAIR</span>" : ""}</div><div class="flow">${j.include_risk ? '<span class="blocked">Risk investigation</span>' : "Shared sources"}<b>-></b> captured generation <b>-></b> proposed patch <b>-></b> Engineering</div>${j.error ? `<div class="callout">${e(j.error)}</div>` : ""}${j.include_risk && j.status === "blocked" ? `<section class="card"><h2>Start again inside the permitted boundary</h2><p class="muted">Keep this artifact in the Risk-authorized workspace. A fresh generation uses only Engineering requirements and the approved specification. Its context does not include this patch or investigation.</p><button data-action="repair">Generate from shared sources</button><p class="muted">We do not claim the two patches are equivalent. Risk must authorize another specification if the shared sources are insufficient.</p></section>` : ""}${["running", "queued", "revising"].includes(j.status) ? '<section class="card"><h2>Working...</h2><p class="muted">Actual activity appears below. This page updates automatically.</p></section>' : ""}${c ? `<section class="card"><div class="section-heading"><h2>Actual proposed patch</h2>${badge(c.files.length + " FILE(S)")}</div>${c.files.map((f) => `<details class="file"><summary>${e(f.path)} / inspect exact bytes</summary><div class="diff"><div><small>BEFORE</small><pre>${e(j.base_files.find((b) => b.path === f.path)?.content || "New file")}</pre></div><div><small>PROPOSED</small><pre>${e(f.content)}</pre></div></div></details>`).join("")}</section><section class="card"><h2>Publication checks</h2><dl><div><dt>Source permissions + lineage</dt><dd>${permission ? "Pass" : "Blocked"}</dd></div><div><dt>Policy requirements</dt><dd>${e(j.violations.join("; ") || "Pass")}</dd></div><div><dt>Executed Rust tests</dt><dd>${c.checks.passed ? "Passed" : "Failed"}</dd></div><div><dt>Approval for this exact patch</dt><dd>${approved ? "Matches" : j.operation ? "Recorded in outcome" : "Required / no current match"}</dd></div></dl><details><summary>Actual test output</summary><pre>${e(c.checks.command)}\n\n${e(c.checks.output)}</pre></details><div class="actions">${j.status === "published" ? `<a class="button" href="${e(j.operation?.receipt?.provider_id)}" target="_blank" rel="noopener">Open draft PR</a>` : `<button data-action="approve" ${canReview && !approved ? "" : "disabled"}>Approve exact patch</button><button data-action="publish" ${publish ? "" : "disabled"}>Publish draft PR</button>`}</div><p class="muted" id="action-status">${publish ? "Current approval recorded in HydraDB. Publication rechecks evidence before both GitHub operations." : "Approval cannot bypass source permissions. The gate checks current lineage, policy, patch and repository."}</p>${approved && !j.operation ? '<div class="revision-note">Test approval binding: revise the actual patch, rerun its tests and invalidate this approval.<div class="actions"><button class="secondary" data-action="revise">Revise patch after approval</button></div></div>' : ""}</section>` : ""}${sources(j)}<section class="card"><h2>Live activity</h2><ol class="activity">${j.events.map((x) => `<li><time>${new Date(x.at * 1000).toLocaleTimeString()}</time><span>${e(x.message)}</span></li>`).join("")}</ol></section><details class="evidence"><summary>Captured plan + connected evidence</summary><pre>${e(JSON.stringify({ plan: j.plan, provider: j.provider, patch_graph: j.graph_id, decision: j.decision, operation: j.operation }, null, 2))}</pre></details>`
  );
}
function bind() {
  document
    .querySelector(".system-map")
    ?.addEventListener("toggle", (ev) => (mapOpen = ev.target.open));
  document
    .querySelectorAll("[data-job]")
    .forEach((b) => (b.onclick = () => go("/agent/" + b.dataset.job)));
  document.querySelector("#back")?.addEventListener("click", () => go("/"));
  document
    .querySelector("#goal")
    ?.addEventListener("input", (ev) => (goal = ev.target.value));
  document
    .querySelector("#new-work")
    ?.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const b = ev.target.querySelector("button");
      b.disabled = true;
      b.textContent = "Submitting...";
      try {
        const j = await api("/work", {
          goal,
          include_risk: document.querySelector("#include-risk").checked,
        });
        go("/agent/" + j.id);
      } catch (error) {
        notify(error.message);
        b.disabled = false;
        b.textContent = "Run coding agent";
      }
    });
  document.querySelectorAll("[data-action]").forEach(
    (b) =>
      (b.onclick = async () => {
        document
          .querySelectorAll("[data-action]")
          .forEach((x) => (x.disabled = true));
        const status = document.querySelector("#action-status");
        if (status)
          status.textContent =
            b.dataset.action === "revise"
              ? "Revising actual bytes and rerunning Rust tests..."
              : "Reading current evidence from HydraDB...";
        try {
          const j = state.jobs.find((x) => x.id === selected());
          const response = await api(
            `/work/${j.id}/${b.dataset.action}`,
            b.dataset.action === "repair"
              ? {}
              : { context_digest: j.context_digest },
          );
          if (b.dataset.action === "repair") go("/agent/" + response.id);
          else await load();
        } catch (error) {
          notify(error.message);
          load();
        }
      }),
  );
  document.querySelector("#test-policy")?.addEventListener("click", () => {
    candidate = readPolicy("policy");
    result = suite = undefined;
    go("/simulator");
  });
  document.querySelector("#policy")?.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const b = ev.target.querySelector("button:last-of-type");
    b.disabled = true;
    try {
      await api("/policy", {
        policy: readPolicy("policy"),
        activate: true,
        expected_revision: Number(ev.target.dataset.revision),
      });
      notify(
        "Policy activated. Existing work requires a fresh run and review.",
      );
      load();
    } catch (error) {
      notify(error.message);
      b.disabled = false;
    }
  });
  bindSimulator();
}
function bindSimulator() {
  const f = document.querySelector("#simulation");
  if (!f) return;
  f.addEventListener("input", () => {
    readSimulation();
    result = suite = undefined;
    document.querySelector("#results").innerHTML = "";
  });
  document.querySelector("#use-active").onclick = () => {
    readSimulation();
    candidate = clone(state.policy);
    result = suite = undefined;
    render();
  };
  document.querySelectorAll("[data-preset]").forEach(
    (b) =>
      (b.onclick = () => {
        readSimulation();
        draft = preset(b.dataset.preset);
        result = suite = undefined;
        render();
      }),
  );
  document.querySelector("#import-json").onclick = () => {
    try {
      const v = JSON.parse(document.querySelector("#policy-json").value);
      const allowed = [
        "name",
        "allowed_paths",
        "max_changed_files",
        "require_tests",
        "approval_minutes",
        "departments",
      ];
      if (
        !v ||
        Array.isArray(v) ||
        typeof v !== "object" ||
        Object.keys(v).some((k) => !allowed.includes(k)) ||
        allowed.slice(0, 5).some((k) => !Object.hasOwn(v, k)) ||
        typeof v.name !== "string" ||
        !Array.isArray(v.allowed_paths) ||
        v.allowed_paths.some((p) => typeof p !== "string") ||
        typeof v.require_tests !== "boolean" ||
        !Number.isInteger(v.max_changed_files) ||
        !Number.isInteger(v.approval_minutes) ||
        (state.departments &&
          (!v.departments ||
            !Array.isArray(v.departments.investigation_readers) ||
            v.departments.investigation_readers.some(
              (r) => !["engineering", "risk"].includes(r),
            ) ||
            Object.keys(v.departments).some(
              (k) => k !== "investigation_readers",
            )))
      )
        throw Error(
          "Use the supported structured fields shown above. Unknown rules are rejected by Rust.",
        );
      readSimulation();
      candidate = v;
      result = suite = undefined;
      render();
    } catch (error) {
      document.querySelector("#json-error").textContent = error.message;
    }
  };
  async function run(all) {
    readSimulation();
    const controls = [
      ...f.elements,
      ...document.querySelectorAll("[data-preset]"),
    ];
    controls.forEach((x) => (x.disabled = true));
    const out = document.querySelector("#results");
    out.setAttribute("aria-busy", "true");
    out.innerHTML =
      '<p role="status">Evaluating in Rust and recording case evidence in HydraDB...</p>';
    result = suite = undefined;
    try {
      if (all) {
        const rows = [];
        for (const [key, label] of presets) {
          out.innerHTML = `<p role="status">Checking ${e(label)} (${rows.length + 1}/${presets.length})...</p>`;
          rows.push([
            label,
            await api("/simulate", { ...preset(key), policy: candidate }),
          ]);
        }
        suite = rows;
      } else result = await api("/simulate", { ...draft, policy: candidate });
      out.innerHTML = results();
      out.scrollIntoView({ behavior: "smooth", block: "start" });
    } catch (error) {
      out.innerHTML = `<p class="callout" role="alert">${e(error.message)}</p>`;
    } finally {
      controls.forEach((x) => (x.disabled = false));
      out.removeAttribute("aria-busy");
    }
  }
  f.onsubmit = (ev) => {
    ev.preventDefault();
    run(false);
  };
  document.querySelector("#run-suite").onclick = () => run(true);
}
function render() {
  document.title = "TrustGraph / " + page();
  document.querySelectorAll("nav a").forEach((a) => {
    const active =
      (new URL(a.href).pathname === "/"
        ? "workspace"
        : new URL(a.href).pathname.slice(1)) === page();
    a.classList.toggle("active", active);
    if (active) a.setAttribute("aria-current", "page");
    else a.removeAttribute("aria-current");
  });
  app.innerHTML =
    (page() === "policy"
      ? policyPage()
      : page() === "simulator"
        ? simulatorPage()
        : workspace()) + architecture();
  bind();
}
async function load() {
  clearTimeout(timer);
  try {
    state = await api("/work");
    if (!graph) {
      try {
        graph = await api("/graph");
      } catch {
        graph = { connected: false };
      }
    }
    render();
  } catch (error) {
    app.innerHTML = heading(
      "Open your workspace.",
      error.message === "workspace authentication required"
        ? "Operator sign-in required. Open operator.html in your local state folder."
        : error.message,
    );
  }
  if (
    state?.jobs.some((j) =>
      ["queued", "running", "revising"].includes(j.status),
    ) &&
    page() === "workspace"
  )
    timer = setTimeout(load, 2500);
}
document.addEventListener("click", (ev) => {
  const a = ev.target.closest("a");
  if (
    a &&
    a.origin === location.origin &&
    !a.target &&
    !ev.metaKey &&
    !ev.ctrlKey
  ) {
    ev.preventDefault();
    go(a.pathname);
  }
});
window.addEventListener("popstate", load);
async function initialize() {
  const hash = location.hash.split("/");
  if (hash[1] === "connect") {
    const token = hash[2];
    history.replaceState({}, "", "/");
    try {
      const r = await fetch("/api/session", {
        method: "POST",
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!r.ok) throw Error("Operator sign-in failed");
    } catch (error) {
      app.textContent = error.message;
      return;
    }
  } else if (location.search || location.hash) {
    const legacy = ["policy", "simulator", "agent"].includes(hash[1])
      ? "/" + hash.slice(1).join("/")
      : "/";
    history.replaceState({}, "", legacy);
  }
  await load();
}
initialize();
