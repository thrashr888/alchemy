//! The Desktop Extension bundle Claude Desktop installs (RFC-desktop-apps §2).
//!
//! Claude Desktop has no place to point at a URL, but it does install `.mcpb`
//! bundles through its own sheet, and it runs their servers as stdio children
//! under its own bundled Node — so the Mac needs no Node of its own. The
//! bundle is therefore a zip of two files: a manifest, and the small proxy in
//! `skills/alchemy-mcpb/server/` that forwards stdio JSON-RPC to Alchemy's
//! streamable-HTTP endpoint.
//!
//! Nothing secret goes in. The proxy reads the port and the private bearer
//! token out of `mcp.json` at runtime, so a relaunch that rotates the token
//! or moves the port heals itself with no re-install — and a bundle sitting
//! in app data is not a copy of the key to the user's notebooks.

use std::path::Path;

use anyhow::{Context, Result};

/// The proxy, embedded rather than shipped as a Tauri resource: it has to be
/// readable from the running app, and `include_str!` makes that true of every
/// build, bundled or not.
const PROXY: &str = include_str!("../../skills/alchemy-mcpb/server/index.mjs");

/// The manifest `name`. Claude Desktop mints the extension id itself
/// (`local.dxt.<author>.<name>`), so this is the only field we can recognize
/// our own installation by.
pub const EXTENSION_NAME: &str = "Alchemy";

/// Path inside the bundle, and the `entry_point` the manifest declares.
/// `.mjs` on purpose: the extracted bundle has no `package.json`, so a bare
/// `.js` would be parsed as CommonJS and the proxy's imports would not load.
const ENTRY_POINT: &str = "server/index.mjs";

/// What Claude Desktop reads first. `manifest_version` is what the current
/// spec requires; `dxt_version` rides along because readers from the Desktop
/// Extension era only know that one, and neither confuses the other.
fn manifest(tools: &[(String, Option<String>)]) -> serde_json::Value {
    serde_json::json!({
        "manifest_version": "0.3",
        "dxt_version": "0.1",
        "name": EXTENSION_NAME,
        "display_name": "Alchemy",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Search and read your Alchemy notebooks from Claude.",
        "long_description":
            "Alchemy is a local-first research notebook. This extension connects Claude \
             Desktop to the Alchemy running on this Mac, so Claude can list your \
             notebooks, search them, and read a source's full text with citations. \
             Everything stays on the machine: the connection is to 127.0.0.1 and is \
             authenticated with a private token the app writes for itself. Alchemy has \
             to be running for the extension to answer.",
        "author": {
            "name": "Paul Thrasher",
            "url": "https://github.com/thrashr888/alchemy"
        },
        "homepage": "https://github.com/thrashr888/alchemy",
        "repository": {
            "type": "git",
            "url": "https://github.com/thrashr888/alchemy"
        },
        "license": "MPL-2.0",
        "keywords": ["notebook", "research", "search", "local-first", "rag"],
        "server": {
            "type": "node",
            "entry_point": ENTRY_POINT,
            "mcp_config": {
                "command": "node",
                "args": [format!("${{__dirname}}/{ENTRY_POINT}")],
                "env": {}
            }
        },
        "tools": tools
            .iter()
            .map(|(name, description)| match description {
                Some(text) => serde_json::json!({ "name": name, "description": text }),
                None => serde_json::json!({ "name": name }),
            })
            .collect::<Vec<_>>(),
        // No `claude_desktop` floor: we have no measured version to name, and
        // an invented one would lock out installs that would have worked.
        "compatibility": {
            "platforms": ["darwin"],
            "runtimes": { "node": ">=18.0.0" }
        }
    })
}

/// Write `alchemy.mcpb` — overwriting any earlier one, so the bundle always
/// carries this build's version and this build's proxy.
pub fn write_bundle(path: &Path, tools: &[(String, Option<String>)]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let file = std::fs::File::create(path)
        .with_context(|| format!("could not write {}", path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = Default::default();

    use std::io::Write;
    zip.start_file("manifest.json", options)?;
    zip.write_all(serde_json::to_string_pretty(&manifest(tools))?.as_bytes())?;
    zip.start_file(ENTRY_POINT, options)?;
    zip.write_all(PROXY.as_bytes())?;
    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tools() -> Vec<(String, Option<String>)> {
        vec![
            ("list_notebooks".into(), Some("List notebooks.".into())),
            ("search".into(), Some("Hybrid search.".into())),
            ("get_source".into(), None),
        ]
    }

    fn read(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> String {
        let mut text = String::new();
        zip.by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    /// The bundle is two files, and the manifest points at the one that runs.
    #[test]
    fn bundle_holds_a_manifest_and_the_proxy_it_names() {
        let dir = std::env::temp_dir().join(format!("alchemy-mcpb-{}", uuid::Uuid::new_v4()));
        let path = dir.join("connectors/alchemy.mcpb");
        write_bundle(&path, &tools()).unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&read(&mut zip, "manifest.json"))
            .expect("the manifest has to be JSON Claude Desktop can read");
        let entry = manifest["server"]["entry_point"].as_str().unwrap();
        assert_eq!(entry, ENTRY_POINT);
        assert!(read(&mut zip, entry).contains("mcp-session-id"));

        assert_eq!(manifest["manifest_version"], "0.3");
        assert_eq!(manifest["name"], EXTENSION_NAME);
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            manifest["server"]["mcp_config"]["args"][0],
            "${__dirname}/server/index.mjs"
        );
        assert_eq!(manifest["compatibility"]["platforms"][0], "darwin");
        assert_eq!(manifest["tools"][1]["name"], "search");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The token is read at runtime from mcp.json; a bundle that carried one
    /// would be a stale key lying around in app data.
    #[test]
    fn nothing_secret_is_baked_into_the_bundle() {
        let dir = std::env::temp_dir().join(format!("alchemy-mcpb-{}", uuid::Uuid::new_v4()));
        let path = dir.join("alchemy.mcpb");
        write_bundle(&path, &tools()).unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let proxy = read(&mut zip, ENTRY_POINT);
        assert!(
            proxy.contains("mcp.json"),
            "it discovers the app at runtime"
        );
        assert!(!proxy.contains("Bearer 0"), "no token is written in");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Re-running Connect must replace the bundle, not append to a stale one.
    #[test]
    fn writing_twice_leaves_one_current_bundle() {
        let dir = std::env::temp_dir().join(format!("alchemy-mcpb-{}", uuid::Uuid::new_v4()));
        let path = dir.join("alchemy.mcpb");
        write_bundle(&path, &[]).unwrap();
        write_bundle(&path, &tools()).unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(zip.len(), 2);
        let manifest: serde_json::Value =
            serde_json::from_str(&read(&mut zip, "manifest.json")).unwrap();
        assert_eq!(manifest["tools"].as_array().unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(dir);
    }
}
