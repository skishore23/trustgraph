# Security reports

TrustGraph is an experimental security framework. Its supported boundary and
trusted components are described in [the gateway threat model](docs/gateway.md#protected-preview-threat-model).
Version 1 gateway configurations do not provide the version 2 protected boundary.

Report vulnerabilities using [GitHub private vulnerability reporting](https://github.com/skishore23/trustgraph/security/advisories/new).
If private reporting is unavailable, open an issue asking the maintainer to
enable a private reporting channel, without including vulnerability details.
Do not post credentials, confidential payloads, or an exploit in a public issue.

Include the commit, operating system, configuration version, affected tool,
expected decision, observed forwarding, and minimal reproduction using fake
data. A redacted journal and upstream call log help distinguish a policy error
from a forwarding or context-restoration failure. Report bypasses, provenance
loss, approval reuse, misclassification, and unintended reader widening.

There is no response-time or supported-release commitment yet. Security fixes
are made on `main`; reproduce issues against its latest commit when possible.
