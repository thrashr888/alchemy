//! Durable conflict copies outside notes/ and sources/: inspectable Markdown,
//! never imported as new corpus entries. Stable names make retries idempotent.
use super::*;
use std::io::Write;

pub(super) struct Context<'a> {
    pub bundle: &'a Path,
    pub rel: &'a str,
    pub base_local_hash: &'a str,
}

impl Context<'_> {
    pub fn preserve_local_if_diverged(
        &self,
        local: &OkfConcept,
        incoming_title: &str,
        incoming_body: &str,
    ) -> Result<(), String> {
        if (local.title == incoming_title && local.content == incoming_body)
            || (!self.base_local_hash.is_empty()
                && local_concept_hash(local) == self.base_local_hash)
        {
            return Ok(());
        }
        self.preserve_local(local)
    }

    pub fn preserve_local_before_delete(&self, local: &OkfConcept) -> Result<(), String> {
        if !self.base_local_hash.is_empty() && local_concept_hash(local) == self.base_local_hash {
            return Ok(());
        }
        self.preserve_local(local)
    }

    fn preserve_local(&self, local: &OkfConcept) -> Result<(), String> {
        // An absent baseline cannot prove that overwriting is safe. Preserve
        // the existing text conservatively when recovering a legacy claim.
        let text = format!(
            "{}{}\n",
            okf_frontmatter(local, &okf_description(&local.content), &HashMap::new()),
            local.content,
        );
        self.preserve("local", &text)
    }

    pub fn preserve_remote(&self, doc: &OkfDoc) -> Result<(), String> {
        let front = serde_yaml_ng::to_string(&doc.front).map_err(|err| err.to_string())?;
        self.preserve("remote", &format!("---\n{front}---\n\n{}", doc.body))
    }

    fn preserve(&self, side: &str, text: &str) -> Result<(), String> {
        let identity = okf_hash(&serde_json::json!([self.rel, side, text]).to_string());
        let directory = self.bundle.join("conflicts");
        let path = directory.join(format!("{identity}.md"));
        let copy = format!(
            "# Recovered sync conflict\n\nOriginal document: `{}`\n\nPreserved version: {side}\n\n{text}",
            self.rel,
            text = text,
        );
        let save = || -> std::io::Result<()> {
            std::fs::create_dir_all(&directory)?;
            // Persist the new directory itself before relying on its contents.
            std::fs::File::open(self.bundle)?.sync_all()?;
            match std::fs::read(&path) {
                Ok(existing) if existing == copy.as_bytes() => {}
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "existing conflict copy has different content",
                    ))
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
                    staged.write_all(copy.as_bytes())?;
                    staged.as_file().sync_all()?;
                    if let Err(err) = staged.persist_noclobber(&path) {
                        if err.error.kind() != std::io::ErrorKind::AlreadyExists
                            || std::fs::read(&path)? != copy.as_bytes()
                        {
                            return Err(err.error);
                        }
                    }
                }
                Err(err) => return Err(err),
            }
            std::fs::File::open(&directory)?.sync_all()?;
            Ok(())
        };
        save()
            .map_err(|err| format!("Could not preserve sync conflict {}: {err}", path.display()))?;

        // Preserve the existing log policy too, but never truncate an unreadable
        // log or advance reconciliation past a failed write. The immutable copy
        // above remains the durable record if cloud clients race over log.md.
        let log = self.bundle.join("log.md");
        let marker = format!("<!-- alchemy-conflict:{identity} -->");
        let mut existing = match std::fs::read_to_string(&log) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => "# Log\n".into(),
            Err(err) => {
                return Err(format!(
                    "Could not read conflict log {}: {err}",
                    log.display()
                ))
            }
        };
        if existing.contains(&marker) {
            return Ok(());
        }
        existing.push_str(&format!(
            "\n{marker}\n\n## Recovered {side} version of {}\n\n[Preserved copy](conflicts/{identity}.md)\n\n{text}\n",
            self.rel,
        ));
        let save_log = || -> std::io::Result<()> {
            let mut staged = tempfile::NamedTempFile::new_in(self.bundle)?;
            staged.write_all(existing.as_bytes())?;
            staged.as_file().sync_all()?;
            staged.persist(&log).map_err(|err| err.error)?;
            std::fs::File::open(self.bundle)?.sync_all()?;
            Ok(())
        };
        save_log().map_err(|err| format!("Could not write conflict log {}: {err}", log.display()))
    }
}

#[cfg(test)]
mod tests;
