# Consulting Permission Simulator Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans. Implement in this session and use an independent final review.

**Goal:** Make a consulting acquisition decision understandable through an interactive, enforced permission simulation.

**Architecture:** Add consulting fixtures and a stateless scenario API over the existing plan compiler. Prepare the exact previewed plan into the existing review and guarded publisher. A separate UI module renders the business workflow.

**Tech Stack:** Rust, Axum, existing vanilla ES modules and CSS. No new dependencies.

**Spec:** docs/superpowers/specs/2026-11-14-consulting-simulator-design.md

## Global constraints

- Single workspace; all companies and values synthetic; no external delivery.
- Preserve existing Northstar tests and journal data.
- Preview never writes; server is authoritative; private source metadata remains scoped.
- Green permission preview still requires human approval of actual content.
- No copy from beetle-bot.

## Review focus

- Hidden private-source metadata: Sam receives only authorized documents and cannot simulate unauthorized inputs.
- Stale preview: mismatched plan/snapshot digests reject before state changes.
- Repeat clicks: same prepared configuration reuses the run and review.
- Input omission: instructions remain a real generation dependency.
- Familiar labels: narrow screen still shows meaningful source and recipient names; advanced diagnostics do not dominate.

### Task 1: Consulting fixture and enforced scenario API

Files: runtime fixtures.rs, engine.rs, plans.rs, publisher.rs; CLI scenario.rs, api.rs, main.rs, lib.rs; tests/consulting_workflow.rs.

- [x] Write and run behavioral HTTP tests against consulting_router: default blocked Sam/Orion; omit benchmark and narrow team both pass; preview doesn't persist plans/artifacts; private input rejected; malformed selection rejected; blocked/stale prepare doesn't mutate; safe prepare has exact lineage; repeat prepare is idempotent; review releases to simulated Atlas destination.
- [x] Add Coordinator::open_with_evidence, preserving open's Northstar behavior and persisted genesis. Reject consulting router on a different persisted workspace.
- [x] Implement consulting fixture, plan builder, source-derived fixture renderer and explicitly allowed synthetic publisher namespace.
- [x] Add GET /api/scenario, POST /api/scenario/simulate and POST /api/scenario/prepare. Return report, matrix and exact plan; validate authorization first. Recompute and compare digests before proposing/preparing.
- [x] Run consulting tests and workspace suite.

### Task 2: Decision-focused UI and business case

Files: web/simulator.js, web/simulator.css, web/app.js, web/index.html; docs/business-case.md, docs/demo-script.md, README.md.

- [x] Default route to simulator, source checkboxes, destination radios, matrix, conflict explanation, two fixes, reset and prepare handoff. Keep review pages and advanced plans/checker.
- [x] Replace primary launch/channel language with consulting memo language; retain legacy fixture support.
- [x] Document buyer, decision, failure, value, adoption wedge, measurable pilot criteria and limitations. Cite primary sources for external facts; label scenario and economics assumptions.
- [x] Build, start consulting workspace, exercise browser controls and actual approval/delivery, capture local screenshots.
- [x] Independent review, fix material findings, run scripts/check.sh, update plan checkboxes and report result.


## Execution record

Implemented in the existing workspace without deleting Northstar data or creating a commit. Builder and final reviews were read-only. They caught a private-source existence oracle and misleading returned-review guidance; both were fixed and checked.

Ruling: the fixture renderer does not apply prose feedback. A returned plan review points users to a revised agent plan, or a different source/audience selection; the UI does not pretend that the generic revision endpoint can release a substituted plan artifact. Repeated preparation opens the existing review/receipt. Reset changes selection only and never deletes history.

Validation: `scripts/check.sh` passed (60 tests, zero failures, one explicitly ignored live Hydra contract); all three JavaScript modules pass `node --check`; the app builds. A browser test at its native 688px width covered source selection, both audience alternatives, actual memo preparation, approval/local delivery, restart persistence, Sam's scoped view, partner feedback and returned-plan guidance. No horizontal overflow or browser errors were observed. Screenshots and check output are in evidence/consulting-*.
