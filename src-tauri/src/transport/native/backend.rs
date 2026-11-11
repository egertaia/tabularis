// The only place native views call the backend.
use crate::application::connections;
use crate::application::explorer::{self, Entry, Location};
use crate::application::persistence;
use crate::application::queries::{self, QueryRequestScope};
use crate::application::{records, themes};
use crate::commands::QueryCancellationState;
use crate::config::AppConfig;
use crate::drivers::driver_trait::{QueryChunk, StreamSummary};
use crate::models::{SavedConnection, TableColumn};
use crate::runtime::RuntimeContext;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

pub fn load_redacted_connections(path: &Path) -> Result<Vec<SavedConnection>, String> {
    connections::load_redacted_connections(path)
}

pub fn load_config(context: &RuntimeContext) -> AppConfig {
    persistence::load_config(context)
}

pub fn theme_catalog(context: &RuntimeContext) -> crate::theme_packages::ThemeCatalog {
    themes::catalog(context)
}

pub async fn list(
    context: &RuntimeContext,
    connection_id: &str,
    location: &Location,
) -> Result<Vec<Entry>, String> {
    explorer::list(context, connection_id, location).await
}

pub async fn columns(
    context: &RuntimeContext,
    connection_id: &str,
    location: &Location,
    entry: &Entry,
) -> Result<Vec<TableColumn>, String> {
    explorer::columns(context, connection_id, location, entry).await
}

pub async fn editable_columns(
    context: &RuntimeContext,
    connection_id: &str,
    location: &Location,
    table: &str,
) -> Result<Option<Vec<TableColumn>>, String> {
    explorer::editable_columns(context, connection_id, location, table).await
}

pub async fn open_table_query(
    context: &RuntimeContext,
    connection_id: &str,
    location: &Location,
    table: &str,
) -> Result<String, String> {
    explorer::open_table_query(context, connection_id, location, table).await
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_query(
    context: &RuntimeContext,
    cancellation: &QueryCancellationState,
    scope: QueryRequestScope<'_>,
    connection_id: String,
    query: String,
    schema: Option<String>,
    query_session_id: Option<String>,
    database: Option<String>,
    sink: impl FnMut(QueryChunk) + Send + 'static,
) -> Result<StreamSummary, String> {
    queries::stream_query(
        context,
        cancellation,
        scope,
        connection_id,
        query,
        schema,
        query_session_id,
        database,
        sink,
    )
    .await
}

pub async fn delete_record(
    context: &RuntimeContext,
    session_id: Option<Uuid>,
    connection_id: &str,
    table: String,
    pk_map: HashMap<String, Value>,
    schema: Option<String>,
    database: Option<String>,
) -> Result<u64, String> {
    records::delete_record(
        context,
        session_id,
        connection_id,
        table,
        pk_map,
        schema,
        database,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_record(
    context: &RuntimeContext,
    session_id: Option<Uuid>,
    connection_id: &str,
    table: String,
    pk_map: HashMap<String, Value>,
    col_name: String,
    new_val: Value,
    schema: Option<String>,
    database: Option<String>,
) -> Result<u64, String> {
    records::update_record(
        context,
        session_id,
        connection_id,
        table,
        pk_map,
        col_name,
        new_val,
        schema,
        database,
    )
    .await
}

pub async fn insert_record(
    context: &RuntimeContext,
    session_id: Option<Uuid>,
    connection_id: &str,
    table: String,
    data: HashMap<String, Value>,
    schema: Option<String>,
    database: Option<String>,
) -> Result<u64, String> {
    records::insert_record(
        context,
        session_id,
        connection_id,
        table,
        data,
        schema,
        database,
    )
    .await
}
