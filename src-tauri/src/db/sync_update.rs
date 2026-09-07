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

    async fn replace_inspected_row(
        &self,
        table: &str,
        expected: RecordBatch,
        updated: RecordBatch,
    ) -> Result<bool> {
        fn literal(batch: &RecordBatch, name: &str, kind: &DataType) -> Result<String> {
            match kind {
                DataType::Utf8 => Ok(format!("'{}'", esc(str_col(batch, name)?.value(0)))),
                DataType::Int64 => Ok(i64_col(batch, name)?.value(0).to_string()),
                _ => anyhow::bail!("Unsupported sync comparison field {name}"),
            }
        }
        let schema = expected.schema();
        let predicate = schema
            .fields()
            .iter()
            .map(|field| {
                Ok(format!(
                    "{} = {}",
                    field.name(),
                    literal(&expected, field.name(), field.data_type())?
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(" AND ");
        let table = self.conn.open_table(table).execute().await?;
        let mut update = table.update().only_if(predicate);
        for field in schema.fields() {
            update = update.column(
                field.name(),
                literal(&updated, field.name(), field.data_type())?,
            );
        }
        Ok(update.execute().await?.rows_updated == 1)
    }
}
