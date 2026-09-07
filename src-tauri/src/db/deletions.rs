//! Immutable local deletion receipts distinguish an interrupted insertion from
//! a row deliberately removed after insertion. They contain identities only.
use super::Db;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

impl Db {
    fn deletion_receipt(&self, kind: &str, id: &str) -> PathBuf {
        let key = format!("{kind}\0{id}");
        self.dir
            .join("sync-deletions")
            .join(format!("{:x}.json", Sha256::digest(key.as_bytes())))
    }

    pub(crate) fn was_deleted(&self, kind: &str, id: &str) -> Result<bool> {
        let bytes = match std::fs::read(self.deletion_receipt(kind, id)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("Could not read deletion receipt"),
        };
        let receipt: (String, String) =
            serde_json::from_slice(&bytes).context("Invalid deletion receipt")?;
        anyhow::ensure!(
            receipt.0 == kind && receipt.1 == id,
            "Deletion receipt identity mismatch"
        );
        Ok(true)
    }

    pub(super) fn record_deletions(&self, kind: &str, ids: &[&str]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let dir = self.dir.join("sync-deletions");
        std::fs::create_dir_all(&dir).context("Could not create deletion receipt directory")?;
        std::fs::File::open(&self.dir)?.sync_all()?;
        for id in ids {
            if self.was_deleted(kind, id)? {
                continue;
            }
            let mut staged = tempfile::NamedTempFile::new_in(&dir)?;
            staged.write_all(&serde_json::to_vec(&(kind, id))?)?;
            staged.as_file().sync_all()?;
            // Parallel deletion of the same ID publishes the same receipt.
            staged
                .persist(self.deletion_receipt(kind, id))
                .map_err(|err| err.error)?;
        }
        std::fs::File::open(&dir)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn receipts_are_typed_durable_and_do_not_interpret_ids_as_paths() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).await.unwrap();
        db.record_deletions("note", &["../untrusted", "same"])
            .unwrap();
        db.record_deletions("source", &["same"]).unwrap();
        assert!(db.was_deleted("note", "../untrusted").unwrap());
        assert!(!db.was_deleted("notebook", "same").unwrap());
        assert_eq!(
            std::fs::read_dir(dir.path().join("sync-deletions"))
                .unwrap()
                .count(),
            3
        );
        drop(db);
        let db = Db::open(dir.path()).await.unwrap();
        assert!(db.was_deleted("source", "same").unwrap());
        assert!(db.was_deleted("note", "same").unwrap());
    }
}
