use gpui::{App, Context, Hsla, IntoElement, ParentElement as _, Styled as _, Window, div, px};
use gpui_component::table::{Column, TableDelegate, TableState};
use serde_json::Value;

pub enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(Box<str>),
    Other(Box<str>),
}

impl From<Value> for Cell {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Cell::Null,
            Value::Bool(flag) => Cell::Bool(flag),
            Value::Number(number) => match number.as_i64() {
                Some(int) => Cell::Int(int),
                None if number.is_f64() => Cell::Float(number.as_f64().unwrap_or_default()),
                None => Cell::Other(number.to_string().into()),
            },
            Value::String(text) => Cell::Text(text.into_boxed_str()),
            other => Cell::Other(other.to_string().into()),
        }
    }
}

pub fn to_cells(rows: Vec<Vec<Value>>) -> Vec<Box<[Cell]>> {
    rows.into_iter()
        .map(|row| row.into_iter().map(Cell::from).collect())
        .collect()
}

pub fn format_cell(cell: &Cell) -> String {
    match cell {
        Cell::Null => "NULL".into(),
        Cell::Int(int) => int.to_string(),
        Cell::Float(float) => serde_json::Number::from_f64(*float)
            .map_or_else(|| float.to_string(), |number| number.to_string()),
        Cell::Bool(flag) => flag.to_string(),
        Cell::Text(text) | Cell::Other(text) => text.to_string(),
    }
}

pub struct ResultGrid {
    columns: Vec<String>,
    rows: Vec<Box<[Cell]>>,
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
        // Dropping millions of rows would stall the UI thread.
        let old = std::mem::replace(&mut self.rows, to_cells(rows));
        std::thread::spawn(move || drop(old));
    }

    pub fn append(&mut self, rows: Vec<Vec<Value>>) {
        self.rows.extend(to_cells(rows));
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
        let cell = self.rows[row_ix].get(col_ix).unwrap_or(&Cell::Null);
        div()
            .truncate()
            .text_color(if matches!(cell, Cell::Null) {
                self.muted
            } else {
                self.text
            })
            .child(format_cell(cell))
    }
}
