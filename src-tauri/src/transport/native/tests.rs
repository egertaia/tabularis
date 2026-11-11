use super::bench::percentile;
use super::grid::{Cell, format_cell};
use super::i18n::t;
use super::shell::sql_to_run;
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
    use serde_json::{Value, json};
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
