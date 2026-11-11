use gpui::{App, Context, Hsla, IntoElement, ParentElement as _, Styled as _, Window, div, px};
use gpui_component::table::{Column, TableDelegate, TableState};
use serde_json::Value;

pub fn format_cell(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub struct ResultGrid {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    text: Hsla,
    muted: Hsla,
}

impl ResultGrid {
    pub fn new(text: Hsla, muted: Hsla) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            text,
            muted,
        }
    }

    pub fn set(&mut self, columns: Vec<String>, rows: Vec<Vec<Value>>) {
        self.columns = columns;
        self.rows = rows;
    }

    pub fn append(&mut self, rows: Vec<Vec<Value>>) {
        self.rows.extend(rows);
    }
}

impl TableDelegate for ResultGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        Column::new(col_ix.to_string(), self.columns[col_ix].clone()).width(px(160.))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let value = self.rows[row_ix].get(col_ix).unwrap_or(&Value::Null);
        div()
            .truncate()
            .text_color(if value.is_null() {
                self.muted
            } else {
                self.text
            })
            .child(format_cell(value))
    }
}
