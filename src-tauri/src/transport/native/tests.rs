use super::grid::format_cell;
use super::i18n::t;
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
fn formats_cells_as_text() {
    use serde_json::json;
    assert_eq!(format_cell(&json!(null)), "NULL");
    assert_eq!(format_cell(&json!("a\"b")), "a\"b");
    assert_eq!(format_cell(&json!(1.5)), "1.5");
    assert_eq!(format_cell(&json!({"k": [1, true]})), r#"{"k":[1,true]}"#);
}
