use super::bench::{self, Bench};
use super::grid::ResultGrid;
use super::i18n::t;
use super::tabs::Tabs;
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

actions!(native, [RunQuery, NewTab, CloseTab]);

pub fn bind_keys(cx: &mut App) {
    // The input binds the same keys to Enter, so this must come after gpui_component::init.
    let key = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    cx.bind_keys([KeyBinding::new(key, RunQuery, Some("Input"))]);
    let (new_tab, close_tab) = if cfg!(target_os = "macos") {
        ("cmd-t", "cmd-w")
    } else {
        ("ctrl-t", "ctrl-w")
    };
    cx.bind_keys([
        KeyBinding::new(new_tab, NewTab, None),
        KeyBinding::new(close_tab, CloseTab, None),
    ]);
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

struct TabView {
    connection: Option<String>,
    // Schema and database of the last opened table, used by the next run.
    scope: Location,
    input: Entity<EditorState>,
    table: Entity<TableState<ResultGrid>>,
    status: String,
    error: Option<String>,
    rows: usize,
    // Result sets seen in the current run; only the first is shown.
    sets: usize,
    bench: Option<Bench>,
}

fn new_view(
    colors: &Colors,
    connection: Option<String>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> TabView {
    let grid = ResultGrid::new(hsla(&colors.text.primary), hsla(&colors.text.muted));
    TabView {
        connection,
        scope: Location::default(),
        input: cx.new(|cx| EditorState::new(window, cx).language("sql")),
        table: cx.new(|cx| TableState::new(grid, window, cx).col_movable(false)),
        status: String::new(),
        error: None,
        rows: 0,
        sets: 0,
        bench: None,
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
    connected: HashSet<String>,
    tabs: Tabs<TabView>,
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
        Self {
            tabs: Tabs::new(new_view(&colors, None, window, cx)),
            colors,
            context,
            runtime,
            cancellation,
            connections,
            selected: None,
            tree: Tree::default(),
            tree_id: 0,
            connected: HashSet::new(),
        }
    }

    pub fn start_bench(&mut self, bench: Bench, window: &mut Window, cx: &mut Context<Self>) {
        self.pick(bench.connection.clone(), cx);
        let id = self.tabs.active_id();
        let view = &mut self.tabs.active_mut().view;
        view.input.update(cx, |input, cx| {
            input.set_value(bench.sql.clone(), window, cx)
        });
        view.bench = Some(Bench {
            start: std::time::Instant::now(),
            ..bench
        });
        self.run(id, window, cx);
    }

    fn pick(&mut self, connection: String, cx: &mut Context<Self>) {
        let tab = self.tabs.active_mut();
        tab.stream.stop();
        let view = &mut tab.view;
        view.connection = Some(connection.clone());
        view.scope = Location::default();
        view.error = None;
        view.status.clear();
        let id = tab.id;
        self.show_columns(id, Vec::new(), cx);
        self.show_tree(connection, cx);
    }

    fn show_tree(&mut self, connection: String, cx: &mut Context<Self>) {
        self.selected = Some(connection);
        self.tree = Tree::default();
        self.tree_id += 1;
        self.tree.root = Some(Load::Idle);
        self.load(Vec::new(), cx);
        cx.notify();
    }

    fn activate(&mut self, id: u64, cx: &mut Context<Self>) {
        self.tabs.select(id);
        let connection = self.tabs.active_mut().view.connection.clone();
        if connection.is_some() && connection != self.selected {
            self.show_tree(connection.unwrap_or_default(), cx);
        }
        cx.notify();
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let view = new_view(&self.colors, self.selected.clone(), window, cx);
        self.tabs.open(None, None, view);
        cx.notify();
    }

    fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let colors = &self.colors;
        let selected = self.selected.clone();
        self.tabs
            .close(id, || new_view(colors, selected, window, cx));
        self.activate(self.tabs.active_id(), cx);
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
        let key = format!(
            "{connection_id}|{:?}|{:?}|{name}",
            location.database, location.schema
        );
        if let Some(id) = self.tabs.find(&key) {
            self.activate(id, cx);
            return;
        }
        let (context, tree_id) = (self.context.clone(), self.tree_id);
        let title = name.clone();
        let task = {
            let location = location.clone();
            let connection_id = connection_id.clone();
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
                        if let Some(id) = this.tabs.find(&key) {
                            this.activate(id, cx);
                            return;
                        }
                        let mut view = new_view(&this.colors, Some(connection_id), window, cx);
                        view.scope = location;
                        view.input
                            .update(cx, |input, cx| input.set_value(sql, window, cx));
                        let id = this.tabs.open(Some(title), Some(key), view);
                        this.run(id, window, cx);
                    }
                    Err(error) => {
                        this.tabs.active_mut().view.error = Some(error);
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

    fn show_columns(&mut self, id: u64, columns: Vec<String>, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        tab.view.rows = 0;
        tab.view.table.update(cx, |table, cx| {
            table.delegate_mut().set(columns, Vec::new());
            table.refresh(cx);
            cx.notify();
        });
    }

    fn show_chunk(
        &mut self,
        id: u64,
        chunk: QueryChunk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.tabs.get_mut(id).map(|tab| &mut tab.view) else {
            return;
        };
        if matches!(chunk, QueryChunk::Columns(_)) {
            view.sets += 1;
        }
        if view.sets > 1 {
            return;
        }
        match chunk {
            QueryChunk::Columns(columns) => self.show_columns(id, columns, cx),
            QueryChunk::Rows(rows) => {
                if view.rows == 0 {
                    if let Some(bench) = &view.bench {
                        bench::first_batch(bench, window);
                    }
                }
                view.rows += rows.len();
                view.status =
                    t("editor.rowsRetrieved").replace("{{count}}", &view.rows.to_string());
                view.table.update(cx, |table, cx| {
                    table.delegate_mut().append(rows);
                    cx.notify();
                });
            }
        }
        cx.notify();
    }

    fn run_active(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.run(self.tabs.active_id(), window, cx);
    }

    fn run(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        let Some(connection_id) = tab.view.connection.clone() else {
            tab.view.status = t("sidebar.selectConnection").into();
            cx.notify();
            return;
        };
        tab.stream.stop();
        let run_id = tab.stream.id();
        let view = &mut tab.view;
        let input = view.input.read(cx);
        let sql = sql_to_run(&input.selected_value(), &input.value());
        let key = if self.connected.contains(&connection_id) {
            "editor.executingQuery"
        } else {
            "connections.connecting"
        };
        view.status = t(key).into();
        view.error = None;
        view.sets = 0;
        let (context, cancellation) = (self.context.clone(), self.cancellation.clone());
        let id_for_task = connection_id.clone();
        let scope = view.scope.clone();
        let (sender, mut chunks) = tokio::sync::mpsc::unbounded_channel();
        let task = self.runtime.spawn(async move {
            queries::stream_query(
                &context,
                &cancellation,
                QueryRequestScope::DESKTOP,
                id_for_task,
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
        tab.stream.start(task.abort_handle());
        cx.spawn_in(window, async move |this, cx| {
            while let Some(chunk) = chunks.recv().await {
                let current = this.update_in(cx, |this, window, cx| {
                    let current = this
                        .tabs
                        .get_mut(id)
                        .is_some_and(|tab| tab.stream.id() == run_id);
                    if current {
                        this.show_chunk(id, chunk, window, cx);
                    }
                    current
                });
                if !matches!(current, Ok(true)) {
                    return;
                }
            }
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update_in(cx, |this, window, cx| {
                let Some(tab) = this.tabs.get_mut(id) else {
                    return;
                };
                if tab.stream.id() != run_id {
                    return;
                }
                tab.stream.finish();
                let view = &mut tab.view;
                match result {
                    Ok(summary) => {
                        this.connected.insert(connection_id);
                        view.status = format!(
                            "{} \u{b7} {} ms",
                            t("editor.rowsRetrieved").replace("{{count}}", &view.rows.to_string()),
                            summary.fetch_time_ms.unwrap_or_default().round()
                        );
                        if let Some(bench) = view.bench.take() {
                            bench::loaded(bench, view.rows, view.table.clone(), window);
                        }
                    }
                    Err(error) => {
                        if view.bench.is_some() {
                            bench::fail(&error);
                        }
                        view.status.clear();
                        view.error = Some(error);
                        this.show_columns(id, Vec::new(), cx);
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
                                this.pick(id.clone(), cx);
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
        let active = self.tabs.active();
        let active_id = active.id;
        let view = &active.view;
        let mut bar = div().flex().items_center().gap_1().px_2().pt_1();
        for tab in self.tabs.items() {
            let id = tab.id;
            let current = id == active_id;
            bar =
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(if current {
                            hsla(&c.border.focus)
                        } else {
                            gpui::transparent_black()
                        })
                        .bg(if current {
                            hsla(&c.bg.elevated)
                        } else {
                            gpui::transparent_black()
                        })
                        .child(
                            div()
                                .id(ElementId::from(format!("tab{id}")))
                                .px_2()
                                .max_w(px(160.))
                                .truncate()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.activate(id, cx)
                                }))
                                .child(tab.title.clone()),
                        )
                        .child(
                            Button::new(ElementId::from(format!("close{id}")))
                                .ghost()
                                .label("\u{d7}")
                                .tooltip(t("editor.closeTab"))
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.close_tab(id, window, cx)
                                })),
                        ),
                );
        }
        let bar = bar.child(
            Button::new("new-tab")
                .ghost()
                .label("+")
                .tooltip(t("editor.newConsole"))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.new_tab(&NewTab, window, cx)
                })),
        );
        let toolbar = div()
            .flex()
            .items_center()
            .gap_3()
            .p_2()
            .child(
                Button::new("run")
                    .primary()
                    .label(t("editor.run"))
                    .disabled(view.connection.is_none())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.run_active(&RunQuery, window, cx)
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(hsla(&c.text.muted))
                    .child(view.status.clone()),
            );
        let grid = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(hsla(&c.bg.base))
            .child(match &view.error {
                Some(error) => div().p_3().child(error.clone()).into_any_element(),
                None => DataTable::new(&view.table).into_any_element(),
            });
        div()
            .size_full()
            .flex()
            .bg(hsla(&c.bg.base))
            .text_color(hsla(&c.text.primary))
            .on_action(cx.listener(Self::run_active))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.close_tab(this.tabs.active_id(), window, cx)
            }))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(bar)
                    .child(Editor::new(&view.input).h(px(160.)))
                    .child(toolbar)
                    .child(grid),
            )
    }
}
