use std::collections::{BTreeMap, BTreeSet};
use trustgraph_core::{digest, *};

fn readers(names: &[&str]) -> Readers {
    names.iter().map(|n| PrincipalId::from(*n)).collect()
}
fn fixture(private: bool) -> (Evidence, Intent) {
    let workspace = WorkspaceId::from("northstar");
    let mut e = Evidence {
        workspace: workspace.clone(),
        complete: true,
        principals: BTreeMap::new(),
        artifacts: BTreeMap::new(),
        sources: BTreeMap::new(),
        acls: BTreeMap::new(),
        transformations: BTreeMap::new(),
        audiences: BTreeMap::new(),
        policy: Policy {
            workspace: workspace.clone(),
            revision: 1,
            approvers: readers(&["maya"]),
            max_approval_lifetime: 100,
            configuration: None,
        },
        approvals: BTreeMap::new(),
        revoked: BTreeSet::new(),
    };
    for name in ["maya", "arun", "sam", "agent"] {
        let p = Principal {
            id: name.into(),
            workspace: workspace.clone(),
            role: if name == "agent" {
                Role::Agent
            } else {
                Role::Human
            },
            enabled: true,
        };
        e.principals.insert(p.id.clone(), p);
    }
    for (name, allowed) in [
        ("roadmap", readers(&["maya", "arun", "sam", "agent"])),
        ("secret", readers(&["maya", "arun", "agent"])),
    ] {
        let a = Artifact::new(
            name.into(),
            workspace.clone(),
            format!("{name} exact content\n"),
            Origin::Source(name.into()),
        );
        let acl = Acl {
            id: name.into(),
            workspace: workspace.clone(),
            revision: 1,
            readers: allowed,
        };
        e.sources.insert(
            name.into(),
            Source {
                id: name.into(),
                workspace: workspace.clone(),
                artifact: a.id.clone(),
                acl: acl.id.clone(),
            },
        );
        e.acls.insert(acl.id.clone(), acl);
        e.artifacts.insert(a.id.clone(), a);
    }
    let audience = Audience {
        id: "company".into(),
        workspace,
        destination: "sim://northstar/company".into(),
        revision: 1,
        recipients: readers(&["maya", "arun", "sam"]),
        resolved: true,
    };
    e.audiences.insert(audience.id.clone(), audience);
    derive(
        &mut e,
        "draft",
        if private {
            vec!["roadmap", "secret"]
        } else {
            vec!["roadmap"]
        },
    );
    (
        e,
        Intent {
            artifact: "draft".into(),
            audience: "company".into(),
            requester: "maya".into(),
        },
    )
}
fn derive(e: &mut Evidence, name: &str, inputs: Vec<&str>) {
    let manifest = Manifest {
        complete: true,
        provider: "fixture-v1".into(),
        template_revision: "launch-v1".into(),
        inputs: inputs
            .iter()
            .enumerate()
            .map(|(order, id)| InputUse {
                occurrence: format!("{name}-{order}"),
                artifact: (*id).into(),
                role: InputRole::Document,
                order,
            })
            .collect(),
    };
    let t = Transformation {
        id: name.into(),
        workspace: e.workspace.clone(),
        output: name.into(),
        manifest_hash: digest::structured("context-manifest", &manifest),
        manifest,
    };
    let a = Artifact::new(
        name.into(),
        e.workspace.clone(),
        "A launch announcement.".into(),
        Origin::Transformation(t.id.clone()),
    );
    e.transformations.insert(t.id.clone(), t);
    e.artifacts.insert(a.id.clone(), a);
}
fn approve(e: &mut Evidence, i: &Intent) -> Context {
    let context = verify(e, i, 10)
        .context()
        .expect("source-safe context")
        .clone();
    let a = Approval {
        id: "approval".into(),
        signer: "maya".into(),
        context: context.clone(),
        issued_at: 10,
        expires_at: 80,
    };
    e.approvals.insert(a.id.clone(), a);
    context
}

#[test]
fn broad_audience_is_blocked_by_transitive_private_source() {
    let (e, i) = fixture(true);
    let result = verify(&e, &i, 10);
    assert!(result.has(Code::RestrictedSource), "{result:?}");
    assert!(
        matches!(result, Decision::Blocked {violations} if violations[0].witness == Some("secret".into()))
    );
}
#[test]
fn safe_generation_waits_for_human_approval_then_becomes_ready() {
    let (mut e, i) = fixture(false);
    assert!(matches!(
        verify(&e, &i, 10),
        Decision::PendingApproval { .. }
    ));
    approve(&mut e, &i);
    assert!(matches!(verify(&e, &i, 11), Decision::Ready { .. }));
}
#[test]
fn editing_one_byte_creates_a_different_binding() {
    let (mut e, mut i) = fixture(false);
    let before = approve(&mut e, &i);
    derive(&mut e, "edited", vec!["draft"]);
    let a = e.artifacts.get_mut(&ArtifactId::from("edited")).unwrap();
    a.logical_id = "draft".into();
    a.version = 2;
    a.content.push('!');
    a.hash = digest::content(a.content.as_bytes());
    i.artifact = "edited".into();
    assert!(verify(&e, &i, 11).has(Code::StaleApproval));
    e.approvals.clear();
    let after = verify(&e, &i, 11).context().unwrap().clone();
    assert_ne!(before.digest(), after.digest());
}
#[test]
fn every_publication_binding_field_matters() {
    for change in 0..5 {
        let (mut e, i) = fixture(false);
        approve(&mut e, &i);
        match change {
            0 => e
                .audiences
                .get_mut(&i.audience)
                .unwrap()
                .destination
                .push_str("-other"),
            1 => e.audiences.get_mut(&i.audience).unwrap().revision += 1,
            2 => {
                e.audiences
                    .get_mut(&i.audience)
                    .unwrap()
                    .recipients
                    .remove(&PrincipalId::from("sam"));
            }
            3 => e.policy.revision += 1,
            _ => e.acls.get_mut(&AclId::from("roadmap")).unwrap().revision += 1,
        }
        assert!(
            verify(&e, &i, 11).has(Code::StaleApproval),
            "change {change}"
        );
    }
}
#[test]
fn human_approval_cannot_override_source_permissions() {
    let (mut e, i) = fixture(false);
    approve(&mut e, &i);
    e.acls
        .get_mut(&AclId::from("roadmap"))
        .unwrap()
        .readers
        .remove(&PrincipalId::from("sam"));
    assert!(verify(&e, &i, 11).has(Code::RestrictedSource));
}
#[test]
fn public_rewrite_with_private_previous_draft_stays_restricted() {
    let (mut e, mut i) = fixture(true);
    derive(&mut e, "rewrite", vec!["roadmap", "draft"]);
    i.artifact = "rewrite".into();
    assert!(verify(&e, &i, 11).has(Code::RestrictedSource));
    derive(&mut e, "clean", vec!["roadmap"]);
    i.artifact = "clean".into();
    assert!(matches!(
        verify(&e, &i, 11),
        Decision::PendingApproval { .. }
    ));
}
#[test]
fn revoked_expired_future_or_ineligible_approval_never_authorizes() {
    for change in 0..6 {
        let (mut e, i) = fixture(false);
        approve(&mut e, &i);
        let code = match change {
            0 => {
                e.revoked.insert("approval".into());
                Code::ApprovalRevoked
            }
            1 => {
                e.approvals
                    .get_mut(&ApprovalId::from("approval"))
                    .unwrap()
                    .expires_at = 11;
                Code::ApprovalExpired
            }
            2 => {
                e.principals
                    .get_mut(&PrincipalId::from("maya"))
                    .unwrap()
                    .role = Role::Agent;
                Code::SignerUnauthorized
            }
            3 => {
                e.policy.approvers.clear();
                Code::StaleApproval
            }
            4 => {
                e.approvals
                    .get_mut(&ApprovalId::from("approval"))
                    .unwrap()
                    .issued_at = 12;
                Code::ApprovalExpired
            }
            _ => {
                e.approvals
                    .get_mut(&ApprovalId::from("approval"))
                    .unwrap()
                    .expires_at = 1_000;
                Code::ApprovalExpired
            }
        };
        assert!(
            verify(&e, &i, 11).has(code),
            "change {change}: {:?}",
            verify(&e, &i, 11)
        );
    }
}
#[test]
fn missing_or_untrusted_provenance_blocks() {
    for change in 0..5 {
        let (mut e, i) = fixture(false);
        match change {
            0 => {
                e.artifacts.remove(&ArtifactId::from("roadmap"));
            }
            1 => e.complete = false,
            2 => {
                let t = e
                    .transformations
                    .get_mut(&TransformationId::from("draft"))
                    .unwrap();
                t.manifest.complete = false;
                t.manifest_hash = digest::structured("context-manifest", &t.manifest);
            }
            3 => {
                let t = e
                    .transformations
                    .get_mut(&TransformationId::from("draft"))
                    .unwrap();
                t.manifest.inputs.clear();
                t.manifest_hash = digest::structured("context-manifest", &t.manifest);
            }
            _ => {
                e.acls.remove(&AclId::from("roadmap"));
            }
        }
        assert!(
            verify(&e, &i, 10).has(Code::IncompleteLineage),
            "change {change}"
        );
    }
}
#[test]
fn schema_equations_cycles_and_tampered_bytes_fail_closed() {
    for change in 0..7 {
        let (mut e, i) = fixture(false);
        let code = match change {
            0 => {
                e.artifacts.get_mut(&i.artifact).unwrap().content.push('!');
                Code::ContentTampered
            }
            1 => {
                e.artifacts
                    .get_mut(&ArtifactId::from("roadmap"))
                    .unwrap()
                    .workspace = "elsewhere".into();
                Code::CrossWorkspace
            }
            2 => {
                e.transformations
                    .get_mut(&TransformationId::from("draft"))
                    .unwrap()
                    .output = "roadmap".into();
                Code::InvalidSchema
            }
            3 => {
                let t = e
                    .transformations
                    .get_mut(&TransformationId::from("draft"))
                    .unwrap();
                t.manifest.inputs[0].artifact = "draft".into();
                t.manifest_hash = digest::structured("context-manifest", &t.manifest);
                Code::InvalidSchema
            }
            4 => {
                e.sources
                    .get_mut(&SourceId::from("roadmap"))
                    .unwrap()
                    .artifact = "secret".into();
                Code::InvalidSchema
            }
            5 => {
                let t = e
                    .transformations
                    .get_mut(&TransformationId::from("draft"))
                    .unwrap();
                t.manifest.inputs[0].order = 2;
                t.manifest_hash = digest::structured("context-manifest", &t.manifest);
                Code::InvalidSchema
            }
            _ => {
                e.transformations
                    .get_mut(&TransformationId::from("draft"))
                    .unwrap()
                    .manifest
                    .provider
                    .push('!');
                Code::ContentTampered
            }
        };
        assert!(
            verify(&e, &i, 10).has(code),
            "change {change}: {:?}",
            verify(&e, &i, 10)
        );
    }
}
#[test]
fn unknown_audiences_and_cross_workspace_recipients_block() {
    for change in 0..3 {
        let (mut e, i) = fixture(false);
        let audience = e.audiences.get_mut(&i.audience).unwrap();
        let code = match change {
            0 => {
                audience.resolved = false;
                Code::AudienceUnresolved
            }
            1 => {
                audience.recipients.insert("internet".into());
                Code::AudienceUnresolved
            }
            _ => {
                audience.workspace = "foreign".into();
                Code::CrossWorkspace
            }
        };
        assert!(verify(&e, &i, 10).has(code));
    }
}
#[test]
fn unauthorized_actor_gets_no_restricted_metadata() {
    let (e, mut i) = fixture(true);
    i.requester = "sam".into();
    assert_eq!(
        verify(&e, &i, 10),
        Decision::Blocked {
            violations: vec![Violation {
                code: Code::CallerUnauthorized,
                witness: None
            }]
        }
    );
}
#[test]
fn repeated_input_occurrences_are_valid_but_duplicate_occurrence_identity_is_not() {
    let (mut e, i) = fixture(false);
    derive(&mut e, "draft", vec!["roadmap", "roadmap"]);
    assert!(matches!(
        verify(&e, &i, 10),
        Decision::PendingApproval { .. }
    ));
    let t = e
        .transformations
        .get_mut(&TransformationId::from("draft"))
        .unwrap();
    t.manifest.inputs[1].occurrence = t.manifest.inputs[0].occurrence.clone();
    t.manifest_hash = digest::structured("context-manifest", &t.manifest);
    assert!(verify(&e, &i, 10).has(Code::InvalidSchema));
}
#[test]
fn exact_bytes_and_structured_domains_have_distinct_hashes() {
    assert_eq!(
        digest::content(b"abc").0,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(digest::content(b"abc"), digest::content(b"abc\n"));
    assert_ne!(
        digest::structured("approval", &"x"),
        digest::structured("artifact", &"x")
    );
    assert_eq!(
        digest::structured("x", &serde_json::json!({"b":2,"a":1})),
        digest::structured("x", &serde_json::json!({"a":1,"b":2}))
    );
}

#[test]
fn traversal_limit_is_unknown_instead_of_assuming_no_restricted_ancestors() {
    let (mut e, mut i) = fixture(true);
    let mut previous = "draft".to_string();
    for index in 0..70 {
        let next = format!("layer-{index}");
        derive(&mut e, &next, vec![&previous]);
        previous = next;
    }
    i.artifact = ArtifactId(previous);
    assert!(verify(&e, &i, 10).has(Code::IncompleteLineage));
}

#[test]
fn shared_subtree_cache_cannot_hide_an_over_limit_path() {
    let (mut e, mut i) = fixture(false);
    let mut shared = "roadmap".to_string();
    for index in 0..40 {
        let next = format!("shared-{index}");
        derive(&mut e, &next, vec![&shared]);
        shared = next;
    }
    let mut deep = shared.clone();
    for index in 0..40 {
        let next = format!("deep-{index}");
        derive(&mut e, &next, vec![&deep]);
        deep = next;
    }
    derive(&mut e, "root", vec![&shared, &deep]);
    i.artifact = "root".into();
    assert!(verify(&e, &i, 10).has(Code::IncompleteLineage));
}

proptest::proptest! {
    #[test]
    fn readership_meet_is_associative_commutative_idempotent_and_never_widens(a in 0_u8..16, b in 0_u8..16, c in 0_u8..16) {
        let names = ["maya","arun","sam","agent"];
        let set = |mask:u8| names.iter().enumerate().filter(|(index,_)| mask & (1 << index) != 0).map(|(_,name)| PrincipalId::from(*name)).collect::<Readers>();
        let (mut e, _) = fixture(false);
        e.acls.get_mut(&AclId::from("roadmap")).unwrap().readers = set(a);
        e.acls.get_mut(&AclId::from("secret")).unwrap().readers = set(b);
        let mut third = e.artifacts[&ArtifactId::from("roadmap")].clone();
        third.id = "third".into(); third.logical_id = "third".into(); third.origin = Origin::Source("third".into());
        e.artifacts.insert(third.id.clone(),third);
        e.sources.insert("third".into(),Source {id:"third".into(),workspace:e.workspace.clone(),artifact:"third".into(),acl:"third".into()});
        e.acls.insert("third".into(),Acl {id:"third".into(),workspace:e.workspace.clone(),revision:1,readers:set(c)});
        for (name,inputs) in [("ab",vec!["roadmap","secret"]),("ba",vec!["secret","roadmap"]),("aa",vec!["roadmap","roadmap"]),("bc",vec!["secret","third"]),("left",vec!["ab","third"]),("right",vec!["roadmap","bc"])] {derive(&mut e,name,inputs);}
        let read = |id:&str| lineage::derived_readers(&e,&id.into()).unwrap();
        proptest::prop_assert_eq!(read("ab"),read("ba"));
        proptest::prop_assert_eq!(read("aa"),set(a));
        proptest::prop_assert_eq!(read("left"),read("right"));
        proptest::prop_assert_eq!(read("left"),set(a & b & c));
        proptest::prop_assert!(read("ab").is_subset(&set(a)));
    }
}
