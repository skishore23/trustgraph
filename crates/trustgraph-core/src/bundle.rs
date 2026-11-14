use crate::{
    digest::{self, Digest},
    *,
};
use serde::{Deserialize, Serialize};

/// Portable historical evidence. Its digest is an integrity check, not a signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub schema: u32,
    pub evidence: Evidence,
    pub intent: Intent,
    pub at: u64,
    pub decision: Decision,
    pub digest: Digest,
}

/// Constructible only by evaluating a complete bundle through the verifier.
#[derive(Debug, Clone)]
pub struct VerifiedBundle {
    bundle: Bundle,
}

impl VerifiedBundle {
    pub fn check(evidence: &Evidence, intent: &Intent, at: u64) -> Result<Self, Box<Decision>> {
        let decision = verify(evidence, intent, at);
        if !matches!(decision, Decision::Ready { .. }) {
            return Err(Box::new(decision));
        }
        let closure = crate::lineage::load(evidence, &intent.artifact).expect("ready lineage");
        let mut snapshot = evidence.clone();
        snapshot
            .artifacts
            .retain(|id, _| closure.artifacts.contains_key(id));
        snapshot
            .transformations
            .retain(|id, _| closure.transformations.contains_key(id));
        snapshot
            .sources
            .retain(|id, _| closure.sources.contains_key(id));
        snapshot
            .acls
            .retain(|id, _| closure.acls.values().any(|acl| acl.id == *id));
        snapshot.audiences.retain(|id, _| *id == intent.audience);
        let Decision::Ready { approval, .. } = &decision else {
            unreachable!()
        };
        snapshot.approvals.retain(|id, _| id == approval);
        snapshot.revoked.retain(|id| id == approval);
        let mut bundle = Bundle {
            schema: 1,
            evidence: snapshot,
            intent: intent.clone(),
            at,
            decision,
            digest: Digest(String::new()),
        };
        bundle.digest = seal(&bundle);
        Ok(Self { bundle })
    }
    pub fn export(&self) -> &Bundle {
        &self.bundle
    }
    pub fn context(&self) -> &Context {
        self.bundle.decision.context().expect("ready bundle")
    }
    pub fn approval(&self) -> &ApprovalId {
        match &self.bundle.decision {
            Decision::Ready { approval, .. } => approval,
            _ => unreachable!("ready bundle"),
        }
    }
}

fn seal(bundle: &Bundle) -> Digest {
    digest::structured(
        "verification-bundle",
        &(
            bundle.schema,
            &bundle.evidence,
            &bundle.intent,
            bundle.at,
            &bundle.decision,
        ),
    )
}

/// Re-evaluate against recorded time and authority, never against current state.
pub fn replay(bundle: &Bundle) -> Result<VerifiedBundle, &'static str> {
    if bundle.schema != 1 || seal(bundle) != bundle.digest {
        return Err("bundle integrity mismatch");
    }
    let checked = VerifiedBundle::check(&bundle.evidence, &bundle.intent, bundle.at)
        .map_err(|_| "recorded publication was not authorized")?;
    if checked.bundle != *bundle {
        return Err("recorded decision does not follow from evidence");
    }
    Ok(checked)
}
