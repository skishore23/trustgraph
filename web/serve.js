// trustgraph serve operator page. Builds plain DOM (no HTML strings), talks
// only to this origin, and signs in with the launch file's single-use secret.
// The session id lives in sessionStorage (this origin and port only) and is
// sent as X-TrustGraph-Operator; there is no cookie.
'use strict';

(function () {
  var SESSION_KEY = 'trustgraph-operator-session';
  var REPLACED = 'Signed out: a newer operator session was started. If that was not you, stop the server.';
  var SIGNED_OUT = 'Not signed in. Open the newest operator launch file (operator.html in the state directory); each one works once.';
  var GONE = 'That request is no longer pending.';
  var LINK_USED = 'This sign-in link is not valid or was already used. If you did not use it, stop the server: someone else holds the operator session.';

  function list(names) {
    return names && names.length ? names.join(', ') : 'nobody';
  }

  function shortId(id) {
    return String(id || '').slice(0, 12);
  }

  // Exactly what an approval grants, repeated in the confirmation step.
  function approvalSummary(p) {
    return 'Approve: ' + p.agent + ' may ' + p.action + ' to ' + list(p.recipients) +
      ', from ' + list(p.sources) + ' (decision ' + shortId(p.decision) + '…).';
  }

  // The confirmation keys the page would render for `state`: an Approve for
  // each pending decision the operator can approve, an Activate for each
  // proposal, an End for each gateway session.
  function confirmableKeys(state) {
    var keys = [];
    (state.pending || []).forEach(function (p) {
      if (p.can_approve) keys.push('approve:' + p.decision);
    });
    (state.proposals || []).forEach(function (p) {
      keys.push('activate:' + p.proposal);
    });
    gatewaySessions(state).forEach(function (s) {
      keys.push('end:' + s.session);
    });
    return keys;
  }

  function gatewaySessions(state) {
    return (state.gateway && state.gateway.sessions) || [];
  }

  // Ending a session makes the gateway forget what it read, so the
  // confirmation says exactly what the agent stops carrying.
  function endSummary(s) {
    return 'End this session? ' + s.agent + "'s later actions will no longer count what it read" +
      (s.sources && s.sources.length ? ': ' + s.sources.join(', ') + '.' : ' (nothing yet).');
  }

  // True when a confirmation is open for an item `state` no longer has
  // (expired, evicted, voided by an activation): it must be dropped, or the
  // periodic refresh would stay paused for good.
  function staleConfirmation(state, confirming) {
    return confirming !== null && confirmableKeys(state).indexOf(confirming) === -1;
  }

  function proposer(p) {
    if (p.origin === 'operator') return 'Proposed by you';
    if (p.origin === 'file') return 'From the policy file (anyone who can edit the file could have written this)';
    return 'Proposed by agent ' + p.from;
  }

  function signedOutMessage(result) {
    var code = result && result.data && result.data.error && result.data.error.code;
    return code === 'operator_session_replaced' ? REPLACED : SIGNED_OUT;
  }

  // Never re-render the lists under the operator's pointer or focus: hold
  // the newest state and say so, until they ask or stop interacting.
  function createScheduler(options) {
    var queued = null;
    function show(state, focusKey) {
      queued = null;
      options.queued(false);
      options.render(state, focusKey);
    }
    return {
      offer: function (state, force, focusKey) {
        if (!force && options.interacting()) {
          queued = state;
          options.queued(true);
          return false;
        }
        show(state, focusKey);
        return true;
      },
      flush: function () {
        if (queued === null) return false;
        show(queued);
        return true;
      },
      hasQueued: function () { return queued !== null; }
    };
  }

  var api = {
    GONE: GONE,
    approvalSummary: approvalSummary,
    confirmableKeys: confirmableKeys,
    endSummary: endSummary,
    proposer: proposer,
    staleConfirmation: staleConfirmation,
    createScheduler: createScheduler,
    list: list,
    shortId: shortId,
    signedOutMessage: signedOutMessage
  };
  if (typeof module === 'object' && module.exports) {
    module.exports = api;
    return;
  }

  var REFRESH_MS = 4000;
  var last = '';
  var confirming = null; // 'approve:<id>' or 'activate:<id>' awaiting confirmation
  var busy = false;
  var pointerInside = false;
  var memorySession = null;

  function $(id) { return document.getElementById(id); }

  function getSession() {
    try {
      var stored = window.sessionStorage.getItem(SESSION_KEY);
      if (stored) return stored;
    } catch (e) { /* storage blocked: fall back to memory */ }
    return memorySession;
  }

  function setSession(value) {
    memorySession = value || null;
    try {
      if (value) window.sessionStorage.setItem(SESSION_KEY, value);
      else window.sessionStorage.removeItem(SESSION_KEY);
    } catch (e) { /* storage blocked: memory only */ }
  }

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

  function clock(seconds) {
    var d = new Date(seconds * 1000);
    var pad = function (n) { return (n < 10 ? '0' : '') + n; };
    return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) +
      ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes());
  }

  function request(method, path, body) {
    var options = { method: method, credentials: 'omit', headers: {} };
    var session = getSession();
    if (session) options.headers['X-TrustGraph-Operator'] = session;
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
      if (result.status === 401) {
        setSession(null);
        return signedOut(signedOutMessage(result));
      }
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

  function confirmStep(item, key, question, yesLabel, onYes) {
    item.appendChild(el('p', question));
    item.appendChild(button(yesLabel, 'yes:' + key, true, function () {
      confirming = null;
      onYes();
    }));
    item.appendChild(button('Cancel', 'cancel:' + key, false, function () {
      confirming = null;
      load(true, key);
    }));
  }

  function renderPending(items) {
    var box = $('pending');
    box.replaceChildren();
    if (!items.length) return empty(box, 'Nothing is waiting.');
    items.forEach(function (p) {
      var key = 'approve:' + p.decision;
      var item = el('div', null, 'item');
      item.appendChild(el('p', p.agent + ' wants ' + p.action + ' → ' + list(p.recipients)));
      item.appendChild(el('p', 'derived from: ' + list(p.sources)));
      item.appendChild(el('p', p.reason, 'muted'));
      item.appendChild(el('p', 'asked ' + clock(p.requested_at) + ' · decision ' + shortId(p.decision), 'muted'));
      if (!p.can_approve) {
        item.appendChild(el('p', p.cannot_approve || 'you are not a reviewer for this action', 'muted'));
      } else if (confirming === key) {
        confirmStep(item, key, approvalSummary(p), 'Approve now', function () {
          act('/v1/operator/approve', { decision: p.decision }, 'Approved. ' + p.agent + ' may now ' + p.action + '.');
        });
      } else {
        item.appendChild(button('Approve', key, true, function () {
          confirming = key;
          load(true, 'yes:' + key);
        }));
      }
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
      item.appendChild(el('p', 'until ' + clock(a.expires_at) + ' · decision ' + shortId(a.decision), 'muted'));
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
      var key = 'activate:' + p.proposal;
      var item = el('div', null, 'item');
      item.appendChild(el('p', proposer(p) + ' · ' + shortId(p.proposal)));
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
      if (confirming === key) {
        confirmStep(item, key, 'Activate this policy? Every approval is voided and agents must ask again.', 'Activate now', function () {
          act('/v1/operator/activate', { proposal: p.proposal }, 'Activated. Agent tokens follow the new policy.');
        });
      } else {
        item.appendChild(button('Activate', key, true, function () {
          confirming = key;
          load(true, 'yes:' + key);
        }));
      }
      box.appendChild(item);
    });
  }

  function renderSessions(state) {
    var gateway = state.gateway;
    $('gateway').hidden = !gateway;
    if (!gateway) return;
    var problems = $('gateway-problems');
    problems.replaceChildren();
    (gateway.problems || []).forEach(function (line) {
      problems.appendChild(el('p', line, 'warn'));
    });
    var box = $('sessions');
    box.replaceChildren();
    var sessions = gatewaySessions(state);
    if (!sessions.length) return empty(box, 'No live sessions (' + gateway.mode + ' mode).');
    sessions.forEach(function (s) {
      var key = 'end:' + s.session;
      var item = el('div', null, 'item');
      item.appendChild(el('p', s.agent + ' · since ' + clock(s.started_at)));
      item.appendChild(el('p', 'carries: ' + (s.sources.length ? s.sources.join(', ') : 'nothing yet')));
      item.appendChild(el('p', 'session ' + s.session, 'muted'));
      if (confirming === key) {
        confirmStep(item, key, endSummary(s), 'End now', function () {
          act('/v1/operator/end-session', { session: s.session }, 'Ended the session of ' + s.agent + '.');
        });
      } else {
        item.appendChild(button('End session', key, false, function () {
          confirming = key;
          load(true, 'yes:' + key);
        }));
      }
      box.appendChild(item);
    });
  }

  function render(state, focusKey) {
    var active = document.activeElement;
    var key = focusKey || (active && active.getAttribute && active.getAttribute('data-key'));
    var policy = state.policy;
    if (staleConfirmation(state, confirming)) {
      confirming = null;
      notice(GONE);
    }
    $('who').textContent = 'Operator ' + state.operator + '. Policy revision ' + policy.revision +
      (policy.source === 'file' ? ', from the policy file.' : ', activated in service.');
    $('clock').hidden = !state.clock_behind;
    $('signed-in').hidden = false;
    renderPending(state.pending);
    renderApprovals(state.approvals);
    renderProposals(state.proposals);
    renderSessions(state);
    if (key) {
      var target = document.querySelector('[data-key="' + key.replace(/"/g, '') + '"]');
      if (target) target.focus();
    }
  }

  function interacting() {
    var lists = $('lists');
    return pointerInside || (lists.contains(document.activeElement) && document.activeElement !== lists);
  }

  var scheduler = createScheduler({
    render: render,
    interacting: interacting,
    queued: function (on) { $('updated').hidden = !on; }
  });

  function signedOut(text) {
    $('signed-in').hidden = true;
    $('who').textContent = text;
    last = '';
  }

  function load(force, focusKey) {
    if (!getSession()) {
      signedOut(SIGNED_OUT);
      return Promise.resolve();
    }
    return request('GET', '/v1/operator/state').then(function (result) {
      if (result.status === 401) {
        setSession(null);
        return signedOut(signedOutMessage(result));
      }
      if (result.status !== 200) return signedOut(message(result));
      var state = result.data;
      var copy = Object.assign({}, state);
      delete copy.now;
      var text = JSON.stringify(copy);
      // Re-render only on change, so focus and reading position stay put.
      if (force || text !== last) {
        last = text;
        scheduler.offer(state, force, focusKey);
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
      credentials: 'omit',
      headers: { Authorization: 'Bearer ' + match[1] }
    }).then(function (response) {
      return response.json().catch(function () { return {}; }).then(function (data) {
        if (response.status === 200 && data.session) setSession(data.session);
        else notice(LINK_USED);
      });
    }, function () {});
  }

  document.addEventListener('DOMContentLoaded', function () {
    var lists = $('lists');
    lists.addEventListener('mouseenter', function () { pointerInside = true; });
    lists.addEventListener('mouseleave', function () { pointerInside = false; });
    $('refresh').addEventListener('click', function (event) {
      event.preventDefault();
      // Always a way out: drop any open confirmation and held update.
      confirming = null;
      notice('');
      load(true);
    });
    $('updated').addEventListener('click', function (event) {
      event.preventDefault();
      if (!scheduler.flush()) load(true);
    });
    connect().then(function () { return load(true); });
    setInterval(function () {
      if (!busy && confirming === null && !document.hidden) load(false);
    }, REFRESH_MS);
  });
})();
