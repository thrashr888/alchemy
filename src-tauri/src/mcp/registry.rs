//! Registry tools: the confirmed cast of things — assets, people, policies,
//! providers, projects, dependencies — and the documents filed under them.
//! The same cards the user sees on Home, so an agent that files a scanned
//! policy under its card has done the user's filing, not its own.
//!
//! The propose/confirm boundary holds for agents too: `attach_source` files
//! a document deliberately, and the arrival sweep's `proposed` rows are for
//! a human to resolve. An agent may confirm one, but it should say why.

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ErrorData as McpError};

use super::*;
use crate::models::{CardAttachment, CardFact, RegistryCard};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddCardReq {
    /// "asset" (a vehicle, appliance, instrument), "person", "policy" (an
    /// insurance or service contract), "provider" (a company or
    /// practitioner), "project" (a thread of documents in sequence), or
    /// "dependency" (a library or service you rely on).
    kind: String,
    /// What the thing is called — the handle you'd use out loud.
    name: String,
    /// Distinctive identifiers, space-separated: VIN, policy number, serial,
    /// model number. Documents containing one of these are filed under this
    /// card automatically, so only put strings here that could not belong to
    /// anything else. Must be 6+ characters and contain a digit to count.
    #[serde(default)]
    identifiers: Option<String>,
    /// Free-text note about the thing.
    #[serde(default)]
    note: Option<String>,
    /// Key facts as label/value pairs (e.g. "Purchased" / "March 2019").
    #[serde(default)]
    facts: Option<Vec<FactReq>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct FactReq {
    /// Short label, e.g. "Serial", "Renews", "Warranty".
    label: String,
    /// The value.
    #[serde(default)]
    value: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ListRegistryReq {
    /// Filter to one kind.
    #[serde(default)]
    kind: Option<String>,
    /// Filter to cards holding at least one document in this notebook.
    #[serde(default)]
    notebook_id: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct UpdateCardReq {
    /// Card id (from list_registry / add_registry_card).
    card_id: String,
    #[serde(default)]
    name: Option<String>,
    /// Replaces the identifier list wholesale.
    #[serde(default)]
    identifiers: Option<String>,
    #[serde(default)]
    note: Option<String>,
    /// Replaces the fact list wholesale.
    #[serde(default)]
    facts: Option<Vec<FactReq>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AttachReq {
    /// Card id.
    card_id: String,
    /// Source id (from list_sources / search results).
    source_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SetAttachmentReq {
    /// Card id.
    card_id: String,
    /// Source id.
    source_id: String,
    /// "confirmed" to accept a proposal, "rejected" to turn it down (which
    /// is remembered, so the sweep stops re-proposing it), or "remove" to
    /// forget the pair entirely.
    status: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CardIdReq {
    /// Card id.
    card_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RuleSuggestedReq {
    /// "keep" confirms every suggested card — they become the user's and
    /// their documents are re-matched in the background. "keep-recommended"
    /// confirms only the cards the triage pass marked recommended (triage ==
    /// "recommended"), leaving the rest queued. "dismiss" turns them all
    /// down, remembered so the same guesses never come back.
    verdict: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SourceIdReq {
    /// Source id.
    source_id: String,
}

fn to_facts(facts: Option<Vec<FactReq>>) -> Vec<CardFact> {
    facts
        .unwrap_or_default()
        .into_iter()
        .map(|f| CardFact {
            label: f.label,
            value: f.value.unwrap_or_default(),
        })
        .collect()
}

#[tool_router(router = registry_router, vis = "pub(super)")]
impl AlchemyMcp {
    #[tool(
        description = "Create a registry card: a thing the user's documents are about — an asset, person, policy, provider, project, or dependency. Documents attach to it and travel with it across notebooks. Give it identifiers (VIN, policy number, serial) and future documents containing one are filed under it automatically. Prefer this over a note when the user's questions will arrive by the THING (\"is the dishwasher still under warranty?\") rather than by document."
    )]
    async fn add_registry_card(
        &self,
        Parameters(AddCardReq {
            kind,
            name,
            identifiers,
            note,
            facts,
        }): Parameters<AddCardReq>,
    ) -> Result<CallToolResult, McpError> {
        commands::validate_registry_kind(&kind).map_err(invalid)?;
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(invalid("name is empty — a card is a thing with a name"));
        }
        let ts = commands::now();
        let card = RegistryCard {
            id: commands::new_id(),
            kind,
            name,
            origin: String::new(),
            triage: String::new(),
            identifiers: commands::normalize_tags(&identifiers.unwrap_or_default()),
            note: note.unwrap_or_default().trim().to_string(),
            facts: to_facts(facts),
            attachments: Vec::new(),
            created_at: ts,
            updated_at: ts,
        };
        self.state()
            .db
            .add_registry_card(&card)
            .await
            .map_err(internal)?;
        self.changed("registry", None);
        json_result(&card)
    }

    #[tool(
        description = "List the registry's cards (alphabetical), optionally filtered by kind or by a notebook they hold documents in. Each card: id, kind, name, identifiers, note, facts, and attachments with the receipt for why each document is filed there (an identifier string, \"name\", or \"manual\") plus its status (confirmed | proposed | rejected)."
    )]
    async fn list_registry(
        &self,
        Parameters(ListRegistryReq { kind, notebook_id }): Parameters<ListRegistryReq>,
    ) -> Result<CallToolResult, McpError> {
        let mut cards = self.state().db.list_registry().await.map_err(internal)?;
        if let Some(kind) = kind {
            cards.retain(|c| c.kind == kind);
        }
        if let Some(nb) = notebook_id {
            cards.retain(|c| {
                c.attachments
                    .iter()
                    .any(|a| a.notebook_id == nb && a.status != "rejected")
            });
        }
        json_result(&cards)
    }

    #[tool(
        description = "Update a registry card's name, identifiers, note, or facts. Identifiers and facts replace the existing list wholesale — read the card first and send the full list. Kind cannot change: a card that changes kind is a different thing."
    )]
    async fn update_registry_card(
        &self,
        Parameters(UpdateCardReq {
            card_id,
            name,
            identifiers,
            note,
            facts,
        }): Parameters<UpdateCardReq>,
    ) -> Result<CallToolResult, McpError> {
        let db = &self.state().db;
        let Some(mut card) = db.get_registry_card(&card_id).await.map_err(internal)? else {
            return Err(invalid("no card with that id"));
        };
        if let Some(name) = name {
            let name = name.trim().to_string();
            if !name.is_empty() {
                card.name = name;
            }
        }
        if let Some(identifiers) = identifiers {
            card.identifiers = commands::normalize_tags(&identifiers);
        }
        if let Some(note) = note {
            card.note = note.trim().to_string();
        }
        if facts.is_some() {
            card.facts = to_facts(facts);
        }
        card.updated_at = commands::now();
        db.update_registry_card(&card).await.map_err(internal)?;
        self.changed("registry", None);
        json_result(&card)
    }

    #[tool(
        description = "File a document under a card deliberately, as confirmed. Use this when you know the document belongs to the thing — it is the fastest way to build out a card's document thread."
    )]
    async fn attach_source(
        &self,
        Parameters(AttachReq { card_id, source_id }): Parameters<AttachReq>,
    ) -> Result<CallToolResult, McpError> {
        let db = &self.state().db;
        let Some(mut card) = db.get_registry_card(&card_id).await.map_err(internal)? else {
            return Err(invalid("no card with that id"));
        };
        let Some(source) = db.get_source(&source_id).await.map_err(internal)? else {
            return Err(invalid("no source with that id"));
        };
        let ts = commands::now();
        if let Some(a) = card
            .attachments
            .iter_mut()
            .find(|a| a.source_id == source_id)
        {
            a.status = "confirmed".into();
            a.matched = "manual".into();
            a.at = ts;
        } else {
            card.attachments.push(CardAttachment {
                source_id,
                notebook_id: source.notebook_id,
                status: "confirmed".into(),
                matched: "manual".into(),
                at: ts,
            });
        }
        card.updated_at = ts;
        db.update_registry_card(&card).await.map_err(internal)?;
        self.changed("registry", None);
        json_result(&card)
    }

    #[tool(
        description = "Resolve a proposed attachment: confirm it, reject it (remembered, so the arrival sweep stops re-proposing that pair), or remove the pair entirely. Proposals come from a name match and are meant for a human to judge — only confirm one when you can say why it belongs."
    )]
    async fn set_attachment_status(
        &self,
        Parameters(SetAttachmentReq {
            card_id,
            source_id,
            status,
        }): Parameters<SetAttachmentReq>,
    ) -> Result<CallToolResult, McpError> {
        let db = &self.state().db;
        let Some(mut card) = db.get_registry_card(&card_id).await.map_err(internal)? else {
            return Err(invalid("no card with that id"));
        };
        if status == "remove" {
            card.attachments.retain(|a| a.source_id != source_id);
        } else {
            commands::validate_attachment_status(&status).map_err(invalid)?;
            let Some(a) = card
                .attachments
                .iter_mut()
                .find(|a| a.source_id == source_id)
            else {
                return Err(invalid("that document isn't filed under this card"));
            };
            a.status = status;
            a.at = commands::now();
        }
        card.updated_at = commands::now();
        db.update_registry_card(&card).await.map_err(internal)?;
        self.changed("registry", None);
        json_result(&card)
    }

    #[tool(
        description = "The cards a given source is filed under, with the receipt for each. Use this to answer \"what is this document about\" in terms of the user's own cast rather than its filename."
    )]
    async fn cards_for_source(
        &self,
        Parameters(SourceIdReq { source_id }): Parameters<SourceIdReq>,
    ) -> Result<CallToolResult, McpError> {
        let cards = self.state().db.list_registry().await.map_err(internal)?;
        let hits: Vec<RegistryCard> = cards
            .into_iter()
            .filter(|c| {
                c.attachments
                    .iter()
                    .any(|a| a.source_id == source_id && a.status != "rejected")
            })
            .collect();
        json_result(&hits)
    }

    #[tool(
        description = "Rule on suggested cards (origin \"auto\") in bulk: \"keep\" confirms them all into the user's registry, \"keep-recommended\" confirms only the ones the triage pass marked (triage \"recommended\"), \"dismiss\" turns them all down (remembered, so the same guesses never return). Returns how many were ruled. Only do this when the user asked for a sweep — suggestions are normally theirs to judge one by one."
    )]
    async fn rule_all_suggested(
        &self,
        Parameters(RuleSuggestedReq { verdict }): Parameters<RuleSuggestedReq>,
    ) -> Result<CallToolResult, McpError> {
        let (origin, only_recommended) = match verdict.as_str() {
            "keep" => ("", false),
            "keep-recommended" => ("", true),
            "dismiss" => ("dismissed", false),
            _ => {
                return Err(invalid(
                    "verdict must be \"keep\", \"keep-recommended\", or \"dismiss\"",
                ))
            }
        };
        let ruled = commands::rule_all_suggested_cards(&self.state().db, origin, only_recommended)
            .await
            .map_err(internal)?;
        if ruled > 0 {
            self.changed("registry", None);
        }
        json_result(&serde_json::json!({ "ruled": ruled }))
    }

    #[tool(
        description = "Delete a registry card. Its documents are untouched — only the card and its filing go away."
    )]
    async fn delete_registry_card(
        &self,
        Parameters(CardIdReq { card_id }): Parameters<CardIdReq>,
    ) -> Result<CallToolResult, McpError> {
        self.state()
            .db
            .delete_registry_card(&card_id)
            .await
            .map_err(internal)?;
        self.changed("registry", None);
        json_result(&serde_json::json!({ "deleted": true }))
    }
}
