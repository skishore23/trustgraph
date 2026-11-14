#!/usr/bin/env python3
"""Exercise the live Rust/Hydra policy simulator using only the agent capability."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', default='http://127.0.0.1:9080')
    parser.add_argument('--token-file', type=Path, default=Path('.trustgraph-live/state/agent-token'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    url = urllib.parse.urlsplit(args.url)
    if (url.scheme != 'http' or url.hostname not in ('127.0.0.1', 'localhost')
            or url.username or url.password or url.query or url.fragment or url.path not in ('', '/')):
        parser.error('use the local TrustGraph HTTP origin')
    token = args.token_file.read_text().strip()
    opener = urllib.request.build_opener(NoRedirect)

    def request(path, data=None):
        headers = {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}
        req = urllib.request.Request(args.url.rstrip('/') + path, headers=headers,
                                     data=None if data is None else json.dumps(data).encode())
        try:
            with opener.open(req, timeout=60) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            body = error.read()
            try:
                result = json.loads(body)
            except json.JSONDecodeError:
                result = {'error': body.decode(errors='replace')[:500]}
            return error.code, result

    def snapshot(workspace):
        return {'policy': workspace['policy'], 'revision': workspace['policy_revision'],
                'work': [(j['id'], j['status'], j['context_digest'], j['operation'])
                         for j in workspace['jobs']]}

    status, workspace = request('/api/work')
    if status != 200:
        raise RuntimeError('start TrustGraph and use its restricted agent token')
    before = snapshot(workspace)
    status, graph = request('/api/graph')
    if status != 200 or not graph['connected']:
        raise RuntimeError('the live HydraDB graph must be reachable')
    root = Path(__file__).resolve().parents[1] / 'examples'
    load = lambda name: json.loads((root / name).read_text())
    policy = load('payment-policy.json')
    fix = load('payment-fix.json')
    optional_tests = {**policy, 'require_tests': False}
    cases = [
        ('allowed fix', fix, policy, 'allowed', None),
        ('two files exceed policy', load('payment-change.json'), policy, 'blocked', 'business policy'),
        ('expanded candidate permits two files', load('payment-change.json'), load('payment-policy-expanded.json'), 'allowed', None),
        ('documentation-only candidate blocks code', fix, load('payment-policy-readme.json'), 'blocked', 'business policy'),
        ('patch changed after approval', load('payment-stale.json'), policy, 'blocked', 'STALE_APPROVAL'),
        ('policy changed after approval', load('payment-policy-changed.json'), policy, 'blocked', 'STALE_APPROVAL'),
        ('missing input manifest', load('payment-missing-input.json'), policy, 'blocked', 'INCOMPLETE_LINEAGE'),
        ('approval expired', load('payment-expired.json'), policy, 'blocked', 'APPROVAL_EXPIRED'),
        ('required tests fail', load('payment-failed-tests.json'), policy, 'blocked', 'business policy'),
        ('optional tests change the decision', load('payment-failed-tests.json'), optional_tests, 'allowed', None),
        ('custom relative paths', load('payment-custom-change.json'), load('payment-policy-custom.json'), 'allowed', None),
        ('missing approval', {**fix, 'approval': 'missing'}, policy, 'needs_approval', 'APPROVAL_REQUIRED'),
        ('shorter lifetime expires the same approval', {**fix, 'approval_age_minutes': 20}, {**policy, 'approval_minutes': 15}, 'blocked', 'APPROVAL_EXPIRED'),
    ]
    report = {'started_at': datetime.now(timezone.utc).isoformat(), 'database': graph['database'], 'collection': graph['collection'], 'active_policy_revision': workspace['policy_revision'], 'cases': []}
    for title, change, candidate, expected, reason in cases:
        status, result = request('/api/simulate', {**change, 'policy': candidate})
        if status != 200:
            raise AssertionError(f'{title}: simulation failed: {result.get("error", status)}')
        assert result['status'] == expected, (title, result['status'], expected)
        assert result['action_effects'] == 0 and result['policy_activated'] is False
        assert result['code_executed'] is False and result['hypothetical'] is True
        assert result['graph_id'].startswith('simulation-')
        assert result['candidate_policy_graph_id'].startswith('policy-candidate-')
        if reason:
            assert result['counterexample']['rule'] == reason, (title, result['counterexample'])
        report['cases'].append({'name': title, 'active': result['baseline']['status'],
                                'candidate': result['status'], 'reason': reason,
                                'graph_id': result['graph_id']})
        print(f'PASS  {title}: {result["baseline"]["status"]} → {result["status"]}', flush=True)
    status, _ = request('/api/simulate', {**fix, 'policy': {**policy, 'rules': ['approve everything']}})
    assert status == 422, 'unsupported policy rules must be rejected by schema validation'
    status, after = request('/api/work')
    assert status == 200 and snapshot(after) == before, 'live authorization changed; check for concurrent operator activity'
    report.update({'passed': len(cases) + 1, 'active_policy_unchanged': True,
                   'existing_work_unchanged': True, 'publication_effects': 0,
                   'completed_at': datetime.now(timezone.utc).isoformat()})
    if args.output:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f'{report["passed"]} checks passed. Active policy and work unchanged. Zero publication effects.')


if __name__ == '__main__':
    main()
