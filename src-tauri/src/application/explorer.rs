use crate::application::{connections, metadata};
use crate::drivers::driver_trait::DriverCapabilities;
use crate::dump_utils::DumpDialect;
use crate::models::TableColumn;
use crate::runtime::RuntimeContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Database,
    Schema,
    Table,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    pub database: Option<String>,
    pub schema: Option<String>,
}

async fn capabilities(
    runtime: &RuntimeContext,
    connection_id: &str,
) -> Result<(String, DriverCapabilities), String> {
    let driver_id = connections::saved_connection_driver(runtime, connection_id)?;
    let driver = crate::drivers::registry::get_driver(&driver_id)
        .await
        .ok_or_else(|| format!("Unsupported driver: {driver_id}"))?;
    Ok((driver_id, driver.manifest().capabilities.clone()))
}

fn next_level(caps: &DriverCapabilities, location: &Location) -> EntryKind {
    let multi_database = !caps.no_connection_required
        && !caps.single_database
        && !caps.file_based
        && !caps.folder_based
        && !caps.schemas;
    if caps.schemas && location.schema.is_none() {
        EntryKind::Schema
    } else if multi_database && location.database.is_none() {
        EntryKind::Database
    } else {
        EntryKind::Table
    }
}

/// The next explorer level under `location`: schemas, databases, or tables and views.
pub async fn list(
    runtime: &RuntimeContext,
    connection_id: &str,
    location: &Location,
) -> Result<Vec<Entry>, String> {
    let (_, caps) = capabilities(runtime, connection_id).await?;
    let named = |kind, names: Vec<String>| {
        names
            .into_iter()
            .map(|name| Entry { kind, name })
            .collect::<Vec<_>>()
    };
    let database = location.database.clone();
    match next_level(&caps, location) {
        EntryKind::Schema => Ok(named(
            EntryKind::Schema,
            metadata::get_schemas(runtime, None, connection_id, database).await?,
        )),
        EntryKind::Database => Ok(named(
            EntryKind::Database,
            metadata::get_available_databases(runtime, None, connection_id).await?,
        )),
        _ => {
            let schema = location.schema.clone();
            let tables = metadata::get_tables(
                runtime,
                None,
                connection_id,
                schema.clone(),
                database.clone(),
            )
            .await?;
            let mut entries = named(
                EntryKind::Table,
                tables.into_iter().map(|table| table.name).collect(),
            );
            if caps.views {
                let views =
                    metadata::get_views(runtime, None, connection_id, schema, database).await?;
                entries.extend(named(
                    EntryKind::View,
                    views.into_iter().map(|view| view.name).collect(),
                ));
            }
            Ok(entries)
        }
    }
}

pub async fn columns(
    runtime: &RuntimeContext,
    connection_id: &str,
    location: &Location,
    entry: &Entry,
) -> Result<Vec<TableColumn>, String> {
    let (name, schema, database) = (
        entry.name.clone(),
        location.schema.clone(),
        location.database.clone(),
    );
    if entry.kind == EntryKind::View {
        metadata::get_view_columns(runtime, None, connection_id, name, schema, database).await
    } else {
        metadata::get_columns(runtime, None, connection_id, name, schema, database).await
    }
}

/// The query the explorer's "open table" action puts in the editor.
pub async fn open_table_query(
    runtime: &RuntimeContext,
    connection_id: &str,
    location: &Location,
    table: &str,
) -> Result<String, String> {
    let (driver_id, caps) = capabilities(runtime, connection_id).await?;
    Ok(select_all_sql(
        &driver_id,
        &caps,
        location.schema.as_deref(),
        table,
    ))
}

fn select_all_sql(
    driver_id: &str,
    caps: &DriverCapabilities,
    schema: Option<&str>,
    table: &str,
) -> String {
    format!(
        "SELECT * FROM {}",
        DumpDialect::from_capabilities(driver_id, caps).table_ref(schema, table)
    )
}

#[cfg(test)]
#[path = "explorer_tests.rs"]
mod tests;
