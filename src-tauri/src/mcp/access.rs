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
//! One table names every tool, and a test holds it to the live tool catalog:
//! a new tool fails the build until someone decides which it is and what to
//! call it. Anything not
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

use Access::{Read, Write};

/// Every tool the server serves: what it does to the library, and what to
/// call it when it shows up in front of the user — an agent's step trail
/// ("List your notebooks") or a permission prompt ("Claude Code wants to
/// create a note"). Agents report our tools by their wire names
/// (`mcp__alchemy__create_note`, `mcp.alchemy.create_note`); we know every
/// one, so the user never has to read them.
///
/// The phrase is a verb phrase in lower case, to read after "wants to".
///
/// Reaching the web to look (discover_feeds, suggest_notebook fetching a
/// URL) still counts as a read: nothing in the library or on the machine
/// changes. Some writes are judgment calls, recorded so they stay decided:
/// `second_look` files its report as a note, `suggest_cards` lands
/// suggestions in the registry, `export_note` writes a file to disk, and
/// `settings` is one tool for reading and changing the app's configuration —
/// it asks, because the cautious reading wins.
const TOOLS: &[(&str, Access, &str)] = &[
    ("activity_stats", Read, "check usage stats"),
    ("add_registry_card", Write, "add a registry entry"),
    ("add_reminder", Write, "add an Apple reminder"),
    ("add_source", Write, "add a source"),
    ("apply_cover_images", Write, "set cover images on sources"),
    ("archive_notebook", Write, "archive a notebook"),
    ("ask_everything", Read, "search all your notebooks"),
    ("ast_search", Read, "search code by structure"),
    (
        "attach_source",
        Write,
        "attach a source to a registry entry",
    ),
    ("bind_notebook_okf", Write, "connect a notebook to a folder"),
    (
        "cards_for_source",
        Read,
        "look up a source's registry entries",
    ),
    ("commission_run", Write, "start a report run"),
    ("complete_reminder", Write, "complete an Apple reminder"),
    ("corpus_timeline", Read, "read your library's timeline"),
    ("create_note", Write, "create a note"),
    ("create_notebook", Write, "create a notebook"),
    ("delete_note", Write, "delete a note"),
    ("delete_notebook", Write, "delete a notebook"),
    ("delete_registry_card", Write, "delete a registry entry"),
    ("delete_schedule", Write, "delete a scheduled report"),
    ("delete_source", Write, "delete a source"),
    ("delete_template", Write, "delete a template"),
    ("deletion_proposals", Read, "review proposed deletions"),
    ("discover_feeds", Read, "find feeds to follow"),
    ("export_note", Write, "export a note to a file"),
    ("generate", Write, "generate a document"),
    ("get_home_chat", Read, "read a chat"),
    ("get_note", Read, "read a note"),
    ("get_source", Read, "read a source"),
    ("grep_sources", Read, "search source files"),
    ("grow", Read, "see what a notebook is missing"),
    ("judge_activity", Read, "check what the judge has decided"),
    ("list_home_chats", Read, "list your chats"),
    ("list_notebooks", Read, "list your notebooks"),
    ("list_notes", Read, "list a notebook's notes"),
    ("list_receipts", Read, "check recent background runs"),
    ("list_registry", Read, "list registry entries"),
    ("triage_preview", Read, "preview the typed registry triage"),
    ("list_schedules", Read, "list scheduled reports"),
    ("list_shared_notebooks", Read, "list shared notebooks"),
    ("list_source_events", Read, "check recent source changes"),
    ("list_sources", Read, "list a notebook's sources"),
    ("list_templates", Read, "list templates"),
    ("open_shared_notebook", Write, "add a shared notebook"),
    ("rebuild_note", Write, "rebuild a note"),
    ("recent_errors", Read, "check recent errors"),
    ("refresh_source", Write, "refresh a source"),
    ("rename_notebook", Write, "rename a notebook"),
    (
        "resolve_deletion_proposal",
        Write,
        "resolve a proposed deletion",
    ),
    (
        "rule_all_suggested",
        Write,
        "accept or dismiss suggested registry entries",
    ),
    ("save_template", Write, "save a template"),
    (
        "scan_cover_images",
        Read,
        "look for cover images for sources",
    ),
    ("schedule_report", Write, "schedule a report"),
    ("search", Read, "search a notebook"),
    ("search_debug", Read, "inspect a search"),
    (
        "second_look",
        Write,
        "fact-check a draft and file the report",
    ),
    (
        "set_attachment_status",
        Write,
        "change a registry attachment",
    ),
    ("set_source_image", Write, "change a source's image"),
    ("set_source_note", Write, "edit a source's note"),
    ("set_source_tags", Write, "change a source's tags"),
    ("set_source_url", Write, "change a source's URL"),
    ("settings", Write, "read or change Alchemy's settings"),
    ("share_notebook", Write, "share a notebook"),
    ("source_hygiene", Read, "check sources for problems"),
    ("suggest_cards", Write, "suggest registry entries"),
    ("suggest_notebook", Read, "suggest where a source belongs"),
    (
        "unbind_notebook_okf",
        Write,
        "disconnect a notebook from its folder",
    ),
    ("update_mac_note", Write, "edit an Apple Note"),
    ("update_note", Write, "edit a note"),
    ("update_registry_card", Write, "edit a registry entry"),
    ("update_source", Write, "edit a source"),
];

fn entry(tool: &str) -> Option<&'static (&'static str, Access, &'static str)> {
    TOOLS.iter().find(|(name, _, _)| *name == tool)
}

/// The access an Alchemy tool needs. Unknown names are writes: fail toward
/// asking.
pub(crate) fn access(tool: &str) -> Access {
    entry(tool).map_or(Access::Write, |(_, access, _)| *access)
}

/// What to call one of our tools after "wants to": "create a note".
pub(crate) fn action(tool: &str) -> Option<&'static str> {
    entry(tool).map(|(_, _, phrase)| *phrase)
}

/// An agent's tool call title in the user's words, if it is one of ours:
/// `mcp__alchemy__list_notebooks` → "List your notebooks". Anything else
/// (the agent's own tools, other servers) is left as the agent named it.
pub(crate) fn human_title(raw: &str) -> Option<String> {
    let phrase = action(alchemy_tool(raw)?)?;
    let mut chars = phrase.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
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

    /// Every tool the server serves is in the table exactly once, and the
    /// table names nothing that no longer exists. A new tool fails here until
    /// someone decides whether it reads or writes and what to call it.
    #[test]
    fn every_tool_is_classified_once() {
        let catalog: BTreeSet<String> = super::super::tool_catalog()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let listed: BTreeSet<String> = TOOLS.iter().map(|(n, _, _)| n.to_string()).collect();
        assert_eq!(TOOLS.len(), listed.len(), "a tool is listed twice");
        let unclassified: Vec<_> = catalog.difference(&listed).collect();
        assert!(
            unclassified.is_empty(),
            "classify and name these: {unclassified:?}"
        );
        let stale: Vec<_> = listed.difference(&catalog).collect();
        assert!(stale.is_empty(), "no longer served: {stale:?}");
    }

    /// The names read as a person would say them: a lower-case verb phrase
    /// with no wire name left in it, so "wants to {phrase}" is a sentence.
    #[test]
    fn every_name_is_a_plain_phrase() {
        for (tool, _, phrase) in TOOLS {
            let first = phrase.chars().next().unwrap();
            assert!(
                first.is_lowercase(),
                "{tool}: {phrase:?} should start lower case"
            );
            assert!(
                !phrase.contains('_'),
                "{tool}: {phrase:?} reads like a wire name"
            );
            assert!(
                !phrase.ends_with('.'),
                "{tool}: {phrase:?} is a phrase, not a sentence"
            );
        }
    }

    /// Wire names from both agents become the same words; anything not ours
    /// keeps the agent's own title.
    #[test]
    fn titles_read_in_the_users_words() {
        assert_eq!(
            human_title("mcp__alchemy__list_notebooks").as_deref(),
            Some("List your notebooks")
        );
        assert_eq!(
            human_title("mcp.alchemy.create_note").as_deref(),
            Some("Create a note")
        );
        assert_eq!(action("create_note"), Some("create a note"));
        assert_eq!(human_title("ToolSearch"), None);
        assert_eq!(human_title("mcp__github__search"), None);
        assert_eq!(human_title("mcp__alchemy__not_a_tool"), None);
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
