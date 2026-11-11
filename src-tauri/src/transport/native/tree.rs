use crate::application::explorer::{Entry, EntryKind, Location};

pub enum Load {
    Idle,
    Loading,
    Failed(String),
    Loaded(Vec<Child>),
}

pub enum Child {
    Node(Node),
    Column { name: String, data_type: String },
}

pub struct Node {
    pub entry: Entry,
    // Where the children are listed; for tables and views, where the table lives.
    pub location: Location,
    pub expanded: bool,
    pub load: Load,
}

impl Node {
    pub fn new(parent: &Location, entry: Entry) -> Self {
        let mut location = parent.clone();
        match entry.kind {
            EntryKind::Database => location.database = Some(entry.name.clone()),
            EntryKind::Schema => location.schema = Some(entry.name.clone()),
            EntryKind::Table | EntryKind::View => {}
        }
        Self {
            entry,
            location,
            expanded: false,
            load: Load::Idle,
        }
    }
}

pub enum RowKind {
    Node {
        kind: EntryKind,
        name: String,
        expanded: bool,
    },
    Column {
        name: String,
        data_type: String,
    },
    Loading,
    Failed(String),
}

pub struct Row {
    pub path: Vec<usize>,
    pub depth: usize,
    pub kind: RowKind,
}

#[derive(Default)]
pub struct Tree {
    pub root: Option<Load>,
}

impl Tree {
    pub fn load_mut(&mut self, path: &[usize]) -> Option<&mut Load> {
        let mut load = self.root.as_mut()?;
        for &index in path {
            let Load::Loaded(children) = load else {
                return None;
            };
            let Child::Node(node) = children.get_mut(index)? else {
                return None;
            };
            load = &mut node.load;
        }
        Some(load)
    }

    pub fn node_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        let (&last, parent) = path.split_last()?;
        let Some(Load::Loaded(children)) = self.load_mut(parent) else {
            return None;
        };
        match children.get_mut(last)? {
            Child::Node(node) => Some(node),
            Child::Column { .. } => None,
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(root) = &self.root {
            push_load(root, &mut Vec::new(), 0, &mut rows);
        }
        rows
    }
}

fn push_load(load: &Load, path: &mut Vec<usize>, depth: usize, rows: &mut Vec<Row>) {
    let row = |path: &[usize], kind| Row {
        path: path.to_vec(),
        depth,
        kind,
    };
    match load {
        Load::Idle => {}
        Load::Loading => rows.push(row(path, RowKind::Loading)),
        Load::Failed(error) => rows.push(row(path, RowKind::Failed(error.clone()))),
        Load::Loaded(children) => {
            for (index, child) in children.iter().enumerate() {
                path.push(index);
                match child {
                    Child::Column { name, data_type } => rows.push(row(
                        path,
                        RowKind::Column {
                            name: name.clone(),
                            data_type: data_type.clone(),
                        },
                    )),
                    Child::Node(node) => {
                        rows.push(row(
                            path,
                            RowKind::Node {
                                kind: node.entry.kind,
                                name: node.entry.name.clone(),
                                expanded: node.expanded,
                            },
                        ));
                        if node.expanded {
                            push_load(&node.load, path, depth + 1, rows);
                        }
                    }
                }
                path.pop();
            }
        }
    }
}
