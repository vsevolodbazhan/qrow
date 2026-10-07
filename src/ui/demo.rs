//! Synthetic catalog and dbt data for the demo window.

use crate::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind},
    dbt,
    model::CatalogSettings,
};
use std::{collections::BTreeMap, io::Write};
use uuid::Uuid;

const MANIFEST: &[u8] = include_bytes!("../../assets/demo/manifest.json");

/// Keep the file alive for the window so the normal dbt SQL readers can
/// read its spans. The file is removed when the window releases it.
pub(super) fn manifest() -> anyhow::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("qrow-demo-")
        .suffix("-manifest.json")
        .tempfile()?;
    file.write_all(MANIFEST)?;
    Ok(file)
}

/// The catalog and the dbt project describe the same tables and columns.
pub(super) fn catalog(profile: Uuid) -> Catalog {
    let index = dbt::parse(MANIFEST).expect("the bundled demo manifest is valid");
    let mut schemas = BTreeMap::<&str, Vec<&dbt::Entry>>::new();
    for entry in index.entries() {
        schemas
            .entry(index.symbol(entry.schema))
            .or_default()
            .push(entry);
    }
    let mut catalog = Catalog::empty(profile, None);
    let at = crate::catalog::now();
    catalog.apply_schemas(
        schemas.keys().map(|schema| (*schema).into()).collect(),
        &CatalogSettings::default(),
        at,
    );
    for (schema, entries) in schemas {
        catalog.apply_relations(
            schema,
            None,
            entries
                .iter()
                .map(|entry| RelationEntry {
                    name: entry.identifier.to_string(),
                    kind: RelationKind::Table,
                    comment: Some(entry.description.to_string()),
                })
                .collect(),
            at,
        );
        catalog.apply_columns(
            schema,
            None,
            entries
                .iter()
                .map(|entry| {
                    let columns = entry
                        .columns
                        .iter()
                        .map(|column| CatalogColumn {
                            name: column.name.to_string(),
                            data_type: column
                                .data_type
                                .map(|symbol| index.symbol(symbol))
                                .unwrap_or("STRING")
                                .into(),
                            comment: Some(column.description.to_string()),
                        })
                        .collect();
                    (entry.identifier.to_string(), columns)
                })
                .collect(),
            at,
        );
    }
    catalog
}
