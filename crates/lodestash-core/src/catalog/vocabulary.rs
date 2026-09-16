use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VocabularyChange {
    Trait {
        namespace: String,
        value: String,
        scope: Option<String>,
        usage_note: Option<String>,
        #[serde(default)]
        teach: bool,
    },
    Category {
        taxonomy_id: i64,
        parent_id: Option<i64>,
        label: String,
        reason: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VocabularyProposal {
    pub request_id: String,
    pub expected_revision: String,
    pub changes: Vec<VocabularyChange>,
}

fn revision_in(conn: &Connection, policy: &str) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(policy.as_bytes());
    for sql in [
            "SELECT json_array(id,norm_name,namespace,scope,usage_note,teach) FROM tag WHERE is_trait=1 ORDER BY id",
            "SELECT json_array(id,name,version,is_active) FROM taxonomy ORDER BY id",
            "SELECT json_array(id,taxonomy_id,parent_id,pref_label,path) FROM category ORDER BY id",
            "SELECT json_array(id,category_id,alias) FROM category_alias ORDER BY id",
            "SELECT json_array(id,taxonomy_id,category_id,match_type,pattern,priority,enabled) FROM mapping_rule ORDER BY id",
        ] {
            let mut stmt = conn.prepare(sql).map_err(|e| CoreError::Catalog(e.to_string()))?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| CoreError::Catalog(e.to_string()))?;
            hash.update(sql.as_bytes());
            for row in rows {
                hash.update(row.map_err(|e| CoreError::Catalog(e.to_string()))?.as_bytes());
                hash.update(b"\n");
            }
        }
    Ok(format!("v2-{}", &hash.finalize().to_hex()[..32]))
}

pub(super) fn validate_assignments_in<'a>(
    conn: &Connection,
    revision: &str,
    policy: &str,
    tags: impl Iterator<Item = (i64, &'a str, &'a str)>,
) -> Result<()> {
    if revision != revision_in(conn, policy)? {
        return Err(CoreError::Catalog(
            "vocabulary_conflict: vocabulary changed".into(),
        ));
    }
    if !matches!(policy, "open" | "controlled" | "extend_with_approval") {
        return Err(CoreError::Catalog(
            "invalid_argument: unknown vocabulary policy".into(),
        ));
    }
    for (asset, namespace, value) in tags {
        let key = normalize_tag(namespace, value)
            .ok_or_else(|| CoreError::Catalog("invalid_argument: empty tag".into()))?
            .0;
        let applicable: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM tag t JOIN asset a ON a.id=?1 JOIN media_type mt ON mt.id=a.media_type_id WHERE t.norm_name=?2 AND t.is_trait=1 AND (t.scope IS NULL OR t.scope=mt.code))", params![asset,key], |r| r.get(0)).map_err(|e| CoreError::Catalog(e.to_string()))?;
        let scoped_elsewhere: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM tag WHERE norm_name=?1 AND is_trait=1 AND scope IS NOT NULL)", [&key], |r| r.get(0)).map_err(|e| CoreError::Catalog(e.to_string()))?;
        if !applicable && (policy != "open" || scoped_elsewhere) {
            return Err(CoreError::Catalog(format!(
                "vocabulary_term_unknown: {key} is not applicable to asset {asset}"
            )));
        }
    }
    Ok(())
}

impl Catalog {
    pub fn validate_vocabulary_assignments<'a>(
        &self,
        revision: &str,
        policy: &str,
        tags: impl Iterator<Item = (i64, &'a str, &'a str)>,
    ) -> Result<()> {
        validate_assignments_in(&self.conn, revision, policy, tags)
    }
    /// A filter-independent revision: paging and modality selection cannot change the write token.
    pub fn vocabulary_revision(&self, policy: &str) -> Result<String> {
        revision_in(&self.conn, policy)
    }

    pub fn validate_vocabulary_proposal(
        &self,
        proposal: &VocabularyProposal,
        policy: &str,
    ) -> Result<()> {
        let invalid = |message: &str| CoreError::Catalog(format!("invalid_argument: {message}"));
        if policy == "controlled" {
            return Err(CoreError::Catalog(
                "vocabulary_policy_denied: controlled vocabulary cannot be extended by an agent"
                    .into(),
            ));
        }
        if !matches!(policy, "open" | "extend_with_approval") {
            return Err(invalid("unknown vocabulary policy"));
        }
        if proposal.changes.is_empty() || proposal.changes.len() > MAX_BATCH_ASSETS {
            return Err(invalid("a proposal holds 1 to 100 changes"));
        }
        if proposal.expected_revision != self.vocabulary_revision(policy)? {
            return Err(CoreError::Catalog(
                "vocabulary_conflict: read the vocabulary again".into(),
            ));
        }
        for change in &proposal.changes {
            match change {
                VocabularyChange::Trait {
                    namespace,
                    value,
                    scope,
                    usage_note,
                    teach,
                } => {
                    if let Some(problem) =
                        assignment_problem("ai:proposal", &TagAssignment::exact(namespace, value))
                    {
                        return Err(invalid(&problem));
                    }
                    if let Some(scope) = scope {
                        if self.media_type_id(scope)?.is_none() {
                            return Err(invalid("unknown trait scope"));
                        }
                    }
                    if usage_note
                        .as_ref()
                        .is_some_and(|note| note.chars().count() > MAX_DESCRIPTION_CHARS)
                    {
                        return Err(invalid("usage note is too long"));
                    }
                    let key = normalize_tag(namespace, value).unwrap().0;
                    for existing in self.list_vocabulary()?.into_iter().filter(|t| t.is_trait) {
                        if normalize_tag(&existing.namespace, &existing.value)
                            .unwrap()
                            .0
                            == key
                            && (existing.scope != *scope
                                || existing.usage_note != *usage_note
                                || existing.teach != *teach)
                        {
                            return Err(invalid("an existing trait's scope, guidance and teaching flag cannot be overwritten"));
                        }
                    }
                }
                VocabularyChange::Category {
                    taxonomy_id,
                    parent_id,
                    label,
                    reason,
                } => {
                    if self.taxonomy_meta(*taxonomy_id)?.is_none() {
                        return Err(invalid("taxonomy does not exist"));
                    }
                    if label.trim().is_empty()
                        || label.chars().count() > MAX_LABEL_CHARS
                        || label.contains(['/', '\\'])
                        || label.chars().any(char::is_control)
                    {
                        return Err(invalid("invalid category label"));
                    }
                    if reason.trim().is_empty() || reason.chars().count() > MAX_DESCRIPTION_CHARS {
                        return Err(invalid("a bounded category reason is required"));
                    }
                    if let Some(parent) = parent_id {
                        if !self
                            .list_categories(*taxonomy_id)?
                            .iter()
                            .any(|(id, _, _)| id == parent)
                        {
                            return Err(invalid("parent is not in the requested taxonomy"));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Existing helpers use this connection; the outer transaction covers all of them and receipt.
    pub fn apply_vocabulary_proposal(
        &self,
        proposal: &VocabularyProposal,
        policy: &str,
        digest: &str,
    ) -> Result<serde_json::Value> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| CoreError::Catalog(e.to_string()))?;
        if let Some(receipt) = self.tag_request_receipt(&proposal.request_id)? {
            return if receipt.digest == digest {
                Ok(receipt.result)
            } else {
                Err(CoreError::Catalog(
                    "request_conflict: request body changed".into(),
                ))
            };
        }
        self.validate_vocabulary_proposal(proposal, policy)?;
        let mut results = Vec::new();
        for change in &proposal.changes {
            match change {
                VocabularyChange::Trait {
                    namespace,
                    value,
                    scope,
                    usage_note,
                    teach,
                } => {
                    // Check each step too: two incompatible changes inside one proposal cannot overwrite.
                    let key = normalize_tag(namespace, value).unwrap().0;
                    let existing = self.list_vocabulary()?.into_iter().find(|t| {
                        normalize_tag(&t.namespace, &t.value).is_some_and(|(k, _)| k == key)
                    });
                    if let Some(t) = existing.as_ref().filter(|t| t.is_trait) {
                        if t.scope != *scope || t.usage_note != *usage_note || t.teach != *teach {
                            return Err(CoreError::Catalog(
                                "invalid_argument: conflicting trait proposals".into(),
                            ));
                        }
                    }
                    let reused = existing.as_ref().is_some_and(|t| t.is_trait);
                    let id = if reused {
                        existing.unwrap().id
                    } else {
                        self.upsert_trait(
                            &namespace.trim().to_lowercase(),
                            value,
                            scope.as_deref(),
                            usage_note.as_deref(),
                            *teach,
                        )?
                    };
                    results.push(serde_json::json!({"kind":"trait","id":id,"reused":reused}));
                }
                VocabularyChange::Category {
                    taxonomy_id,
                    parent_id,
                    label,
                    ..
                } => {
                    let rows = self.list_categories(*taxonomy_id)?;
                    let parent_path = parent_id.and_then(|parent| {
                        rows.iter()
                            .find(|(id, _, _)| *id == parent)
                            .map(|(_, _, path)| path.as_str())
                    });
                    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
                    let path = parent_path
                        .map_or_else(|| label.clone(), |parent| format!("{parent}/{label}"));
                    let existing = rows
                        .iter()
                        .find(|(_, _, candidate)| candidate.to_lowercase() == path.to_lowercase());
                    let id = match existing {
                        Some((id, _, _)) => *id,
                        None => self.add_category(*taxonomy_id, *parent_id, &label, &path)?,
                    };
                    results.push(
                        serde_json::json!({"kind":"category","id":id,"reused":existing.is_some()}),
                    );
                }
            }
        }
        let result =
            serde_json::json!({"changes":results,"revision":self.vocabulary_revision(policy)?});
        self.conn.execute("INSERT INTO tag_request(request_id,body_digest,op_id,result_json,created_at) VALUES (?1,?2,NULL,?3,?4)", params![proposal.request_id,digest,result.to_string(),unix_now()]).map_err(|e| CoreError::Catalog(e.to_string()))?;
        tx.commit().map_err(|e| CoreError::Catalog(e.to_string()))?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposals_commit_together_replay_and_preserve_existing_guidance() {
        let c = Catalog::open_in_memory().unwrap();
        c.seed_media_types().unwrap();
        let taxonomy = c.create_taxonomy("Game", true).unwrap();
        let proposal = VocabularyProposal {
            request_id: "test-proposal".into(),
            expected_revision: c.vocabulary_revision("extend_with_approval").unwrap(),
            changes: vec![
                VocabularyChange::Trait {
                    namespace: "subject".into(),
                    value: "Arch".into(),
                    scope: Some("model_3d".into()),
                    usage_note: Some("Curved doorway".into()),
                    teach: false,
                },
                VocabularyChange::Category {
                    taxonomy_id: taxonomy,
                    parent_id: None,
                    label: "Architecture".into(),
                    reason: "Group structures".into(),
                },
            ],
        };
        let result = c
            .apply_vocabulary_proposal(&proposal, "extend_with_approval", "digest")
            .unwrap();
        assert_eq!(result["changes"].as_array().unwrap().len(), 2);
        assert_ne!(
            c.vocabulary_revision("extend_with_approval").unwrap(),
            proposal.expected_revision
        );
        assert_eq!(
            c.apply_vocabulary_proposal(&proposal, "extend_with_approval", "digest")
                .unwrap(),
            result
        );
        let mut bad = proposal.clone();
        bad.request_id = "bad".into();
        bad.expected_revision = c.vocabulary_revision("extend_with_approval").unwrap();
        bad.changes[0] = VocabularyChange::Trait {
            namespace: "subject".into(),
            value: "New term".into(),
            scope: None,
            usage_note: None,
            teach: false,
        };
        bad.changes[1] = VocabularyChange::Category {
            taxonomy_id: taxonomy,
            parent_id: Some(999),
            label: "Broken".into(),
            reason: "Bad parent".into(),
        };
        assert!(c
            .apply_vocabulary_proposal(&bad, "extend_with_approval", "bad")
            .is_err());
        assert!(!c
            .list_vocabulary()
            .unwrap()
            .iter()
            .any(|t| t.value == "New term"));
        assert!(c.tag_request_receipt("bad").unwrap().is_none());
        bad.changes = vec![VocabularyChange::Trait {
            namespace: "subject".into(),
            value: "arch".into(),
            scope: None,
            usage_note: None,
            teach: true,
        }];
        assert!(c
            .apply_vocabulary_proposal(&bad, "extend_with_approval", "changed")
            .is_err());
        let traits = c.list_vocabulary().unwrap();
        assert_eq!(traits[0].usage_note.as_deref(), Some("Curved doorway"));
        assert!(!traits[0].teach);
    }
    #[test]
    fn conflicting_changes_roll_back_prior_steps_and_receipt() {
        let c = Catalog::open_in_memory().unwrap();
        c.seed_media_types().unwrap();
        let before = c.vocabulary_revision("extend_with_approval").unwrap();
        let p = VocabularyProposal {
            request_id: "rollback".into(),
            expected_revision: before.clone(),
            changes: vec![
                VocabularyChange::Trait {
                    namespace: "subject".into(),
                    value: "tower".into(),
                    scope: None,
                    usage_note: None,
                    teach: false,
                },
                VocabularyChange::Trait {
                    namespace: "subject".into(),
                    value: "TOWER".into(),
                    scope: None,
                    usage_note: None,
                    teach: true,
                },
            ],
        };
        assert!(c
            .apply_vocabulary_proposal(&p, "extend_with_approval", "digest")
            .is_err());
        assert_eq!(
            c.vocabulary_revision("extend_with_approval").unwrap(),
            before
        );
        assert!(c.tag_request_receipt("rollback").unwrap().is_none());
    }
}
