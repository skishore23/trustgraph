use crate::{digest, *};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ARTIFACTS: usize = 256;
const MAX_DEPTH: usize = 64;

pub(crate) struct Closure<'a> {
    pub artifacts: BTreeMap<ArtifactId, &'a Artifact>,
    pub transformations: BTreeMap<TransformationId, &'a Transformation>,
    pub sources: BTreeMap<SourceId, &'a Source>,
    pub acls: BTreeMap<SourceId, &'a Acl>,
    pub readers: Readers,
}

pub(crate) fn load<'a>(e: &'a Evidence, root: &ArtifactId) -> Result<Closure<'a>, Code> {
    let mut closure = Closure {
        artifacts: BTreeMap::new(),
        transformations: BTreeMap::new(),
        sources: BTreeMap::new(),
        acls: BTreeMap::new(),
        readers: Readers::new(),
    };
    let mut visiting = BTreeSet::new();
    let mut memo = BTreeMap::new();
    closure.readers = walk(e, root, 0, &mut visiting, &mut memo, &mut closure)?.0;
    Ok(closure)
}

fn walk<'a>(
    e: &'a Evidence,
    id: &ArtifactId,
    depth: usize,
    visiting: &mut BTreeSet<ArtifactId>,
    memo: &mut BTreeMap<ArtifactId, (Readers, usize)>,
    closure: &mut Closure<'a>,
) -> Result<(Readers, usize), Code> {
    if depth > MAX_DEPTH {
        return Err(Code::IncompleteLineage);
    }
    if let Some((readers, height)) = memo.get(id) {
        if depth + height > MAX_DEPTH {
            return Err(Code::IncompleteLineage);
        }
        return Ok((readers.clone(), *height));
    }
    if !visiting.insert(id.clone()) {
        return Err(Code::InvalidSchema);
    }
    if closure.artifacts.len() >= MAX_ARTIFACTS {
        return Err(Code::IncompleteLineage);
    }
    let artifact = e.artifacts.get(id).ok_or(Code::IncompleteLineage)?;
    if &artifact.id != id
        || id.0.is_empty()
        || artifact.logical_id.is_empty()
        || artifact.version == 0
    {
        return Err(Code::InvalidSchema);
    }
    if artifact.workspace != e.workspace {
        return Err(Code::CrossWorkspace);
    }
    if artifact.hash != digest::content(artifact.content.as_bytes()) {
        return Err(Code::ContentTampered);
    }
    closure.artifacts.insert(id.clone(), artifact);
    let (readers, height) = match &artifact.origin {
        Origin::Source(source_id) => {
            let source = e.sources.get(source_id).ok_or(Code::IncompleteLineage)?;
            if source.id != *source_id || source.artifact != *id {
                return Err(Code::InvalidSchema);
            }
            if source.workspace != e.workspace {
                return Err(Code::CrossWorkspace);
            }
            let acl = e.acls.get(&source.acl).ok_or(Code::IncompleteLineage)?;
            if acl.workspace != e.workspace {
                return Err(Code::CrossWorkspace);
            }
            if acl.id != source.acl || acl.revision == 0 {
                return Err(Code::InvalidSchema);
            }
            for reader in &acl.readers {
                let p = e.principals.get(reader).ok_or(Code::IncompleteLineage)?;
                if p.id != *reader || p.workspace != e.workspace {
                    return Err(Code::CrossWorkspace);
                }
            }
            closure.sources.insert(source_id.clone(), source);
            closure.acls.insert(source_id.clone(), acl);
            (acl.readers.clone(), 0)
        }
        Origin::Transformation(transformation_id) => {
            let t = e
                .transformations
                .get(transformation_id)
                .ok_or(Code::IncompleteLineage)?;
            if t.id != *transformation_id
                || t.output != *id
                || e.transformations
                    .values()
                    .filter(|p| p.output == *id)
                    .count()
                    != 1
            {
                return Err(Code::InvalidSchema);
            }
            if t.workspace != e.workspace {
                return Err(Code::CrossWorkspace);
            }
            if digest::structured("context-manifest", &t.manifest) != t.manifest_hash {
                return Err(Code::ContentTampered);
            }
            if !t.manifest.complete
                || t.manifest.inputs.is_empty()
                || t.manifest.provider.is_empty()
                || t.manifest.template_revision.is_empty()
            {
                return Err(Code::IncompleteLineage);
            }
            closure.transformations.insert(t.id.clone(), t);
            let mut occurrences = BTreeSet::new();
            let mut allowed: Option<Readers> = None;
            let mut height = 0;
            for (order, input) in t.manifest.inputs.iter().enumerate() {
                if input.order != order
                    || input.occurrence.is_empty()
                    || !occurrences.insert(&input.occurrence)
                {
                    return Err(Code::InvalidSchema);
                }
                let (incoming, child_height) =
                    walk(e, &input.artifact, depth + 1, visiting, memo, closure)?;
                height = height.max(child_height + 1);
                allowed = Some(match allowed {
                    None => incoming,
                    Some(existing) => existing.intersection(&incoming).cloned().collect(),
                });
            }
            (allowed.ok_or(Code::IncompleteLineage)?, height)
        }
    };
    visiting.remove(id);
    memo.insert(id.clone(), (readers.clone(), height));
    Ok((readers, height))
}

/// Read authorization uses the same complete dependency closure as publication.
pub fn can_read(e: &Evidence, actor: &PrincipalId, artifact: &ArtifactId) -> bool {
    e.complete
        && e.principals
            .get(actor)
            .is_some_and(|p| p.enabled && p.id == *actor && p.workspace == e.workspace)
        && load(e, artifact).is_ok_and(|closure| closure.readers.contains(actor))
}

pub fn derived_readers(e: &Evidence, artifact: &ArtifactId) -> Result<Readers, Code> {
    if !e.complete {
        return Err(Code::IncompleteLineage);
    }
    Ok(load(e, artifact)?.readers)
}
