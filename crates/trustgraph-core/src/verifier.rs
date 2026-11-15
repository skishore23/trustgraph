use crate::{digest, lineage, *};
/// Evaluate a materialized snapshot at an explicit timestamp.
///
/// `Ready` additionally requires that the requester is an enabled human in the
/// workspace (agents may request review but never execute a release), and that
/// no revoked approval for the same exact context was issued at or after the
/// selected approval: revoking any approval for a context blocks every approval
/// for that context issued at or before the revoked one; a strictly newer
/// approval can re-authorize.
pub fn verify(e: &Evidence, intent: &Intent, now: u64) -> Decision {
    let block = |code, witness| Decision::Blocked {
        violations: vec![Violation { code, witness }],
    };
    let Some(actor) = e.principals.get(&intent.requester) else {
        return block(Code::CallerUnauthorized, None);
    };
    if !actor.enabled || actor.id != intent.requester || actor.workspace != e.workspace {
        return block(Code::CallerUnauthorized, None);
    }
    if !e.complete {
        return block(Code::IncompleteLineage, None);
    }
    if e.policy.workspace != e.workspace {
        return block(Code::CrossWorkspace, None);
    }
    if e.policy.revision == 0 || e.policy.max_approval_lifetime == 0 {
        return block(Code::InvalidSchema, None);
    }
    let closure = match lineage::load(e, &intent.artifact) {
        Ok(c) => c,
        Err(code) => return block(code, None),
    };
    if !closure.readers.contains(&intent.requester) {
        return block(Code::CallerUnauthorized, None);
    }
    let audience = match resolve_audience(e, &intent.audience) {
        Ok(audience) => audience,
        Err(code) => return block(code, None),
    };
    if !permissions::can_flow(&closure.readers, &audience.recipients) {
        let witness = closure
            .acls
            .iter()
            .find(|(_, acl)| !permissions::can_flow(&acl.readers, &audience.recipients))
            .and_then(|(id, _)| closure.sources.get(id).map(|s| s.artifact.clone()));
        return block(Code::RestrictedSource, witness);
    }
    let artifact = e.artifacts.get(&intent.artifact).expect("validated root");
    let context = bind_context(e, intent, &closure, audience);
    // Revoking any approval for an exact context blocks every approval for that
    // same context issued at or before the revoked approval's issue time. Only
    // a strictly newer approval can re-authorize the context.
    let revoked_through = e
        .approvals
        .iter()
        .filter(|(id, a)| e.revoked.contains(*id) && a.context == context)
        .map(|(_, a)| a.issued_at)
        .max();
    let mut failure = None;
    let mut stale = false;
    for (id, approval) in &e.approvals {
        if approval.context != context {
            stale |= e
                .artifacts
                .get(&approval.context.artifact)
                .is_some_and(|a| a.logical_id == artifact.logical_id);
            continue;
        }
        let code = if approval.id != *id || id.0.is_empty() {
            Some(Code::InvalidSchema)
        } else if e.revoked.contains(id)
            || revoked_through.is_some_and(|cutoff| approval.issued_at <= cutoff)
        {
            Some(Code::ApprovalRevoked)
        } else if approval.issued_at > now
            || now >= approval.expires_at
            || approval.expires_at <= approval.issued_at
            || approval.expires_at - approval.issued_at > e.policy.max_approval_lifetime
        {
            Some(Code::ApprovalExpired)
        } else if !e.policy.approvers.contains(&approval.signer)
            || !e.principals.get(&approval.signer).is_some_and(|p| {
                p.id == approval.signer
                    && p.enabled
                    && p.role == Role::Human
                    && p.workspace == e.workspace
            })
            || !closure.readers.contains(&approval.signer)
        {
            Some(Code::SignerUnauthorized)
        } else {
            None
        };
        if let Some(code) = code {
            failure.get_or_insert(code);
        } else {
            // Approval authorizes the exact context, not the executor: only an
            // enabled, in-workspace human requester may perform the release.
            if actor.role != Role::Human {
                return block(Code::CallerUnauthorized, None);
            }
            return Decision::Ready {
                context,
                approval: id.clone(),
            };
        }
    }
    if let Some(code) = failure {
        return block(code, None);
    }
    if stale {
        return block(Code::StaleApproval, None);
    }
    Decision::PendingApproval { context }
}

/// Describe the exact publication binding for diagnostics, including when the
/// permission check fails. This is not authorization: only `verify` can return
/// Ready. Callers must separately authorize access to this metadata.
pub fn publication_binding(e: &Evidence, intent: &Intent) -> Result<Context, Code> {
    if !e.complete {
        return Err(Code::IncompleteLineage);
    }
    let closure = lineage::load(e, &intent.artifact)?;
    let audience = e
        .audiences
        .get(&intent.audience)
        .ok_or(Code::AudienceUnresolved)?;
    if e.policy.workspace != e.workspace || audience.workspace != e.workspace {
        return Err(Code::CrossWorkspace);
    }
    if audience.id != intent.audience || audience.revision == 0 || !audience.resolved {
        return Err(Code::AudienceUnresolved);
    }
    Ok(bind_context(e, intent, &closure, audience))
}

fn bind_context(
    e: &Evidence,
    intent: &Intent,
    closure: &lineage::Closure<'_>,
    audience: &Audience,
) -> Context {
    let artifact = e.artifacts.get(&intent.artifact).expect("validated root");
    Context {
        workspace: e.workspace.clone(),
        artifact: intent.artifact.clone(),
        artifact_hash: artifact.hash.clone(),
        audience: audience.clone(),
        action: "publish".into(),
        policy_hash: digest::structured("policy", &e.policy),
        lineage_hash: digest::structured(
            "lineage",
            &(
                &closure.artifacts,
                &closure.transformations,
                &closure.sources,
            ),
        ),
        acl_hash: digest::structured("source-acls", &closure.acls),
    }
}

/// Shared destination validation for publication and plan analysis. Missing or
/// unresolved memberships are never treated as empty sets.
pub(crate) fn resolve_audience<'a>(e: &'a Evidence, id: &AudienceId) -> Result<&'a Audience, Code> {
    let audience = e.audiences.get(id).ok_or(Code::AudienceUnresolved)?;
    if audience.workspace != e.workspace {
        return Err(Code::CrossWorkspace);
    }
    if audience.id != *id || audience.revision == 0 || audience.destination.is_empty() {
        return Err(Code::InvalidSchema);
    }
    if !audience.resolved
        || audience.recipients.is_empty()
        || audience.recipients.iter().any(|id| {
            !e.principals
                .get(id)
                .is_some_and(|p| p.id == *id && p.enabled && p.workspace == e.workspace)
        })
    {
        return Err(Code::AudienceUnresolved);
    }
    Ok(audience)
}
