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
