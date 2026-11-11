use super::bench::percentile;
use super::grid::{format_cell, Cell};
use super::i18n::t;
use super::shell::sql_to_run;
use super::tabs::Tabs;
use super::theme::colors_from_source;

#[test]
fn translates_existing_key_and_ignores_unknown() {
    assert_eq!(t("sidebar.connections"), "Connections");
    assert_eq!(t("no.such.key"), "");
}

#[test]
fn reads_colors_from_legacy_theme_source() {
    let source = include_str!("../../../../packages/web-ui/src/themes/builtin-themes.json");
    let themes: Vec<serde_json::Value> = serde_json::from_str(source).unwrap();
    let dark = themes.iter().find(|t| t["id"] == "tabularis-dark").unwrap();
    let colors = colors_from_source(&dark.to_string()).unwrap();
    assert_eq!(colors.bg.base, "#020617");
    assert!(colors_from_source("{}").is_none());
}

#[test]
fn a_json_value_converted_to_a_cell_displays_the_same_text_as_before() {
    use serde_json::{json, Value};
    let big: Value = serde_json::from_str("18446744073709551615").unwrap();
    for (value, text) in [
        (json!(null), "NULL"),
        (json!(42), "42"),
        (json!(1.5), "1.5"),
        (json!(2.0), "2.0"),
        (big, "18446744073709551615"),
        (json!(true), "true"),
        (json!("a\"b"), "a\"b"),
        (json!({"k": [1, true]}), r#"{"k":[1,true]}"#),
    ] {
        assert_eq!(format_cell(&Cell::from(value)), text);
    }
}

#[test]
fn nearest_rank_percentiles_of_known_slice() {
    let sorted: Vec<f64> = (1..=100).map(f64::from).collect();
    assert_eq!(percentile(&sorted, 50.), 50.);
    assert_eq!(percentile(&sorted, 99.), 99.);
    assert_eq!(percentile(&[7.], 99.), 7.);
}

#[test]
fn runs_selection_else_whole_buffer() {
    assert_eq!(sql_to_run("select 2", "select 1; select 2"), "select 2");
    assert_eq!(sql_to_run("", "select 1"), "select 1");
    assert_eq!(sql_to_run("  \n", "select 1"), "select 1");
}

#[test]
fn opening_a_new_tab_and_closing_it_returns_to_the_previous_tab_and_closing_the_last_tab_leaves_one_empty_tab(
) {
    let mut tabs = Tabs::new("first");
    let first = tabs.active_id();
    let second = tabs.open(None, None, "second");
    assert_eq!(tabs.active_id(), second);
    assert_eq!(tabs.items()[1].title, "Console 2");

    tabs.close(second, || "fresh");
    assert_eq!(tabs.active_id(), first);

    tabs.close(first, || "fresh");
    assert_eq!(tabs.items().len(), 1);
    assert_eq!(tabs.active_mut().view, "fresh");
    assert_eq!(tabs.active_mut().title, "Console");
}

#[test]
fn a_query_running_in_one_tab_keeps_running_when_another_tab_is_selected_and_closing_its_tab_cancels_it(
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let task = runtime.spawn(std::future::pending::<()>());
    let mut tabs = Tabs::new(());
    let running = tabs.active_id();
    tabs.active_mut().stream.start(task.abort_handle());
    let other = tabs.open(None, None, ());

    tabs.select(running);
    tabs.select(other);
    assert!(tabs.get_mut(running).unwrap().stream.is_running());
    assert!(!task.is_finished());

    tabs.close(running, || ());
    assert!(runtime.block_on(task).unwrap_err().is_cancelled());
}

fn items_source() -> super::edits::Source {
    let column = |name: &str, is_pk: bool| -> crate::models::TableColumn {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "data_type": "TEXT",
            "is_pk": is_pk,
            "is_nullable": !is_pk,
            "is_auto_increment": false,
        }))
        .unwrap()
    };
    super::edits::Source {
        connection_id: "missing".into(),
        location: Default::default(),
        table: "items".into(),
        sql: "SELECT * FROM items".into(),
        columns: vec![column("id", true), column("name", false)],
    }
}

#[test]
fn a_result_that_is_not_editable_offers_no_editing() {
    let source = items_source();
    let result = ["id".to_string(), "name".to_string()];
    assert_eq!(source.keys("SELECT * FROM items", &result), Some(vec![0]));
    assert_eq!(source.keys("SELECT name FROM items", &result), None);
    assert_eq!(source.keys("SELECT * FROM items", &result[1..]), None);
    let keyless = super::edits::Source {
        columns: source.columns.into_iter().skip(1).collect(),
        ..items_source()
    };
    assert_eq!(keyless.keys("SELECT * FROM items", &result), None);
}

#[tokio::test]
async fn a_failed_save_keeps_only_the_changes_that_were_not_written_so_saving_again_does_not_write_a_row_twice(
) {
    use crate::application::records::tests::{items, items_fixture};
    use serde_json::{json, Value};
    let (_temp, context, database) = items_fixture(false).await;
    let source = super::edits::Source {
        connection_id: "items".into(),
        ..items_source()
    };
    let result = ["id".to_string(), "name".to_string()];
    let mut edits = super::edits::Edits::default();
    edits.insert(vec![json!(4), json!("d")]);
    edits.insert(vec![json!(5), Value::Null]);
    let original = |_, _| Value::Null;

    let ops = edits.ops(&source, &result, &[0], original).unwrap();
    let error = super::edits::save(&context, &source, ops)
        .await
        .err()
        .unwrap();
    edits.forget(&error.applied);

    assert_eq!(items(&database).await.len(), 4);
    assert_eq!(edits.count(), 1);
    assert_eq!(edits.inserted(), [vec![json!(5), Value::Null]]);

    edits.edit_new(0, 1, json!("e"));
    let ops = edits.ops(&source, &result, &[0], original).unwrap();
    assert!(super::edits::save(&context, &source, ops).await.is_ok());

    let names: Vec<_> = items(&database)
        .await
        .into_iter()
        .map(|row| row.1)
        .collect();
    assert_eq!(names, ["a", "b", "c", "d", "e"]);
}

#[derive(Default)]
struct MemorySecrets(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl crate::runtime::secrets::RuntimeSecrets for MemorySecrets {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}

type ConnectionFixture = (
    tempfile::TempDir,
    crate::runtime::RuntimeContext,
    std::sync::Arc<crate::runtime::state::ApplicationState>,
    std::sync::Arc<MemorySecrets>,
);

fn connection_fixture() -> ConnectionFixture {
    let temp = tempfile::tempdir().unwrap();
    let secrets = std::sync::Arc::new(MemorySecrets::default());
    let context = crate::runtime::RuntimeContext::new(
        std::sync::Arc::new(crate::runtime::paths::FixedRuntimePaths::new(
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
        )),
        std::sync::Arc::new(crate::runtime::events::NoopRuntimeEvents),
        secrets.clone(),
    );
    (temp, context, Default::default(), secrets)
}

fn listed(context: &crate::runtime::RuntimeContext) -> Vec<crate::models::SavedConnection> {
    super::backend::load_redacted_connections(&context.paths.connections_file()).unwrap()
}

fn sqlite_values(name: &str, path: &str) -> super::connection_form::Values {
    super::connection_form::Values {
        name: name.into(),
        database: path.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn saving_a_new_sqlite_connection_from_the_form_stores_it_and_it_appears_in_the_connection_list(
) {
    use crate::drivers::driver_trait::DatabaseDriver as _;
    let (temp, context, state, _) = connection_fixture();
    let sqlite = crate::drivers::sqlite::SqliteDriver::new()
        .manifest()
        .clone();
    let path = temp.path().join("app.db").to_string_lossy().into_owned();
    let values = sqlite_values("Local", &path);
    assert_eq!(
        super::connection_form::save_error(&sqlite, None, &values),
        None
    );

    let params = super::connection_form::params(&sqlite, None, &values);
    let saved = super::backend::save_connection(&context, &state, values.name, params)
        .await
        .unwrap();

    let connections = listed(&context);
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0].id, saved.id);
    assert_eq!(connections[0].name, "Local");
    assert_eq!(connections[0].params.driver, "sqlite");
    assert_eq!(connections[0].params.database.primary(), path);
}

#[tokio::test]
async fn editing_a_connection_without_typing_a_password_keeps_the_stored_password() {
    use crate::drivers::driver_trait::DatabaseDriver as _;
    use crate::runtime::secrets::RuntimeSecrets as _;
    let (_temp, context, state, secrets) = connection_fixture();
    let postgres = crate::drivers::postgres::PostgresDriver::new()
        .manifest()
        .clone();
    let typed = super::connection_form::Values {
        name: "Server".into(),
        host: "db.internal".into(),
        port: "5432".into(),
        username: "app".into(),
        password: "stored-secret".into(),
        database: "sales".into(),
        read_only: false,
    };
    let params = super::connection_form::params(&postgres, None, &typed);
    let saved = super::backend::save_connection(&context, &state, typed.name.clone(), params)
        .await
        .unwrap();
    let account = format!("{}:db", saved.id);
    assert_eq!(
        secrets.get(&account).unwrap().as_deref(),
        Some("stored-secret")
    );

    let editing = listed(&context).remove(0);
    let edited = super::connection_form::Values {
        name: "Renamed".into(),
        password: String::new(),
        ..typed
    };
    let params = super::connection_form::params(&postgres, Some(&editing.params), &edited);
    assert_eq!(params.password, None);
    super::backend::update_connection(&context, &state, &editing, edited.name, params)
        .await
        .unwrap();

    assert_eq!(
        secrets.get(&account).unwrap().as_deref(),
        Some("stored-secret")
    );
    let persisted = crate::persistence::load_connections_file(&context.paths.connections_file())
        .unwrap()
        .connections
        .remove(0);
    assert_eq!(persisted.name, "Renamed");
    assert_eq!(persisted.params.password, None);
    assert_eq!(persisted.params.save_in_keychain, Some(true));
}

#[tokio::test]
async fn deleting_a_connection_removes_it_from_the_list() {
    use crate::drivers::driver_trait::DatabaseDriver as _;
    let (temp, context, state, _) = connection_fixture();
    let sqlite = crate::drivers::sqlite::SqliteDriver::new()
        .manifest()
        .clone();
    let path = temp.path().join("app.db").to_string_lossy().into_owned();
    let values = sqlite_values("Doomed", &path);
    let params = super::connection_form::params(&sqlite, None, &values);
    let saved = super::backend::save_connection(&context, &state, values.name, params)
        .await
        .unwrap();

    super::backend::disconnect_connection(&context, &state, &saved.id)
        .await
        .unwrap();
    super::backend::delete_connection(&context, &state, saved.id)
        .await
        .unwrap();

    assert!(listed(&context).is_empty());
}
