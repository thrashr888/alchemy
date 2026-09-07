//! Sync may await reads and durable conflict copies before publishing. Match
//! the complete inspected row at commit so a concurrent edit is never lost.
use super::*;

impl Db {
    pub(crate) async fn update_note_if_unchanged(
        &self,
        expected: &Note,
        updated: &Note,
    ) -> Result<bool> {
        anyhow::ensure!(
            expected.id == updated.id && expected.notebook_id == updated.notebook_id,
            "Cannot change a note identity during sync"
        );
        let _guard = self.note_index_lock.lock().await;
        let schema = notes_schema();
        self.replace_inspected_row(
            T_NOTES,
            note_batch(&schema, std::slice::from_ref(expected))?,
            note_batch(&schema, std::slice::from_ref(updated))?,
        )
        .await
    }

    pub(crate) async fn replace_source_row_if_unchanged(
        &self,
        expected: &Source,
        updated: &Source,
    ) -> Result<bool> {
        anyhow::ensure!(
            expected.id == updated.id && expected.notebook_id == updated.notebook_id,
            "Cannot change a source identity during sync"
        );
        let schema = sources_schema();
        self.replace_inspected_row(
            T_SOURCES,
            source_batch(&schema, std::slice::from_ref(expected))?,
            source_batch(&schema, std::slice::from_ref(updated))?,
        )
        .await
    }

    /// The receipt records the remote delete's intent. A changed snapshot is
    /// left intact (including chunks), so its newer content can be preserved.
    pub(crate) async fn delete_note_if_unchanged(&self, expected: &Note) -> Result<bool> {
        let _guard = self.note_index_lock.lock().await;
        self.record_deletions("note", &[&expected.id])?;
        let schema = notes_schema();
        let row = note_batch(&schema, std::slice::from_ref(expected))?;
        if !self.delete_inspected_row(T_NOTES, &row).await? {
            return Ok(false);
        }
        self.delete_note_chunks(&expected.id).await?;
        self.delete_where(T_NOTE_USAGE, &format!("note_id = '{}'", esc(&expected.id)))
            .await?;
        Ok(true)
    }

    pub(crate) async fn delete_source_if_unchanged(&self, expected: &Source) -> Result<bool> {
        self.record_deletions("source", &[&expected.id])?;
        let schema = sources_schema();
        let row = source_batch(&schema, std::slice::from_ref(expected))?;
        if !self.delete_inspected_row(T_SOURCES, &row).await? {
            return Ok(false);
        }
        let pred = format!(
            "source_id = '{0}' OR source_id = '{GIST_CHUNK_PREFIX}{0}' OR source_id = '{SECTION_CHUNK_PREFIX}{0}' OR source_id = '{SNOTE_CHUNK_PREFIX}{0}'",
            esc(&expected.id),
        );
        self.delete_where(T_CHUNKS, &pred).await?;
        Ok(true)
    }

    async fn delete_inspected_row(&self, table: &str, expected: &RecordBatch) -> Result<bool> {
        let table = self.conn.open_table(table).execute().await?;
        Ok(table
            .delete(&inspected_predicate(expected)?)
            .await?
            .num_deleted_rows
            == 1)
    }

    async fn replace_inspected_row(
        &self,
        table: &str,
        expected: RecordBatch,
        updated: RecordBatch,
    ) -> Result<bool> {
        let schema = expected.schema();
        let predicate = inspected_predicate(&expected)?;
        let table = self.conn.open_table(table).execute().await?;
        let mut update = table.update().only_if(predicate);
        for field in schema.fields() {
            update = update.column(
                field.name(),
                field_literal(&updated, field.name(), field.data_type())?,
            );
        }
        Ok(update.execute().await?.rows_updated == 1)
    }
}

fn field_literal(batch: &RecordBatch, name: &str, kind: &DataType) -> Result<String> {
    match kind {
        DataType::Utf8 => Ok(format!("'{}'", esc(str_col(batch, name)?.value(0)))),
        DataType::Int64 => Ok(i64_col(batch, name)?.value(0).to_string()),
        _ => anyhow::bail!("Unsupported sync comparison field {name}"),
    }
}

fn inspected_predicate(expected: &RecordBatch) -> Result<String> {
    expected
        .schema()
        .fields()
        .iter()
        .map(|field| {
            Ok(format!(
                "{} = {}",
                field.name(),
                field_literal(expected, field.name(), field.data_type())?
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join(" AND "))
}
