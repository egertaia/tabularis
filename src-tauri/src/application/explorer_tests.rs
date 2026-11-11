use super::*;
use crate::drivers::driver_trait::DatabaseDriver;
use crate::drivers::mysql::MysqlDriver;
use crate::drivers::postgres::PostgresDriver;
use crate::drivers::sqlite::SqliteDriver;
use std::sync::Arc;

struct NoSecrets;

impl crate::runtime::secrets::RuntimeSecrets for NoSecrets {
    fn get(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn caps(driver: &dyn DatabaseDriver) -> DriverCapabilities {
    driver.manifest().capabilities.clone()
}

#[test]
fn open_table_query_quotes_per_driver_capabilities() {
    let (sqlite, postgres, mysql) = (
        caps(&SqliteDriver::new()),
        caps(&PostgresDriver::new()),
        caps(&MysqlDriver::new()),
    );
    assert_eq!(
        select_all_sql("sqlite", &sqlite, None, "my\"table"),
        r#"SELECT * FROM "my""table""#
    );
    assert_eq!(
        select_all_sql("postgres", &postgres, Some("public"), "users"),
        r#"SELECT * FROM "public"."users""#
    );
    assert_eq!(
        select_all_sql("mysql", &mysql, None, "users"),
        "SELECT * FROM `users`"
    );
}

#[tokio::test]
async fn lists_sqlite_tables_and_views_then_columns_with_types() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = RuntimeContext::new(
        Arc::new(crate::runtime::paths::FixedRuntimePaths::new(
            temp.path().into(),
            temp.path().into(),
        )),
        Arc::new(crate::runtime::events::NoopRuntimeEvents),
        Arc::new(NoSecrets),
    );
    let database = temp.path().join("explorer.sqlite");
    {
        use sqlx::{Connection, Executor};
        let mut connection = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&database)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        connection
            .execute(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                 CREATE VIEW user_names AS SELECT name FROM users;",
            )
            .await
            .unwrap();
    }
    crate::drivers::registry::register_driver(SqliteDriver::new()).await;
    let saved: crate::models::SavedConnection = serde_json::from_value(serde_json::json!({
        "id": "explored",
        "name": "Explored",
        "params": { "driver": "sqlite", "database": database.to_string_lossy() },
    }))
    .unwrap();
    crate::persistence::save_connections_file(
        &runtime.paths.connections_file(),
        &crate::models::ConnectionsFile {
            connections: vec![saved],
            ..Default::default()
        },
    )
    .unwrap();

    let location = Location::default();
    let entries = list(&runtime, "explored", &location).await.unwrap();
    let table = Entry {
        kind: EntryKind::Table,
        name: "users".into(),
    };
    let view = Entry {
        kind: EntryKind::View,
        name: "user_names".into(),
    };
    assert_eq!(entries, vec![table.clone(), view]);

    let columns = columns(&runtime, "explored", &location, &table)
        .await
        .unwrap();
    let described: Vec<_> = columns
        .iter()
        .map(|c| (c.name.as_str(), c.data_type.as_str()))
        .collect();
    assert_eq!(described, vec![("id", "INTEGER"), ("name", "TEXT")]);
    assert_eq!(
        open_table_query(&runtime, "explored", &location, "users")
            .await
            .unwrap(),
        r#"SELECT * FROM "users""#
    );
}
