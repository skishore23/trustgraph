# Agent handoffs

TrustGraph accepts a tracked artifact and a proposed destination as a review
request. Existing agents remain responsible for preparation. The review app does
not replace their editor or task runtime.

The local preview uses the fixed bearer identity `demo-agent`. The human review
identity is separate. These are synthetic development credentials, not deployed
authentication. All examples target your local server.

## Record and submit

The fixture broker is `POST /api/drafts/generate` with
`{"restricted":false,"key":"my-broker-run"}`. It returns an artifact ID. A real
provider adapter must retain broker-owned context capture before replacing this
fixture. Arbitrary content with caller-declared provenance is not accepted.

Submit a request with `POST /api/requests`:

```json
{
  "key": "roadmap-update-001",
  "title": "Share the roadmap update",
  "summary": "Prepared for the company channel.",
  "artifact": "draft-my-broker-run",
  "audience": "company"
}
```

The key is a stable request identity, up to 64 ASCII letters/digits or `-_.`.
An identical retry returns the current authorized resource. A different payload
with the same key conflicts. The agent can read its own requests, subject to
current access to every historical request artifact.

## Feedback and resubmission

Read `GET /api/requests/{id}` to inspect status, current request revision and
reviewer feedback. The local app records feedback; it does not send an external
notification to an agent service.

An edit through `POST /api/artifacts/{id}/versions` retains the previous draft as
an input. For a clean regeneration, start a fresh broker run with the intended
source set. Neither path can erase provenance from an existing artifact.

Submit the new tracked artifact through `POST /api/requests/{id}/revisions`:

```json
{"revision": 1, "artifact": "edit-the-new-tracked-version"}
```

`revision` is the expected current request revision. An outdated request conflicts
rather than replacing someone else's later work. Feedback remains in history.
A released or uncertain delivery is not reopened for another send.

## Human decisions

Only authorized human reviewers can return or release requests.
`POST /api/requests/{id}/changes` takes `revision`, `body` and an idempotency `key`.
It records feedback and revokes matching existing approvals in one journal event.

`POST /api/requests/{id}/release` takes `revision` and `context_digest` from the
current detail response. The server rechecks source permission, reviewer
authority and the exact binding before publication. Request-to-operation linkage
is journaled with reservation before any send. Repeated clicks or another
reviewer's identical retry return the original operation.

Uncertain outcomes are reconciled with the existing operation endpoint. There is
no automatic resend. Publication is still a local simulator in this preview.
