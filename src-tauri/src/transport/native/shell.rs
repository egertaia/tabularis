use super::bench::{self, Bench};
use super::grid::ResultGrid;
use super::i18n::t;
use super::theme::hsla;
use super::theme::Colors;
use super::tree::{Child, Load, Node, RowKind, Tree};
use crate::application::explorer::{self, Entry, EntryKind, Location};
use crate::application::queries::{self, QueryRequestScope};
use crate::commands::QueryCancellationState;
use crate::drivers::driver_trait::QueryChunk;
use crate::models::SavedConnection;
use crate::runtime::RuntimeContext;
use gpui::{
    actions, div, px, AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Entity,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Editor, EditorState};
use gpui_component::table::{DataTable, TableState};
use gpui_component::Disableable as _;
use std::collections::HashSet;
use std::sync::Arc;

actions!(native, [RunQuery]);

pub fn bind_keys(cx: &mut App) {
    // The input binds the same keys to Enter, so this must come after gpui_component::init.
    let key = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    cx.bind_keys([KeyBinding::new(key, RunQuery, Some("Input"))]);
}

pub(super) fn sql_to_run(selected: &str, whole: &str) -> String {
    let sql = if selected.trim().is_empty() {
        whole
    } else {
        selected
    };
    sql.to_string()
}

enum Job {
    List(Location),
    Columns(Location, Entry),
}

async fn fetch(context: RuntimeContext, id: String, job: Job) -> Result<Vec<Child>, String> {
    match job {
        Job::List(location) => Ok(explorer::list(&context, &id, &location)
            .await?
            .into_iter()
            .map(|entry| Child::Node(Node::new(&location, entry)))
            .collect()),
        Job::Columns(location, entry) => Ok(explorer::columns(&context, &id, &location, &entry)
            .await?
            .into_iter()
            .map(|column| Child::Column {
                name: column.name,
                data_type: column.data_type,
            })
            .collect()),
    }
}

pub struct Shell {
    colors: Colors,
    context: RuntimeContext,
    runtime: tokio::runtime::Handle,
    cancellation: Arc<QueryCancellationState>,
    connections: Result<Vec<SavedConnection>, String>,
    selected: Option<String>,
    tree: Tree,
    // Bumped on every selection so late metadata replies are ignored.
    tree_id: u64,
    // Schema and database of the last opened table, used by the next run.
    scope: Location,
    connected: HashSet<String>,
    input: Entity<EditorState>,
    table: Entity<TableState<ResultGrid>>,
    status: String,
    error: Option<String>,
    // Bumped when a run is dropped so its late chunks and result are ignored.
    run_id: u64,
    in_flight: Option<tokio::task::AbortHandle>,
    rows: usize,
    // Result sets seen in the current run; only the first is shown.
    sets: usize,
    bench: Option<Bench>,
}

impl Shell {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        colors: Colors,
        context: RuntimeContext,
        runtime: tokio::runtime::Handle,
        cancellation: Arc<QueryCancellationState>,
        connections: Result<Vec<SavedConnection>, String>,
    ) -> Self {
        let grid = ResultGrid::new(hsla(&colors.text.primary), hsla(&colors.text.muted));
        Self {
            input: cx.new(|cx| EditorState::new(window, cx).language("sql")),
            table: cx.new(|cx| TableState::new(grid, window, cx).col_movable(false)),
            colors,
            context,
            runtime,
            cancellation,
            connections,
            selected: None,
            tree: Tree::default(),
            tree_id: 0,
            scope: Location::default(),
            connected: HashSet::new(),
            status: String::new(),
            error: None,
            run_id: 0,
            in_flight: None,
            rows: 0,
            sets: 0,
            bench: None,
        }
    }

    pub fn start_bench(&mut self, bench: Bench, window: &mut Window, cx: &mut Context<Self>) {
        self.select(bench.connection.clone(), cx);
        self.input.update(cx, |input, cx| {
            input.set_value(bench.sql.clone(), window, cx)
        });
        self.bench = Some(Bench {
            start: std::time::Instant::now(),
            ..bench
        });
        self.run(&RunQuery, window, cx);
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.tree = Tree::default();
        self.tree_id += 1;
        self.scope = Location::default();
        self.stop();
        self.error = None;
        self.status.clear();
        self.show_columns(Vec::new(), cx);
        cx.notify();
    }

    fn load_root(&mut self, cx: &mut Context<Self>) {
        self.tree.root = Some(Load::Idle);
        self.load(Vec::new(), cx);
    }

    fn load(&mut self, path: Vec<usize>, cx: &mut Context<Self>) {
        let Some(connection_id) = self.selected.clone() else {
            return;
        };
        let job = match self.tree.node_mut(&path) {
            None => Job::List(Location::default()),
            Some(node) => match node.entry.kind {
                EntryKind::Table | EntryKind::View => {
                    Job::Columns(node.location.clone(), node.entry.clone())
                }
                _ => Job::List(node.location.clone()),
            },
        };
        let Some(load) = self.tree.load_mut(&path) else {
            return;
        };
        *load = Load::Loading;
        let (context, tree_id) = (self.context.clone(), self.tree_id);
        let task = self
            .runtime
            .spawn(async move { fetch(context, connection_id, job).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update(cx, |this, cx| {
                if this.tree_id != tree_id {
                    return;
                }
                if let Some(load) = this.tree.load_mut(&path) {
                    *load = match result {
                        Ok(children) => Load::Loaded(children),
                        Err(error) => Load::Failed(error),
                    };
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn toggle(&mut self, path: Vec<usize>, cx: &mut Context<Self>) {
        let Some(node) = self.tree.node_mut(&path) else {
            return;
        };
        node.expanded = !node.expanded;
        if node.expanded && matches!(node.load, Load::Idle | Load::Failed(_)) {
            self.load(path, cx);
        }
        cx.notify();
    }

    fn open_table(&mut self, path: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection_id) = self.selected.clone() else {
            return;
        };
        let Some(node) = self.tree.node_mut(&path) else {
            return;
        };
        let (location, name) = (node.location.clone(), node.entry.name.clone());
        let (context, tree_id) = (self.context.clone(), self.tree_id);
        let task = {
            let location = location.clone();
            self.runtime.spawn(async move {
                explorer::open_table_query(&context, &connection_id, &location, &name).await
            })
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update_in(cx, |this, window, cx| {
                if this.tree_id != tree_id {
                    return;
                }
                match result {
                    Ok(sql) => {
                        this.scope = location;
                        this.input
                            .update(cx, |input, cx| input.set_value(sql, window, cx));
                        this.run(&RunQuery, window, cx);
                    }
                    Err(error) => {
                        this.error = Some(error);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn tree_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = &self.colors;
        self.tree
            .rows()
            .into_iter()
            .map(|row| {
                let indent = px(8. + 14. * (row.depth + 1) as f32);
                let base = div().flex().gap_1().pl(indent).text_sm();
                let key = format!("{:?}", row.path);
                match row.kind {
                    RowKind::Loading => base
                        .text_color(hsla(&c.text.muted))
                        .child(t("common.loading"))
                        .into_any_element(),
                    RowKind::Failed(error) => base.child(error).into_any_element(),
                    RowKind::Column { name, data_type } => base
                        .child(name)
                        .child(div().text_color(hsla(&c.text.muted)).child(data_type))
                        .into_any_element(),
                    RowKind::Node {
                        kind,
                        name,
                        expanded,
                    } => {
                        let (toggle_path, label_path) = (row.path.clone(), row.path);
                        let opens = matches!(kind, EntryKind::Table | EntryKind::View);
                        base.child(
                            div()
                                .id(ElementId::from(format!("toggle{key}")))
                                .w(px(12.))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.toggle(toggle_path.clone(), cx);
                                }))
                                .child(if expanded { "\u{25be}" } else { "\u{25b8}" }),
                        )
                        .child(
                            div()
                                .id(ElementId::from(format!("label{key}")))
                                .truncate()
                                .cursor_pointer()
                                .hover(|d| d.bg(hsla(&c.bg.elevated)))
                                .on_click(cx.listener(
                                    move |this, event: &ClickEvent, window, cx| {
                                        if !opens {
                                            this.toggle(label_path.clone(), cx);
                                        } else if event.click_count() >= 2 {
                                            this.open_table(label_path.clone(), window, cx);
                                        }
                                    },
                                ))
                                .child(name),
                        )
                        .into_any_element()
                    }
                }
            })
            .collect()
    }

    fn stop(&mut self) {
        self.run_id += 1;
        if let Some(task) = self.in_flight.take() {
            task.abort();
        }
    }

    fn show_columns(&mut self, columns: Vec<String>, cx: &mut Context<Self>) {
        self.rows = 0;
        self.table.update(cx, |table, cx| {
            table.delegate_mut().set(columns, Vec::new());
            table.refresh(cx);
            cx.notify();
        });
    }

    fn show_chunk(&mut self, chunk: QueryChunk, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(chunk, QueryChunk::Columns(_)) {
            self.sets += 1;
        }
        if self.sets > 1 {
            return;
        }
        match chunk {
            QueryChunk::Columns(columns) => self.show_columns(columns, cx),
            QueryChunk::Rows(rows) => {
                if self.rows == 0 {
                    if let Some(bench) = &self.bench {
                        bench::first_batch(bench, window);
                    }
                }
                self.rows += rows.len();
                self.status =
                    t("editor.rowsRetrieved").replace("{{count}}", &self.rows.to_string());
                self.table.update(cx, |table, cx| {
                    table.delegate_mut().append(rows);
                    cx.notify();
                });
            }
        }
        cx.notify();
    }

    fn run(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection_id) = self.selected.clone() else {
            self.status = t("sidebar.selectConnection").into();
            cx.notify();
            return;
        };
        self.stop();
        let input = self.input.read(cx);
        let sql = sql_to_run(&input.selected_value(), &input.value());
        let key = if self.connected.contains(&connection_id) {
            "editor.executingQuery"
        } else {
            "connections.connecting"
        };
        self.status = t(key).into();
        self.error = None;
        self.sets = 0;
        let (context, cancellation, run_id) =
            (self.context.clone(), self.cancellation.clone(), self.run_id);
        let id = connection_id.clone();
        let scope = self.scope.clone();
        let (sender, mut chunks) = tokio::sync::mpsc::unbounded_channel();
        let task = self.runtime.spawn(async move {
            queries::stream_query(
                &context,
                &cancellation,
                QueryRequestScope::DESKTOP,
                id,
                sql,
                scope.schema,
                None,
                scope.database,
                move |chunk| {
                    let _ = sender.send(chunk);
                },
            )
            .await
        });
        self.in_flight = Some(task.abort_handle());
        cx.spawn_in(window, async move |this, cx| {
            while let Some(chunk) = chunks.recv().await {
                let current = this.update_in(cx, |this, window, cx| {
                    let current = this.run_id == run_id;
                    if current {
                        this.show_chunk(chunk, window, cx);
                    }
                    current
                });
                if !matches!(current, Ok(true)) {
                    return;
                }
            }
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update_in(cx, |this, window, cx| {
                if this.run_id != run_id {
                    return;
                }
                this.in_flight = None;
                match result {
                    Ok(summary) => {
                        this.connected.insert(connection_id);
                        this.status = format!(
                            "{} \u{b7} {} ms",
                            t("editor.rowsRetrieved").replace("{{count}}", &this.rows.to_string()),
                            summary.fetch_time_ms.unwrap_or_default().round()
                        );
                        if let Some(bench) = this.bench.take() {
                            bench::loaded(bench, this.rows, this.table.clone(), window);
                        }
                    }
                    Err(error) => {
                        if this.bench.is_some() {
                            bench::fail(&error);
                        }
                        this.status.clear();
                        this.error = Some(error);
                        this.show_columns(Vec::new(), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.colors;
        let list = match &self.connections {
            Err(error) => div().child(error.clone()),
            Ok(connections) if connections.is_empty() => {
                div().child(t("connections.noConnectionsHint"))
            }
            Ok(connections) => {
                let mut items = Vec::new();
                for conn in connections {
                    let selected = self.selected.as_deref() == Some(&conn.id);
                    let id = conn.id.clone();
                    items.push(
                        div()
                            .id(ElementId::from(conn.id.clone()))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(if selected {
                                hsla(&c.border.focus)
                            } else {
                                gpui::transparent_black()
                            })
                            .bg(if selected {
                                hsla(&c.bg.elevated)
                            } else {
                                gpui::transparent_black()
                            })
                            .hover(|d| d.bg(hsla(&c.bg.elevated)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.select(id.clone(), cx);
                                this.load_root(cx);
                            }))
                            .child(div().truncate().child(conn.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(hsla(&c.text.muted))
                                    .child(conn.params.driver.clone()),
                            )
                            .into_any_element(),
                    );
                    if selected {
                        items.extend(self.tree_rows(cx));
                    }
                }
                div().flex().flex_col().children(items)
            }
        };
        let sidebar = div()
            .w(px(240.))
            .h_full()
            .p_3()
            .bg(hsla(&c.surface.primary))
            .border_r_1()
            .border_color(hsla(&c.border.subtle))
            .text_color(hsla(&c.text.primary))
            .child(t("sidebar.connections"))
            .child(list);
        let toolbar = div()
            .flex()
            .items_center()
            .gap_3()
            .p_2()
            .child(
                Button::new("run")
                    .primary()
                    .label(t("editor.run"))
                    .disabled(self.selected.is_none())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.run(&RunQuery, window, cx)
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(hsla(&c.text.muted))
                    .child(self.status.clone()),
            );
        let grid = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(hsla(&c.bg.base))
            .child(match &self.error {
                Some(error) => div().p_3().child(error.clone()).into_any_element(),
                None => DataTable::new(&self.table).into_any_element(),
            });
        div()
            .size_full()
            .flex()
            .bg(hsla(&c.bg.base))
            .text_color(hsla(&c.text.primary))
            .on_action(cx.listener(Self::run))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(Editor::new(&self.input).h(px(160.)))
                    .child(toolbar)
                    .child(grid),
            )
    }
}
