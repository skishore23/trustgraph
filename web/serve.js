// trustgraph serve operator page. Builds plain DOM (no HTML strings), talks
// only to this origin, and signs in once with the launch file's secret.
'use strict';

(function () {
  var REFRESH_MS = 4000;
  var last = '';
  var confirming = null; // proposal id awaiting the in-page confirmation
  var busy = false;

  function $(id) { return document.getElementById(id); }

  function el(tag, text, cls) {
    var node = document.createElement(tag);
    if (text !== undefined && text !== null) node.textContent = text;
    if (cls) node.className = cls;
    return node;
  }

  function button(label, key, primary, onClick) {
    var b = el('button', label, primary ? 'primary' : '');
    b.type = 'button';
    b.setAttribute('data-key', key);
    b.addEventListener('click', onClick);
    return b;
  }

  function notice(text) {
    var n = $('notice');
    n.textContent = text || '';
    n.hidden = !text;
  }

  function list(names) {
    return names && names.length ? names.join(', ') : 'nobody';
  }

  function clock(seconds) {
    var d = new Date(seconds * 1000);
    var pad = function (n) { return (n < 10 ? '0' : '') + n; };
    return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) +
      ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes());
  }

  function request(method, path, body) {
    var options = { method: method, credentials: 'same-origin', headers: {} };
    if (body !== undefined) {
      options.headers['Content-Type'] = 'application/json';
      options.body = JSON.stringify(body);
    }
    return fetch(path, options).then(function (response) {
      return response.json().catch(function () { return {}; }).then(function (data) {
        return { status: response.status, data: data };
      });
    });
  }

  function message(result) {
    var error = result.data && result.data.error;
    return error ? error.message + ' (' + error.code + ')' : 'request failed (' + result.status + ')';
  }

  function act(path, body, done) {
    if (busy) return;
    busy = true;
    request('POST', path, body).then(function (result) {
      busy = false;
      notice(result.status === 200 ? done : message(result));
      load(true);
    }, function () {
      busy = false;
      notice('The server did not answer. Is trustgraph serve still running?');
    });
  }

  function empty(container, text) {
    container.appendChild(el('p', text, 'muted'));
  }

  function renderPending(items) {
    var box = $('pending');
    box.replaceChildren();
    if (!items.length) return empty(box, 'Nothing is waiting.');
    items.forEach(function (p) {
      var item = el('div', null, 'item');
      item.appendChild(el('p', p.agent + ' wants ' + p.action + ' → ' + list(p.recipients)));
      item.appendChild(el('p', 'derived from: ' + list(p.sources)));
      item.appendChild(el('p', p.reason, 'muted'));
      item.appendChild(el('p', 'asked ' + clock(p.requested_at), 'muted'));
      item.appendChild(button('Approve', 'approve:' + p.decision, true, function () {
        act('/v1/operator/approve', { decision: p.decision }, 'Approved. ' + p.agent + ' may now ' + p.action + '.');
      }));
      box.appendChild(item);
    });
  }

  function renderApprovals(items) {
    var box = $('approvals');
    box.replaceChildren();
    if (!items.length) return empty(box, 'No live approvals.');
    items.forEach(function (a) {
      var item = el('div', null, 'item');
      item.appendChild(el('p', a.agent + ' may ' + a.action + ' → ' + list(a.recipients)));
      item.appendChild(el('p', 'until ' + clock(a.expires_at), 'muted'));
      item.appendChild(button('Revoke', 'revoke:' + a.decision, false, function () {
        act('/v1/operator/revoke', { decision: a.decision }, 'Revoked.');
      }));
      box.appendChild(item);
    });
  }

  function renderProposals(items) {
    var box = $('proposals');
    box.replaceChildren();
    if (!items.length) return empty(box, 'No proposals.');
    items.forEach(function (p) {
      var item = el('div', null, 'item');
      item.appendChild(el('p', p.from === 'operator' ? 'Proposed by you' : 'Proposed by ' + p.from));
      if (p.widens) item.appendChild(el('p', 'This gives more people access.', 'warn'));
      String(p.explanation || '').split('\n').forEach(function (line) {
        if (line.trim()) item.appendChild(el('p', line));
      });
      item.appendChild(el('p', 'changes from the active policy:', 'muted'));
      var changes = el('ul', null, 'changes');
      if (!p.changes.length) changes.appendChild(el('li', 'No changes from the active policy.', 'muted'));
      p.changes.forEach(function (c) {
        var tag = c.kind === 'widens' ? 'WIDENS ' : c.kind === 'narrows' ? 'narrows ' : 'neutral ';
        changes.appendChild(el('li', tag + c.subject + ': ' + c.summary, c.kind === 'widens' ? 'warn' : ''));
      });
      item.appendChild(changes);
      if (confirming === p.proposal) {
        item.appendChild(el('p', 'Activate this policy? Every approval is voided and agents must ask again.'));
        item.appendChild(button('Activate now', 'activate-now:' + p.proposal, true, function () {
          confirming = null;
          act('/v1/operator/activate', { proposal: p.proposal }, 'Activated. Agent tokens follow the new policy.');
        }));
        item.appendChild(button('Cancel', 'cancel:' + p.proposal, false, function () {
          confirming = null;
          load(true, 'activate:' + p.proposal);
        }));
      } else {
        item.appendChild(button('Activate', 'activate:' + p.proposal, true, function () {
          confirming = p.proposal;
          load(true, 'activate-now:' + p.proposal);
        }));
      }
      box.appendChild(item);
    });
  }

  function render(state, focusKey) {
    var active = document.activeElement;
    var key = focusKey || (active && active.getAttribute && active.getAttribute('data-key'));
    var policy = state.policy;
    $('who').textContent = 'Operator ' + state.operator + '. Policy revision ' + policy.revision +
      (policy.source === 'file' ? ', from the policy file.' : ', activated in service.');
    $('signed-in').hidden = false;
    renderPending(state.pending);
    renderApprovals(state.approvals);
    renderProposals(state.proposals);
    if (key) {
      var target = document.querySelector('[data-key="' + key.replace(/"/g, '') + '"]');
      if (target) target.focus();
    }
  }

  function signedOut(text) {
    $('signed-in').hidden = true;
    $('who').textContent = text;
    last = '';
  }

  function load(force, focusKey) {
    return request('GET', '/v1/operator/state').then(function (result) {
      if (result.status === 401) {
        return signedOut('Not signed in. Open the operator launch file that trustgraph serve printed; it works once per start.');
      }
      if (result.status !== 200) return signedOut(message(result));
      var state = result.data;
      var copy = Object.assign({}, state);
      delete copy.now;
      var text = JSON.stringify(copy);
      // Re-render only on change, so focus and reading position stay put.
      if (force || text !== last) {
        last = text;
        render(state, focusKey);
      }
    }, function () {
      signedOut('The server did not answer. Is trustgraph serve still running?');
    });
  }

  function connect() {
    var match = /^#\/connect\/([0-9a-f]{64})$/.exec(location.hash);
    if (!match) return Promise.resolve();
    // Drop the secret from the address bar and history before using it.
    history.replaceState(null, '', location.pathname);
    return fetch('/session', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { Authorization: 'Bearer ' + match[1] }
    }).then(function (response) {
      if (response.status !== 200) notice('This launch link was already used. Restart trustgraph serve for a new one.');
    }, function () {});
  }

  document.addEventListener('DOMContentLoaded', function () {
    $('refresh').addEventListener('click', function (event) {
      event.preventDefault();
      notice('');
      load(true);
    });
    connect().then(function () { return load(true); });
    setInterval(function () {
      if (!busy && confirming === null && !document.hidden) load(false);
    }, REFRESH_MS);
  });
})();
