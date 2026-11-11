use super::edits::{Edits, Op, Origin, Source};
use super::theme::{hsla, Colors};
use gpui::{
    div, prelude::FluentBuilder as _, px, App, Context, Div, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Stateful, Styled as _, Window,
};
use gpui_component::input::{Input, InputState};
use gpui_component::table::{Column, TableDelegate, TableState};
use gpui_component::Sizable as _;
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

impl From<&Cell> for Value {
    fn from(cell: &Cell) -> Self {
        match cell {
            Cell::Null => Value::Null,
            Cell::Int(int) => (*int).into(),
            Cell::Float(float) => (*float).into(),
            Cell::Bool(flag) => (*flag).into(),
            Cell::Text(text) => Value::String(text.to_string()),
            // Holds JSON text: big numbers, arrays and objects.
            Cell::Other(text) => {
                serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
            }
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
    pub edits: Edits,
    pub editor: Option<(usize, usize, Entity<InputState>)>,
    text: Hsla,
    muted: Hsla,
    modified: Hsla,
    new: Hsla,
    deleted: Hsla,
}

impl ResultGrid {
    pub fn new(colors: &Colors) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            edits: Edits::default(),
            editor: None,
            text: hsla(&colors.text.primary),
            muted: hsla(&colors.text.muted),
            modified: hsla(&colors.semantic.modified),
            new: hsla(&colors.semantic.new),
            deleted: hsla(&colors.semantic.deleted),
        }
    }

    pub fn set(&mut self, columns: Vec<String>, rows: Vec<Vec<Value>>) {
        self.columns = columns;
        self.edits = Edits::default();
        self.editor = None;
        // Dropping millions of rows would stall the UI thread.
        let old = std::mem::replace(&mut self.rows, to_cells(rows));
        std::thread::spawn(move || drop(old));
    }

    pub fn append(&mut self, rows: Vec<Vec<Value>>) {
        self.rows.extend(to_cells(rows));
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    fn original(&self, row: usize, col: usize) -> Value {
        self.rows[row].get(col).map_or(Value::Null, Value::from)
    }

    /// The pending value of a cell, if any. New rows always have one.
    fn pending(&self, row: usize, col: usize) -> Option<&Value> {
        match row.checked_sub(self.rows.len()) {
            None => self.edits.change(row, col),
            Some(index) => self.edits.inserted().get(index)?.get(col),
        }
    }

    pub fn is_deleted(&self, row: usize) -> bool {
        row < self.rows.len() && self.edits.is_deleted(row)
    }

    /// The cell text an editor starts with; NULL starts empty.
    pub fn text(&self, row: usize, col: usize) -> String {
        let value = match self.pending(row, col) {
            Some(value) => value.clone(),
            None if row < self.rows.len() => self.original(row, col),
            None => Value::Null,
        };
        match value {
            Value::Null => String::new(),
            value => format_cell(&Cell::from(value)),
        }
    }

    pub fn commit(&mut self, row: usize, col: usize, value: Value) {
        match row.checked_sub(self.rows.len()) {
            None => {
                let original = self.original(row, col);
                self.edits.edit(row, col, value, &original);
            }
            Some(index) => self.edits.edit_new(index, col, value),
        }
    }

    /// Marks an existing row for deletion or restores it; a new row is discarded.
    pub fn toggle_delete(&mut self, row: usize) {
        match row.checked_sub(self.rows.len()) {
            None => self.edits.toggle_delete(row),
            Some(index) => self.edits.discard_new(index),
        }
    }

    pub fn ops(&self, source: &Source, keys: &[usize]) -> Result<Vec<(Origin, Op)>, String> {
        self.edits.ops(source, &self.columns, keys, |row, col| {
            self.original(row, col)
        })
    }
}

impl TableDelegate for ResultGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len() + self.edits.inserted().len()
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
        if let Some((_, _, input)) = &self
            .editor
            .as_ref()
            .filter(|(row, col, _)| (*row, *col) == (row_ix, col_ix))
        {
            return Input::new(input).small().into_any_element();
        }
        let pending = self
            .pending(row_ix, col_ix)
            .map(|value| Cell::from(value.clone()));
        let modified = pending.is_some() && row_ix < self.rows.len();
        let deleted = self.is_deleted(row_ix);
        let cell = match &pending {
            Some(cell) => cell,
            None => self
                .rows
                .get(row_ix)
                .and_then(|row| row.get(col_ix))
                .unwrap_or(&Cell::Null),
        };
        div()
            .truncate()
            .when(modified, |d| d.bg(self.modified.alpha(0.3)))
            .when(deleted, |d| d.line_through())
            .text_color(if deleted {
                self.deleted
            } else if matches!(cell, Cell::Null) {
                self.muted
            } else {
                self.text
            })
            .child(format_cell(cell))
            .into_any_element()
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let row = div().id(("row", row_ix));
        if row_ix >= self.rows.len() {
            row.bg(self.new.alpha(0.15))
        } else if self.edits.is_deleted(row_ix) {
            row.bg(self.deleted.alpha(0.1))
        } else {
            row
        }
    }
}
