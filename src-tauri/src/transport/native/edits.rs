use super::i18n::t;
use crate::application::explorer::Location;
use crate::application::records;
use crate::models::TableColumn;
use crate::runtime::RuntimeContext;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The table a tab was opened from and the query that loaded it.
pub struct Source {
    pub connection_id: String,
    pub location: Location,
    pub table: String,
    pub sql: String,
    pub columns: Vec<TableColumn>,
}

impl Source {
    /// Result indexes of the primary key, or `None` when the result did not come from `sql`.
    pub fn keys(&self, sql: &str, result: &[String]) -> Option<Vec<usize>> {
        if sql != self.sql {
            return None;
        }
        self.columns
            .iter()
            .filter(|column| column.is_pk)
            .map(|column| result.iter().position(|name| *name == column.name))
            .collect::<Option<Vec<_>>>()
            .filter(|keys| !keys.is_empty())
    }

    fn column(&self, name: &str) -> Option<&TableColumn> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn nullable(&self, name: &str) -> bool {
        self.column(name).is_some_and(|column| column.is_nullable)
    }

    pub fn new_row(&self, result: &[String]) -> Vec<Value> {
        result
            .iter()
            .map(|name| match self.column(name) {
                Some(c) if !(c.is_generated || c.is_auto_increment || c.is_nullable) => {
                    Value::String(String::new())
                }
                _ => Value::Null,
            })
            .collect()
    }
}

pub enum Op {
    Delete(HashMap<String, Value>),
    Update(HashMap<String, Value>, String, Value),
    Insert(HashMap<String, Value>),
}

/// Where a write came from, so a written change can leave the pending edits.
#[derive(Clone, Copy)]
pub enum Origin {
    Deleted(usize),
    Changed(usize, usize),
    Inserted(usize),
}

pub struct SaveError {
    pub applied: Vec<Origin>,
    pub message: String,
}

/// Pending changes of one result, addressed by result row and column index.
#[derive(Default)]
pub struct Edits {
    changes: BTreeMap<(usize, usize), Value>,
    deleted: BTreeSet<usize>,
    inserted: Vec<Vec<Value>>,
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn count(&self) -> usize {
        let rows: BTreeSet<_> = self.changes.keys().map(|(row, _)| row).collect();
        rows.len() + self.deleted.len() + self.inserted.len()
    }

    pub fn change(&self, row: usize, col: usize) -> Option<&Value> {
        self.changes.get(&(row, col))
    }

    pub fn is_deleted(&self, row: usize) -> bool {
        self.deleted.contains(&row)
    }

    pub fn inserted(&self) -> &[Vec<Value>] {
        &self.inserted
    }

    /// Stages `value` for an existing row; writing the stored value back clears the change.
    pub fn edit(&mut self, row: usize, col: usize, value: Value, original: &Value) {
        let same = match (&value, original) {
            (Value::Null, _) | (_, Value::Null) => value == *original,
            _ => text(&value) == text(original),
        };
        if same {
            self.changes.remove(&(row, col));
        } else {
            self.changes.insert((row, col), value);
        }
    }

    pub fn edit_new(&mut self, index: usize, col: usize, value: Value) {
        if let Some(cell) = self
            .inserted
            .get_mut(index)
            .and_then(|row| row.get_mut(col))
        {
            *cell = value;
        }
    }

    pub fn toggle_delete(&mut self, row: usize) {
        if !self.deleted.remove(&row) {
            self.deleted.insert(row);
        }
    }

    pub fn insert(&mut self, values: Vec<Value>) {
        self.inserted.push(values);
    }

    pub fn discard_new(&mut self, index: usize) {
        if index < self.inserted.len() {
            self.inserted.remove(index);
        }
    }

    /// Drops written changes; a deleted row's other changes go with it.
    pub fn forget(&mut self, applied: &[Origin]) {
        let mut inserted = Vec::new();
        for origin in applied {
            match *origin {
                Origin::Deleted(row) => {
                    self.deleted.remove(&row);
                    self.changes.retain(|&(r, _), _| r != row);
                }
                Origin::Changed(row, col) => {
                    self.changes.remove(&(row, col));
                }
                Origin::Inserted(index) => inserted.push(index),
            }
        }
        inserted.sort_unstable_by(|a, b| b.cmp(a));
        for index in inserted {
            self.discard_new(index);
        }
    }

    /// Deletes, then updates, then inserts. A new row missing a required value fails before any write.
    pub fn ops(
        &self,
        source: &Source,
        result: &[String],
        keys: &[usize],
        original: impl Fn(usize, usize) -> Value,
    ) -> Result<Vec<(Origin, Op)>, String> {
        let key = |row| {
            keys.iter()
                .map(|&col| (result[col].clone(), original(row, col)))
                .collect::<HashMap<_, _>>()
        };
        let mut ops: Vec<(Origin, Op)> = self
            .deleted
            .iter()
            .map(|&row| (Origin::Deleted(row), Op::Delete(key(row))))
            .collect();
        for (&(row, col), value) in &self.changes {
            if !self.is_deleted(row) {
                ops.push((
                    Origin::Changed(row, col),
                    Op::Update(key(row), result[col].clone(), value.clone()),
                ));
            }
        }
        for (index, row) in self.inserted.iter().enumerate() {
            let mut data = HashMap::new();
            for (name, value) in result.iter().zip(row) {
                let empty = value.is_null() || value.as_str() == Some("");
                match source.column(name) {
                    Some(c) if c.is_generated => continue,
                    Some(c)
                        if empty
                            && (c.is_auto_increment
                                || c.default_value.as_deref().is_some_and(|d| !d.is_empty())) =>
                    {
                        continue
                    }
                    Some(c) if empty && !c.is_nullable => {
                        return Err(format!("{name}: {}", t("newRow.required")))
                    }
                    _ => data.insert(name.clone(), value.clone()),
                };
            }
            ops.push((Origin::Inserted(index), Op::Insert(data)));
        }
        Ok(ops)
    }
}

/// Writes through the parameterised record functions, which refuse read-only connections.
/// Stops at the first failure and reports the writes that already happened.
pub async fn save(
    context: &RuntimeContext,
    source: &Source,
    ops: Vec<(Origin, Op)>,
) -> Result<(), SaveError> {
    let (id, schema, database) = (
        source.connection_id.as_str(),
        &source.location.schema,
        &source.location.database,
    );
    let mut applied = Vec::new();
    for (origin, op) in ops {
        let table = source.table.clone();
        match op {
            Op::Delete(key) => {
                records::delete_record(
                    context,
                    None,
                    id,
                    table,
                    key,
                    schema.clone(),
                    database.clone(),
                )
                .await
            }
            Op::Update(key, column, value) => {
                records::update_record(
                    context,
                    None,
                    id,
                    table,
                    key,
                    column,
                    value,
                    schema.clone(),
                    database.clone(),
                )
                .await
            }
            Op::Insert(data) => {
                records::insert_record(
                    context,
                    None,
                    id,
                    table,
                    data,
                    schema.clone(),
                    database.clone(),
                )
                .await
            }
        }
        .map_err(|message| SaveError {
            applied: applied.clone(),
            message,
        })?;
        applied.push(origin);
    }
    Ok(())
}
