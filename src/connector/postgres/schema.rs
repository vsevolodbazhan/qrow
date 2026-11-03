//! Describe user SQL without retaining recursive server type metadata.
use super::*;
use futures_util::TryStreamExt;
use std::collections::{BTreeSet, HashMap};
use tokio_postgres::{Statement, types::ToSql};

pub(super) const MAX_SQL_BYTES: usize = 1024 * 1024;

pub(super) async fn types(
    client: &Client,
    cancel: &Cancel,
    statement: &Statement,
) -> Result<Vec<String>> {
    let unknown: BTreeSet<_> = statement
        .columns()
        .iter()
        .filter(|column| column.type_().name().is_empty())
        .map(|column| column.type_().oid())
        .collect();
    let mut names = HashMap::new();
    if !unknown.is_empty() {
        // OIDs are integers received in Describe, never SQL text from the user.
        let oids = unknown
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "SELECT oid::pg_catalog.int8, typname::pg_catalog.text FROM pg_catalog.pg_type WHERE oid IN ({oids})"
        );
        let description = cancel.until(client.prepare_bounded(&query)).await?;
        let stream = cancel
            .until(client.query_raw(&description, std::iter::empty::<&(dyn ToSql + Sync)>()))
            .await?;
        pin_mut!(stream);
        while let Some(row) = cancel.until(stream.try_next()).await? {
            let oid: i64 = row.try_get(0)?;
            let oid = u32::try_from(oid).context("Invalid Postgres type OID")?;
            let name: &str = row.try_get(1)?;
            anyhow::ensure!(
                unknown.contains(&oid)
                    && !names.contains_key(&oid)
                    && names.len() < 4096
                    && !name.is_empty()
                    && name.len() <= 1024,
                "Postgres type metadata exceeds its limit"
            );
            names.insert(oid, name.to_owned());
        }
        anyhow::ensure!(
            names.len() == unknown.len(),
            "Postgres type metadata is incomplete"
        );
    }
    statement
        .columns()
        .iter()
        .map(|column| {
            let name = if column.type_().name().is_empty() {
                names
                    .get(&column.type_().oid())
                    .context("Postgres type metadata is missing")?
                    .as_str()
            } else {
                column.type_().name()
            };
            Ok(export_type(name, column.type_modifier()))
        })
        .collect()
}
