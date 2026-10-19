//! One-way, recoverable retirement of the Registry. Original records remain
//! in the database and an atomic JSON backup; only kept cards become notes.
use crate::{
    db::Db,
    models::{Note, Notebook, RegistryCard},
};
use anyhow::{Context, Result};
use std::{collections::BTreeMap, io::Write, path::Path};

const DONE: &str = "registry-notes-v1";
pub(crate) fn note_id(card: &str) -> String {
    format!("registry-{card}")
}

fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('\n', " ")
}

fn content(
    card: &RegistryCard,
    notebooks: &[Notebook],
    titles: &BTreeMap<String, String>,
) -> String {
    let mut out = format!("# {}\n\n{}\n\n", card.name, card.note);
    if !card.identifiers.is_empty() {
        out.push_str(&format!("**Identifiers:** {}\n\n", card.identifiers));
    }
    for fact in &card.facts {
        out.push_str(&format!("- **{}:** {}\n", fact.label, fact.value));
    }
    out.push_str("\n## Related documents\n\n");
    for a in card.attachments.iter().filter(|a| a.status != "rejected") {
        let nb = notebooks
            .iter()
            .find(|n| n.id == a.notebook_id)
            .map(|n| n.title.as_str())
            .unwrap_or("Original notebook");
        out.push_str(&format!(
            "- [{}](alchemy://source/{}) — {} ({}, matched: {})\n",
            escape_label(
                titles
                    .get(&a.source_id)
                    .map(String::as_str)
                    .unwrap_or(&a.source_id)
            ),
            a.source_id,
            nb,
            a.status,
            a.matched
        ));
    }
    out.push_str(&format!("\n---\nPreserved from Registry ({}, record {}). Facts retain their original attribution; a document link does not establish which fact it supports. Original records and attachment decisions are in registry-backup.json beside the database.\n", card.kind, card.id));
    out
}

/// Atomic create-if-absent: a crash cannot leave a partial backup mistaken
/// for a completed export, and a retry never overwrites the original.
fn backup(path: &Path, cards: &[RegistryCard]) -> Result<Vec<RegistryCard>> {
    if path.exists() {
        return Ok(serde_json::from_slice(&std::fs::read(path)?)?);
    }
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(cards)?)?;
    f.sync_all()?;
    match std::fs::hard_link(&tmp, path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
    }
    std::fs::remove_file(tmp)?;
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

pub(crate) async fn migrate(db: &Db, dir: &Path) -> Result<()> {
    if db.kv_get(DONE).await?.is_some() {
        return Ok(());
    }
    let cards = db.list_registry().await?;
    if cards.is_empty() {
        return Ok(());
    }
    let cards = backup(&dir.join("registry-backup.json"), &cards)
        .context("Registry backup failed; original data retained")?;
    let mut notebooks = db.list_notebooks().await?;
    let mut titles = BTreeMap::new();
    let mut origins = BTreeMap::new();
    for card in &cards {
        for a in &card.attachments {
            if let Some(source) = db.get_source(&a.source_id).await? {
                titles.insert(a.source_id.clone(), source.title);
                origins.insert(a.source_id.clone(), source.url);
            }
        }
    }
    for card in cards.iter().filter(|c| c.origin.is_empty()) {
        let id = note_id(&card.id);
        // Preserve later edits, and safely resume a partially completed migration.
        if db.get_note(&id).await?.is_some() {
            continue;
        }
        let mut owners: BTreeMap<String, usize> = BTreeMap::new();
        for a in card.attachments.iter().filter(|a| a.status != "rejected") {
            if notebooks
                .iter()
                .any(|n| n.id == a.notebook_id && n.status.is_empty())
            {
                *owners.entry(a.notebook_id.clone()).or_default() += 1;
            }
        }
        let owner = owners
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(id, _)| id)
            .unwrap_or_else(|| "registry-preserved-notes".into());
        if !notebooks.iter().any(|n| n.id == owner) {
            let nb = Notebook {
                id: owner.clone(),
                title: "Saved knowledge".into(),
                created_at: card.created_at,
                updated_at: card.updated_at,
                color: String::new(),
                icon: String::new(),
                cover: String::new(),
                status: String::new(),
                growth_web: false,
                source_count: 0,
                note_count: 0,
                report_count: 0,
                built_in: false,
            };
            db.create_notebook(&nb).await?;
            notebooks.push(nb);
        }
        let note = Note {
            id,
            notebook_id: owner,
            title: card.name.clone(),
            content: {
                let mut text = content(card, &notebooks, &titles);
                for a in card.attachments.iter().filter(|a| a.status != "rejected") {
                    if let Some(url) = origins.get(&a.source_id).filter(|u| !u.is_empty()) {
                        text.push_str(&format!("\nOriginal resource ({}): {}\n", a.source_id, url));
                    }
                }
                text
            },
            kind: "note".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: card.created_at,
            updated_at: card.updated_at,
        };
        db.add_note(&note).await?;
        db.touch_notebook(&note.notebook_id, chrono::Utc::now().timestamp_millis())
            .await?;
        crate::note_index::mark_pending(db, &note.id).await?;
    }
    // Completion prevents a later deliberate note deletion from resurrecting it.
    db.kv_set(DONE, "complete").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CardAttachment, CardFact};
    #[tokio::test]
    async fn migration_is_recoverable_preserves_edits_and_does_not_promote_suggestions() {
        let dir = std::env::temp_dir().join(format!("alchemy-retire-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.unwrap();
        let mut card = RegistryCard {
            id: "kept".into(),
            kind: "policy".into(),
            name: "House policy".into(),
            identifiers: "POL-123456".into(),
            note: "My annotation".into(),
            facts: vec![CardFact {
                label: "Limit".into(),
                value: "$500".into(),
            }],
            attachments: vec![],
            origin: String::new(),
            mentions: 0,
            surfaced: false,
            triage: String::new(),
            created_at: 1,
            updated_at: 2,
        };
        db.add_registry_card(&card).await.unwrap();
        card.id = "suggested".into();
        card.origin = "auto".into();
        db.add_registry_card(&card).await.unwrap();
        migrate(&db, &dir).await.unwrap();
        let note = db.get_note(&note_id("kept")).await.unwrap().unwrap();
        assert!(note.content.contains("POL-123456") && note.content.contains("$500"));
        assert!(db.get_note(&note_id("suggested")).await.unwrap().is_none());
        let original: Vec<RegistryCard> =
            serde_json::from_slice(&std::fs::read(dir.join("registry-backup.json")).unwrap())
                .unwrap();
        assert_eq!(original.len(), 2);
        db.update_note(&note.id, "Edited", "My edited content", 3)
            .await
            .unwrap();
        migrate(&db, &dir).await.unwrap();
        assert_eq!(
            db.get_note(&note.id).await.unwrap().unwrap().content,
            "My edited content"
        );
        db.delete_note(&note.id).await.unwrap();
        migrate(&db, &dir).await.unwrap();
        assert!(db.get_note(&note.id).await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[tokio::test]
    async fn backup_failure_prevents_partial_migration_and_retry_recovers() {
        let dir =
            std::env::temp_dir().join(format!("alchemy-retire-failure-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).await.unwrap();
        let card = RegistryCard {
            id: "retry".into(),
            kind: "policy".into(),
            name: "Policy".into(),
            identifiers: "REF-123456".into(),
            note: String::new(),
            facts: vec![],
            attachments: vec![],
            origin: String::new(),
            triage: String::new(),
            mentions: 0,
            surfaced: false,
            created_at: 1,
            updated_at: 1,
        };
        db.add_registry_card(&card).await.unwrap();
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        assert!(migrate(&db, &blocked).await.is_err());
        assert!(db.get_note(&note_id("retry")).await.unwrap().is_none());
        assert!(db.kv_get(DONE).await.unwrap().is_none());
        migrate(&db, &dir).await.unwrap();
        assert!(db.get_note(&note_id("retry")).await.unwrap().is_some());
        assert!(db.get_registry_card("retry").await.unwrap().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn related_links_keep_notebook_and_attachment_provenance() {
        let card = RegistryCard {
            id: "a".into(),
            kind: "asset".into(),
            name: "Thing".into(),
            identifiers: String::new(),
            note: String::new(),
            facts: vec![],
            attachments: vec![CardAttachment {
                source_id: "s".into(),
                notebook_id: "nb".into(),
                status: "confirmed".into(),
                matched: "manual".into(),
                at: 1,
            }],
            origin: String::new(),
            mentions: 0,
            surfaced: false,
            triage: String::new(),
            created_at: 1,
            updated_at: 1,
        };
        let text = content(
            &card,
            &[],
            &BTreeMap::from([("s".into(), "Doc [1]".into())]),
        );
        assert!(text.contains("alchemy://source/s") && text.contains("matched: manual"));
        assert!(text.contains("Doc \\[1\\]"));
    }
    /// Creates a legacy corpus for native UI QA, never the user's library.
    #[tokio::test]
    #[ignore = "manual native QA fixture; requires ALCHEMY_QA_DATA_DIR"]
    async fn seed_native_retirement_qa() {
        let root = std::path::PathBuf::from(
            std::env::var("ALCHEMY_QA_DATA_DIR").expect("QA data directory required"),
        );
        assert!(
            root.is_absolute() && root.file_name().unwrap() == "com.thrashr888.alchemy.paul26qa"
        );
        let dir = root.join("lancedb");
        assert!(!dir.exists(), "Refuse to overwrite a fixture store");
        let db = Db::open(&dir).await.unwrap();
        for (id, title) in [("qa-house", "Household"), ("qa-finance", "Finance")] {
            db.create_notebook(&Notebook {
                id: id.into(),
                title: title.into(),
                created_at: 1,
                updated_at: 1,
                color: String::new(),
                icon: String::new(),
                cover: String::new(),
                status: String::new(),
                growth_web: false,
                source_count: 0,
                note_count: 0,
                report_count: 0,
                built_in: false,
            })
            .await
            .unwrap();
        }
        for (id, nb, title, typ, parent, text) in [
            ("qa-policy-source", "qa-house", "Insurance coverage", "text", "", "The house policy covers flood damage up to fifteen thousand dollars."),
            ("qa-notion", "qa-finance", "Imported Notion snapshot", "notion", "", "Previously imported page tree."),
            ("qa-notion-child", "qa-finance", "Renewal details", "markdown", "qa-notion", "The policy renews November 15. This content was imported before the connector was retired.")
        ] {
            let source = crate::models::Source { id: id.into(), notebook_id: nb.into(), title: title.into(), source_type: typ.into(), parent_id: parent.into(), content: text.into(), char_count: text.len() as i64, url: if typ == "notion" { "https://notion.so/qa-retired-snapshot".into() } else { String::new() }, status: "ready".into(), ..Default::default() };
            db.insert_source(&source, &[], &[]).await.unwrap();
        }
        let card = RegistryCard {
            id: "qa-policy".into(),
            kind: "policy".into(),
            name: "House insurance policy".into(),
            identifiers: "POL-884422".into(),
            note: "Kept by the user; original annotation survives.".into(),
            facts: vec![CardFact {
                label: "Policy number".into(),
                value: "POL-884422".into(),
            }],
            attachments: vec![
                CardAttachment {
                    source_id: "qa-policy-source".into(),
                    notebook_id: "qa-house".into(),
                    status: "confirmed".into(),
                    matched: "manual".into(),
                    at: 1,
                },
                CardAttachment {
                    source_id: "qa-notion-child".into(),
                    notebook_id: "qa-finance".into(),
                    status: "confirmed".into(),
                    matched: "name".into(),
                    at: 1,
                },
            ],
            origin: String::new(),
            triage: String::new(),
            mentions: 0,
            surfaced: false,
            created_at: 1,
            updated_at: 1,
        };
        db.add_registry_card(&card).await.unwrap();
        let mut suggested = card.clone();
        suggested.id = "qa-suggestion".into();
        suggested.origin = "auto".into();
        db.add_registry_card(&suggested).await.unwrap();
        let stamp = chrono::Utc::now().timestamp_millis();
        for (id, role, content, citations) in [
            (
                "qa-question",
                "user",
                "What is my house policy number?",
                vec![],
            ),
            (
                "qa-answer",
                "assistant",
                "Your policy number is POL-884422 [1].",
                vec![crate::models::MetaCitation {
                    kind: "card".into(),
                    notebook_id: String::new(),
                    notebook_title: "Registry".into(),
                    id: card.id.clone(),
                    title: card.name.clone(),
                    snippet: "Policy number: POL-884422".into(),
                }],
            ),
        ] {
            db.add_meta_turn(&crate::models::MetaTurn {
                id: id.into(),
                thread_id: "qa-global".into(),
                role: role.into(),
                content: content.into(),
                citations,
                kind: "chat".into(),
                model: String::new(),
                scope: String::new(),
                created_at: stamp,
            })
            .await
            .unwrap();
        }
    }
}
