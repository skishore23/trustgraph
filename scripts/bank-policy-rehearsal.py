#!/usr/bin/env python3
"""Test supported policy variants against the live Rust evaluator and HydraDB.

Read-only to live policy/repository. Writes simulation evidence to HydraDB.
Run from the TrustGraph checkout; the agent token cannot approve or publish.
"""
import argparse
import json
from pathlib import Path
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', default='http://127.0.0.1:8080')
parser.add_argument('--config', default='trustgraph.local.json')
parser.add_argument('--output', default='.trustgraph-live/bank-policy-rehearsal.json')
args = parser.parse_args()
config = json.loads(Path(args.config).read_text())
token = (Path(config['directory']) / 'agent-token').read_text().strip()

def call(path, body=None):
    request = urllib.request.Request(
        args.url.rstrip('/') + '/api' + path,
        data=json.dumps(body).encode() if body is not None else None,
        headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=240) as response:
        return json.load(response)

before = call('/work')
assert before['departments'], 'Start the bank department workspace first.'
policy = before['policy']
assert policy['departments']['investigation_readers'] == ['risk'], 'Rehearse against the Risk-only policy.'
base = dict(paths=['src/lib.rs'], tests_passed=True, lineage_complete=True,
            approval='valid', approval_age_minutes=0, include_risk=False, audience='engineering')
cases = [
    ('approved inputs', {}, {}, 'allowed', 'allowed'),
    ('Risk to Engineering', {'include_risk': True}, {}, 'blocked', 'blocked'),
    ('Risk to Risk', {'include_risk': True, 'audience': 'risk'}, {}, 'allowed', 'allowed'),
    ('patch changed', {'approval': 'stale'}, {}, 'blocked', 'blocked'),
    ('audience changed', {'approval': 'destination'}, {}, 'blocked', 'blocked'),
    ('policy changed', {'approval': 'previous_policy'}, {}, 'blocked', 'blocked'),
    ('approval missing', {'approval': 'missing'}, {}, 'needs_approval', 'needs_approval'),
    ('approval expired', {'approval': 'expired', 'approval_age_minutes': 1441}, {}, 'blocked', 'blocked'),
    ('unknown lineage', {'lineage_complete': False}, {}, 'blocked', 'blocked'),
    ('tests failed', {'tests_passed': False}, {}, 'blocked', 'blocked'),
    ('explicit source grant', {'include_risk': True}, {'departments': {'investigation_readers': ['risk', 'engineering']}}, 'blocked', 'allowed'),
    ('source grant cannot repair lineage', {'include_risk': True, 'lineage_complete': False}, {'departments': {'investigation_readers': ['risk', 'engineering']}}, 'blocked', 'blocked'),
    ('shorter approval lifetime', {'approval_age_minutes': 10}, {'approval_minutes': 5}, 'allowed', 'blocked'),
    ('test requirement removed', {'tests_passed': False}, {'require_tests': False}, 'blocked', 'allowed'),
    ('expanded file scope', {'paths': ['src/lib.rs', 'README.md']}, {'allowed_paths': ['src/lib.rs', 'README.md'], 'max_changed_files': 2}, 'blocked', 'allowed'),
    ('custom module scope', {'paths': ['payments/reconcile.rs']}, {'allowed_paths': ['payments/reconcile.rs']}, 'blocked', 'allowed'),
]
results = []
for name, scenario, variant, active_status, candidate_status in cases:
    candidate = {**policy, **variant}
    result = call('/simulate', {**base, **scenario, 'policy': candidate})
    actual = (result['baseline']['status'], result['status'])
    assert actual == (active_status, candidate_status), (name, actual, active_status, candidate_status)
    assert result['action_effects'] == 0 and result['policy_activated'] is False
    assert result['graph_id'] and result['candidate_policy_graph_id']
    results.append({'case': name, 'active': actual[0], 'candidate': actual[1], 'graph_id': result['graph_id']})
    print(f"PASS {name}: {actual[0]} -> {actual[1]}", flush=True)
after = call('/work')
assert after['policy'] == before['policy'] and after['policy_revision'] == before['policy_revision']
assert [(j['id'], j['status']) for j in after['jobs']] == [(j['id'], j['status']) for j in before['jobs']]
output = Path(args.output)
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps({'cases': results, 'active_policy_unchanged': True, 'live_jobs_unchanged': True, 'github_writes': 0}, indent=2))
print(f'{len(results)} cases passed. Evidence: {output}')
