use super::{
    detect_blob_mime, detect_mime_type, parse_upload_reference, resolve_upload_value,
    BLOB_TRANSFER_PURPOSE,
};
use crate::application::file_transfers::FileTransferStore;
use base64::Engine;
use bytes::Bytes;
use std::convert::Infallible;
use uuid::Uuid;

async fn store_blob(data_dir: &std::path::Path, owner: Uuid, bytes: &'static [u8]) -> String {
    let metadata = FileTransferStore::new(data_dir)
        .store_upload(
            owner,
            BLOB_TRANSFER_PURPOSE,
            "blob.bin",
            Some("application/octet-stream"),
            futures::stream::once(
                async move { Ok::<Bytes, Infallible>(Bytes::from_static(bytes)) },
            ),
        )
        .await
        .unwrap();
    format!(
        "BLOB_UPLOAD_REF:{}:{}:{}",
        metadata.size, metadata.mime_type, metadata.token
    )
}

#[test]
fn mime_detection_is_bounded_and_uses_magic_bytes() {
    let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);

    assert_eq!(detect_mime_type(&encoded).unwrap(), "image/png");
    assert_eq!(
        detect_blob_mime(&encoded).unwrap(),
        format!("BLOB:8:image/png:{encoded}")
    );
    let oversized_header = base64::engine::general_purpose::STANDARD.encode(vec![0; 8193]);
    assert!(detect_mime_type(&oversized_header).is_err());
}

#[tokio::test]
async fn resolving_an_upload_consumes_its_token_and_cleans_the_temporary_file() {
    let temp = tempfile::tempdir().unwrap();
    let session = Uuid::new_v4();
    let reference = store_blob(temp.path(), session, b"blob").await;
    let (_, _, token) = parse_upload_reference(&reference).unwrap();
    let token = token.to_string();
    let mut value = serde_json::Value::String(reference);

    let upload = resolve_upload_value(temp.path(), Some(session), &mut value, 1024)
        .unwrap()
        .unwrap();
    let consumed_path = value
        .as_str()
        .unwrap()
        .strip_prefix("BLOB_FILE_REF:")
        .unwrap()
        .splitn(3, ':')
        .nth(2)
        .unwrap()
        .to_string();

    assert!(FileTransferStore::new(temp.path())
        .open_upload(session, &token, BLOB_TRANSFER_PURPOSE)
        .await
        .is_err());
    assert!(std::path::Path::new(&consumed_path).exists());
    drop(upload);
    assert!(!std::path::Path::new(&consumed_path).exists());
}

#[test]
fn browser_values_reject_server_file_references() {
    let temp = tempfile::tempdir().unwrap();
    let mut browser_value = serde_json::json!("BLOB_FILE_REF:4:text/plain:/etc/passwd");
    let mut desktop_value = browser_value.clone();

    let error =
        match resolve_upload_value(temp.path(), Some(Uuid::new_v4()), &mut browser_value, 1024) {
            Err(error) => error,
            Ok(_) => panic!("browser file reference should be rejected"),
        };
    assert!(error.contains("cannot contain server file paths"));
    assert!(
        resolve_upload_value(temp.path(), None, &mut desktop_value, 1024)
            .unwrap()
            .is_none()
    );
}

#[test]
fn blob_upload_references_reject_paths_and_non_v4_identifiers() {
    assert!(parse_upload_reference("BLOB_UPLOAD_REF:4:text/plain:../../config.json").is_err());
    assert!(
        parse_upload_reference(&format!("BLOB_UPLOAD_REF:4:text/plain:{}", Uuid::nil())).is_err()
    );
}

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

pub(crate) async fn items_fixture(
    read_only: bool,
) -> (
    tempfile::TempDir,
    crate::runtime::RuntimeContext,
    std::path::PathBuf,
) {
    use sqlx::{Connection, Executor};
    let temp = tempfile::tempdir().unwrap();
    let runtime = crate::runtime::RuntimeContext::new(
        std::sync::Arc::new(crate::runtime::paths::FixedRuntimePaths::new(
            temp.path().into(),
            temp.path().into(),
        )),
        std::sync::Arc::new(crate::runtime::events::NoopRuntimeEvents),
        std::sync::Arc::new(NoSecrets),
    );
    let database = temp.path().join("items.sqlite");
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    connection
        .execute(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             INSERT INTO items VALUES (1, 'a'), (2, 'b'), (3, 'c');",
        )
        .await
        .unwrap();
    connection.close().await.unwrap();
    crate::drivers::registry::register_driver(crate::drivers::sqlite::SqliteDriver::new()).await;
    let saved: crate::models::SavedConnection = serde_json::from_value(serde_json::json!({
        "id": "items",
        "name": "Items",
        "params": {
            "driver": "sqlite",
            "database": database.to_string_lossy(),
            "read_only": read_only,
        },
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
    (temp, runtime, database)
}

pub(crate) async fn items(database: &std::path::Path) -> Vec<(i64, String)> {
    use sqlx::Connection;
    let mut connection = sqlx::SqliteConnection::connect(&database.to_string_lossy())
        .await
        .unwrap();
    sqlx::query_as("SELECT id, name FROM items ORDER BY id")
        .fetch_all(&mut connection)
        .await
        .unwrap()
}

fn expected(rows: &[(i64, &str)]) -> Vec<(i64, String)> {
    rows.iter()
        .map(|(id, name)| (*id, name.to_string()))
        .collect()
}

#[tokio::test]
async fn editing_a_cell_and_saving_updates_exactly_that_row_in_a_sqlite_table_with_a_primary_key_and_the_other_rows_are_unchanged(
) {
    let (_temp, runtime, database) = items_fixture(false).await;
    let key = std::collections::HashMap::from([("id".to_string(), serde_json::json!(2))]);

    let affected = super::update_record(
        &runtime,
        None,
        "items",
        "items".into(),
        key,
        "name".into(),
        serde_json::json!("B"),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(affected, 1);
    assert_eq!(
        items(&database).await,
        expected(&[(1, "a"), (2, "B"), (3, "c")])
    );
}

#[tokio::test]
async fn on_a_read_only_connection_saving_a_pending_change_is_refused_by_the_backend_and_nothing_is_written(
) {
    use serde_json::json;
    use std::collections::HashMap;
    let (_temp, runtime, database) = items_fixture(true).await;
    let key = || HashMap::from([("id".to_string(), json!(2))]);
    let blocked = Err(crate::application::read_only::WRITE_BLOCKED.to_string());

    let update = super::update_record(
        &runtime,
        None,
        "items",
        "items".into(),
        key(),
        "name".into(),
        json!("B"),
        None,
        None,
    )
    .await;
    let delete =
        super::delete_record(&runtime, None, "items", "items".into(), key(), None, None).await;
    let data = HashMap::from([
        ("id".to_string(), json!(4)),
        ("name".to_string(), json!("d")),
    ]);
    let insert =
        super::insert_record(&runtime, None, "items", "items".into(), data, None, None).await;

    assert_eq!(
        (update, delete, insert),
        (blocked.clone(), blocked.clone(), blocked)
    );
    assert_eq!(
        items(&database).await,
        expected(&[(1, "a"), (2, "b"), (3, "c")])
    );
}
