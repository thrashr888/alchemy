//! Which of Alchemy's tools read and which write (docs/RFC-unified-chat.md §4).
//!
//! One classification for every brain that can touch the user's notebooks.
//! The local tool loop holds writes to the user's own words; a hosted agent is
//! held to the same rule by Alchemy answering its permission requests: reads
//! are allowed without asking, writes are shown to the user. The rule is the
//! app's, not the agent's — Claude Code in `auto` mode, Codex's own approval
//! layer and opencode would otherwise each decide differently what may change
//! a notebook, and "consistent across providers" means none of them decides.
//!
//! Both lists are explicit, and a test holds them to the live tool catalog: a
//! new tool fails the build until someone decides which it is. Anything not
//! on the read list is treated as a write at runtime, so a name this module
//! has never heard of asks rather than acts.

/// What a tool does to the user's library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Looks without changing anything: allowed without asking.
    Read,
    /// Adds, changes, deletes, files, or acts outside the app: the user says.
    Write,
}

/// Tools that only look. Reaching the web to look (discover_feeds,
/// suggest_notebook fetching a URL) still counts as looking: nothing in the
/// library or on the machine changes.
const READ: &[&str] = &[
    "activity_stats",
    "ask_everything",
    "ast_search",
    "cards_for_source",
    "corpus_timeline",
    "deletion_proposals",
    "discover_feeds",
    "get_home_chat",
    "get_note",
    "get_source",
    "grep_sources",
    "grow",
    "list_home_chats",
    "list_notebooks",
    "list_notes",
    "list_receipts",
    "list_registry",
    "list_schedules",
    "list_shared_notebooks",
    "list_source_events",
    "list_sources",
    "list_templates",
    "recent_errors",
    "search",
    "search_debug",
    "source_hygiene",
    "suggest_notebook",
];

/// Tools that change something. Some are judgment calls, recorded here so
/// they stay decided: `second_look` files its report as a note,
/// `suggest_cards` lands suggestions in the registry, `export_note` writes a
/// file to disk, and `settings` is one tool for both reading and changing the
/// app's configuration — it asks, because the cautious reading wins.
// Read only by the coverage test: at runtime anything not on READ is a write
// already, so this list's job is to make each one a decision someone made.
#[cfg_attr(not(test), allow(dead_code))]
const WRITE: &[&str] = &[
    "add_registry_card",
    "add_reminder",
    "add_source",
    "archive_notebook",
    "attach_source",
    "bind_notebook_okf",
    "commission_run",
    "complete_reminder",
    "create_note",
    "create_notebook",
    "delete_note",
    "delete_notebook",
    "delete_registry_card",
    "delete_schedule",
    "delete_source",
    "delete_template",
    "export_note",
    "generate",
    "open_shared_notebook",
    "rebuild_note",
    "refresh_source",
    "rename_notebook",
    "resolve_deletion_proposal",
    "rule_all_suggested",
    "save_template",
    "schedule_report",
    "second_look",
    "set_attachment_status",
    "set_source_image",
    "set_source_note",
    "set_source_tags",
    "set_source_url",
    "settings",
    "share_notebook",
    "suggest_cards",
    "unbind_notebook_okf",
    "update_mac_note",
    "update_note",
    "update_registry_card",
    "update_source",
];

/// The access an Alchemy tool needs. Unknown names are writes: fail toward
/// asking.
pub(crate) fn access(tool: &str) -> Access {
    if READ.contains(&tool) {
        Access::Read
    } else {
        Access::Write
    }
}

/// The Alchemy tool an agent's tool call names, if it is one of ours.
///
/// Agents spell an MCP tool differently: Claude Code `mcp__alchemy__search`
/// (or `mcp__plugin_alchemy_alchemy__search` through the Claude Code plugin),
/// Codex `mcp.alchemy.search`. The server segment has to say "alchemy" —
/// otherwise a `search` tool from some other server would be allowed as ours.
pub(crate) fn alchemy_tool(name: &str) -> Option<&str> {
    let name = name.trim();
    let (server, tool) = match name.strip_prefix("mcp__") {
        Some(rest) => rest.rsplit_once("__")?,
        None => name.strip_prefix("mcp.")?.rsplit_once('.')?,
    };
    (server.contains("alchemy") && !tool.is_empty()).then_some(tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every tool the server serves is classified, exactly once, and the
    /// lists name nothing that no longer exists. A new tool fails here until
    /// someone decides whether it reads or writes.
    #[test]
    fn every_tool_is_classified_once() {
        let catalog: BTreeSet<String> = super::super::tool_catalog()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let read: BTreeSet<String> = READ.iter().map(|s| s.to_string()).collect();
        let write: BTreeSet<String> = WRITE.iter().map(|s| s.to_string()).collect();

        let listed = &read | &write;
        let unclassified: Vec<_> = catalog.difference(&listed).collect();
        assert!(unclassified.is_empty(), "classify these: {unclassified:?}");
        let both: Vec<_> = read.intersection(&write).collect();
        assert!(both.is_empty(), "listed as both read and write: {both:?}");
        let stale: Vec<_> = listed.difference(&catalog).collect();
        assert!(stale.is_empty(), "no longer served: {stale:?}");
        assert_eq!(READ.len(), read.len(), "duplicate in READ");
        assert_eq!(WRITE.len(), write.len(), "duplicate in WRITE");
    }

    /// Unknown names ask. A tool added to the server but somehow missed by
    /// the test above must never be waved through.
    #[test]
    fn unknown_tools_are_writes() {
        assert_eq!(access("search"), Access::Read);
        assert_eq!(access("delete_notebook"), Access::Write);
        assert_eq!(access("brand_new_tool"), Access::Write);
    }

    /// The spellings seen live: Claude Code (direct and through the plugin)
    /// and Codex. Another server's `search` is not ours.
    #[test]
    fn agent_spellings_resolve_to_our_tools() {
        assert_eq!(
            alchemy_tool("mcp__alchemy__list_notebooks"),
            Some("list_notebooks")
        );
        assert_eq!(
            alchemy_tool("mcp__plugin_alchemy_alchemy__search"),
            Some("search")
        );
        assert_eq!(
            alchemy_tool("mcp.alchemy.ask_everything"),
            Some("ask_everything")
        );
        assert_eq!(alchemy_tool("mcp__github__search"), None);
        assert_eq!(alchemy_tool("mcp.playwright.click"), None);
        assert_eq!(alchemy_tool("Read file"), None);
        assert_eq!(alchemy_tool("mcp__alchemy__"), None);
    }
}
