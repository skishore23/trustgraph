# Paste into the 8090 project agent

Read WO-2 in this project and draft one GitHub issue proposal. This is an
operator-mediated handoff to TrustGraph. Do not create or edit issues, pull
requests, files, work orders, or other records. Do not start Codex or a coding
workspace. Return only a valid JSON object inside one fenced `json` block:

```json
{
  "key": "wo-2-live-demo-20261114",
  "title": "Demo: your concise issue title",
  "body": "A fictional task based on WO-2, with acceptance criteria. Escape newlines inside this string.",
  "work_order": {
    "project_id": "6b23e5ca-1d03-46a7-93dd-6a3608432db7",
    "id": "WO-2"
  }
}
```

Preserve the key and project identifiers exactly. Include no other fields.
Drafting is neither permission nor human approval. Do not claim this proposal
was submitted or published. The human will paste it into TrustGraph and review
the exact title, body, repository, connected account and configured tool binding.
The Connect adapter binds an input schema hash, not a dated provider version.

For another distinct issue after publication, use a new proposal key. To retry
the same import, reuse the same key and exact content. Changed content with an
existing key is rejected; revisions require a fresh human review.
