// The only place native views call the backend.
use crate::application::connections::{self, ConnectionCommand};
use crate::application::explorer::{self, Entry, Location};
use crate::application::persistence;
use crate::application::queries::{self, QueryRequestScope};
use crate::application::{records, themes};
use crate::commands::QueryCancellationState;
use crate::config::AppConfig;
use crate::drivers::driver_trait::{PluginManifest, QueryChunk, StreamSummary};
use crate::models::{ConnectionParams, SavedConnection, TableColumn, TestConnectionRequest};
use crate::runtime::{state::ApplicationState, RuntimeContext};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

pub fn load_redacted_connections(path: &Path) -> Result<Vec<SavedConnection>, String> {
    connections::load_redacted_connections(path)
}

pub async fn list_drivers() -> Vec<PluginManifest> {
    crate::drivers::registry::list_drivers().await
}

async fn connection_command<T: serde::de::DeserializeOwned>(
    context: &RuntimeContext,
    state: &Arc<ApplicationState>,
    command: ConnectionCommand,
) -> Result<T, String> {
    let value = connections::execute(context, state, command).await?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub async fn save_connection(
    context: &RuntimeContext,
    state: &Arc<ApplicationState>,
    name: String,
    params: ConnectionParams,
) -> Result<SavedConnection, String> {
    let command = ConnectionCommand::SaveConnection {
        name,
        params,
        plugin_secret_changes: None,
        detect_json_in_text_columns: None,
        environment: None,
    };
    connection_command(context, state, command).await
}

/// `saved` supplies the settings the native form does not edit, so they are kept.
pub async fn update_connection(
    context: &RuntimeContext,
    state: &Arc<ApplicationState>,
    saved: &SavedConnection,
    name: String,
    params: ConnectionParams,
) -> Result<SavedConnection, String> {
    let command = ConnectionCommand::UpdateConnection {
        id: saved.id.clone(),
        name,
        params,
        plugin_secret_changes: None,
        detect_json_in_text_columns: saved.detect_json_in_text_columns,
        environment: saved.environment.clone(),
    };
    connection_command(context, state, command).await
}

pub async fn delete_connection(
    context: &RuntimeContext,
    state: &Arc<ApplicationState>,
    id: String,
) -> Result<(), String> {
    connection_command::<Value>(context, state, ConnectionCommand::DeleteConnection { id })
        .await
        .map(drop)
}

pub async fn disconnect_connection(
    context: &RuntimeContext,
    state: &Arc<ApplicationState>,
    connection_id: &str,
) -> Result<(), String> {
    connections::disconnect_connection(context, Some(state), None, connection_id).await
}

pub async fn test_connection(
    context: &RuntimeContext,
    state: &Arc<ApplicationState>,
    params: ConnectionParams,
    connection_id: Option<String>,
) -> Result<String, String> {
    let request = TestConnectionRequest {
        params,
        connection_id,
        plugin_secret_changes: Default::default(),
        progress_id: None,
    };
    connection_command(
        context,
        state,
        ConnectionCommand::TestConnection { request },
    )
    .await
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
