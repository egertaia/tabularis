use super::backend;
use super::bench::{self, Bench};
use super::connection_form::{ConnectionForm, FormEvent};
use super::edits::{self, Source};
use super::grid::ResultGrid;
use super::i18n::t;
use super::settings::{self, SettingsEvent, SettingsView};
use super::tabs::Tabs;
use super::theme::{self, hsla, Colors};
use super::tree::{Child, Load, Node, RowKind, Tree};
use crate::application::explorer::{Entry, EntryKind, Location};
use crate::application::queries::QueryRequestScope;
use crate::commands::QueryCancellationState;
use crate::drivers::driver_trait::QueryChunk;
use crate::models::SavedConnection;
use crate::runtime::{state::ApplicationState, RuntimeContext};
use gpui::{
    actions, div, prelude::FluentBuilder as _, px, AnyElement, App, AppContext as _, ClickEvent,
    Context, ElementId, Entity, Focusable as _, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Editor, EditorState, Escape, InputEvent, InputState};
use gpui_component::notification::Notification;
use gpui_component::table::{
    DataTable, TableDelegate as _, TableEvent, TableSelection, TableState,
};
use gpui_component::{Disableable as _, Sizable as _, WindowExt as _};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

actions!(native, [RunQuery, NewTab, CloseTab, SaveEdits, EditCell]);

pub fn bind_keys(cx: &mut App) {
    // The input binds the same keys to Enter, so this must come after gpui_component::init.
    let key = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    cx.bind_keys([KeyBinding::new(key, RunQuery, Some("Input"))]);
    let (new_tab, close_tab, save) = if cfg!(target_os = "macos") {
        ("cmd-t", "cmd-w", "cmd-s")
    } else {
        ("ctrl-t", "ctrl-w", "ctrl-s")
    };
    cx.bind_keys([
        KeyBinding::new(new_tab, NewTab, None),
        KeyBinding::new(close_tab, CloseTab, None),
        KeyBinding::new(save, SaveEdits, None),
        KeyBinding::new("enter", EditCell, Some("DataTable")),
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
        Job::List(location) => Ok(backend::list(&context, &id, &location)
            .await?
            .into_iter()
            .map(|entry| Child::Node(Node::new(&location, entry)))
            .collect()),
        Job::Columns(location, entry) => Ok(backend::columns(&context, &id, &location, &entry)
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
    // Set when the tab was opened from a table the grid may edit.
    source: Option<Arc<Source>>,
    // Primary key columns of the shown result, when it may be edited.
    keys: Option<Vec<usize>>,
    sql: String,
    saving: bool,
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
    cells: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> TabView {
    let grid = ResultGrid::new(colors);
    let table = cx.new(|cx| {
        TableState::new(grid, window, cx)
            .col_movable(false)
            .cell_selectable(cells)
    });
    if cells {
        cx.subscribe_in(&table, window, |this, _, event: &TableEvent, window, cx| {
            if let TableEvent::DoubleClickedCell(row, col) = event {
                this.start_edit(*row, *col, window, cx);
            }
        })
        .detach();
    }
    TabView {
        connection,
        scope: Location::default(),
        source: None,
        keys: None,
        sql: String::new(),
        saving: false,
        input: cx.new(|cx| EditorState::new(window, cx).language("sql")),
        table,
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
    state: Arc<ApplicationState>,
    cancellation: Arc<QueryCancellationState>,
    connections: Result<Vec<SavedConnection>, String>,
    form: Option<Entity<ConnectionForm>>,
    settings: Option<Entity<SettingsView>>,
    // Rows a run keeps before it stops; 0 keeps all.
    page_size: u32,
    selected: Option<String>,
    tree: Tree,
    // Bumped on every selection so late metadata replies are ignored.
    tree_id: u64,
    connected: HashSet<String>,
    tabs: Tabs<TabView>,
}

impl Shell {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        colors: Colors,
        context: RuntimeContext,
        runtime: tokio::runtime::Handle,
        state: Arc<ApplicationState>,
        connections: Result<Vec<SavedConnection>, String>,
        page_size: u32,
    ) -> Self {
        Self {
            tabs: Tabs::new(new_view(&colors, None, false, window, cx)),
            colors,
            context,
            runtime,
            cancellation: state.query_cancellation.clone(),
            state,
            connections,
            form: None,
            settings: None,
            page_size,
            selected: None,
            tree: Tree::default(),
            tree_id: 0,
            connected: HashSet::new(),
        }
    }

    pub fn start_bench(&mut self, bench: Bench, window: &mut Window, cx: &mut Context<Self>) {
        self.page_size = 0;
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
        view.source = None;
        view.error = None;
        view.status.clear();
        let id = tab.id;
        self.show_columns(id, Vec::new(), cx);
        self.show_tree(connection, cx);
    }

    fn open_form(
        &mut self,
        saved: Option<SavedConnection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = self.runtime.spawn(backend::list_drivers());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(drivers) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let index = match &saved {
                    Some(saved) => drivers
                        .iter()
                        .position(|driver| driver.id == saved.params.driver),
                    None => (!drivers.is_empty()).then_some(0),
                };
                let Some(index) = index else {
                    let driver = saved.map(|saved| saved.params.driver).unwrap_or_default();
                    let message =
                        format!("{driver}: {}", t("connections.importFromApp.notInstalled"));
                    window.push_notification(Notification::error(message), cx);
                    return;
                };
                let (colors, context, state, runtime) = (
                    this.colors.clone(),
                    this.context.clone(),
                    this.state.clone(),
                    this.runtime.clone(),
                );
                let form = cx.new(|cx| {
                    ConnectionForm::new(
                        colors, context, state, runtime, drivers, index, saved, window, cx,
                    )
                });
                this.settings = None;
                cx.subscribe_in(&form, window, |this, _, event: &FormEvent, window, cx| {
                    this.form = None;
                    if let FormEvent::Saved { plain_text } = event {
                        this.reload_connections();
                        if *plain_text {
                            let message = t("newConnection.secretsStoredInPlainText");
                            window.push_notification(Notification::warning(message), cx);
                        }
                    }
                    cx.notify();
                })
                .detach();
                this.form = Some(form);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (colors, context, runtime) = (
            self.colors.clone(),
            self.context.clone(),
            self.runtime.clone(),
        );
        let view = cx.new(|cx| SettingsView::new(colors, context, runtime, window, cx));
        cx.subscribe_in(
            &view,
            window,
            |this, _, event: &SettingsEvent, window, cx| {
                match event {
                    SettingsEvent::Theme(colors) => {
                        theme::install(colors, cx);
                        for tab in this.tabs.items_mut() {
                            tab.view.table.update(cx, |table, cx| {
                                table.delegate_mut().recolor(colors);
                                cx.notify();
                            });
                        }
                        this.colors = (**colors).clone();
                    }
                    SettingsEvent::PageSize(size) => this.page_size = *size,
                    SettingsEvent::FontSize(size) => theme::set_editor_font_size(*size, cx),
                    SettingsEvent::Closed => this.settings = None,
                }
                window.refresh();
                cx.notify();
            },
        )
        .detach();
        self.form = None;
        self.settings = Some(view);
        cx.notify();
    }

    fn reload_connections(&mut self) {
        self.connections =
            backend::load_redacted_connections(&self.context.paths.connections_file());
    }

    fn confirm_delete(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let shell = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (shell, id) = (shell.clone(), id.clone());
            alert
                .confirm()
                .title(t("connections.deleteTitle"))
                .description(t("connections.confirmDelete"))
                .ok_text(t("common.delete"))
                .cancel_text(t("common.cancel"))
                .on_ok(move |_, window, cx| {
                    shell
                        .update(cx, |this, cx| this.delete(id.clone(), window, cx))
                        .ok();
                    true
                })
        });
    }

    fn delete(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let (context, state) = (self.context.clone(), self.state.clone());
        let task = self.runtime.spawn({
            let id = id.clone();
            async move {
                backend::disconnect_connection(&context, &state, &id).await?;
                backend::delete_connection(&context, &state, id).await
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        this.connected.remove(&id);
                        if this.selected.as_deref() == Some(id.as_str()) {
                            this.selected = None;
                            this.tree = Tree::default();
                            this.tree_id += 1;
                        }
                        this.reload_connections();
                    }
                    Err(error) => {
                        let message = format!("{}\n\nError: {error}", t("connections.failDelete"));
                        window.push_notification(Notification::error(message), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_tree(&mut self, connection: String, cx: &mut Context<Self>) {
        self.selected = Some(connection);
        self.tree = Tree::default();
        self.tree_id += 1;
        self.tree.root = Some(Load::Idle);
        self.load(Vec::new(), cx);
        cx.notify();
    }

    fn activate(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.select(id);
        let connection = self.tabs.active_mut().view.connection.clone();
        if connection.is_some() && connection != self.selected {
            self.show_tree(connection.unwrap_or_default(), cx);
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    // Deferred: the editor is only part of the focus tree after the next render.
    fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.tabs.active().view.input.clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx))
        });
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let view = new_view(&self.colors, self.selected.clone(), false, window, cx);
        self.tabs.open(None, None, view);
        self.focus_editor(window, cx);
        cx.notify();
    }

    fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let colors = &self.colors;
        let selected = self.selected.clone();
        self.tabs
            .close(id, || new_view(colors, selected, false, window, cx));
        self.activate(self.tabs.active_id(), window, cx);
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
        let (location, name, kind) = (
            node.location.clone(),
            node.entry.name.clone(),
            node.entry.kind,
        );
        let key = format!(
            "{connection_id}|{:?}|{:?}|{name}",
            location.database, location.schema
        );
        if let Some(id) = self.tabs.find(&key) {
            self.activate(id, window, cx);
            return;
        }
        let (context, tree_id) = (self.context.clone(), self.tree_id);
        let title = name.clone();
        let task = {
            let location = location.clone();
            let connection_id = connection_id.clone();
            self.runtime.spawn(async move {
                let sql =
                    backend::open_table_query(&context, &connection_id, &location, &name).await?;
                let columns = if kind == EntryKind::Table {
                    backend::editable_columns(&context, &connection_id, &location, &name)
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };
                Ok::<_, String>((sql, columns))
            })
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update_in(cx, |this, window, cx| {
                if this.tree_id != tree_id {
                    return;
                }
                match result {
                    Ok((sql, columns)) => {
                        if let Some(id) = this.tabs.find(&key) {
                            this.activate(id, window, cx);
                            return;
                        }
                        let mut view =
                            new_view(&this.colors, Some(connection_id.clone()), true, window, cx);
                        view.source = columns.map(|columns| {
                            Arc::new(Source {
                                connection_id,
                                location: location.clone(),
                                table: title.clone(),
                                sql: sql.clone(),
                                columns,
                            })
                        });
                        view.scope = location;
                        view.input
                            .update(cx, |input, cx| input.set_value(sql, window, cx));
                        let id = this.tabs.open(Some(title), Some(key), view);
                        this.focus_editor(window, cx);
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
        let view = &mut tab.view;
        view.rows = 0;
        view.keys = view
            .source
            .as_ref()
            .and_then(|source| source.keys(&view.sql, &columns));
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
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        let view = &mut tab.view;
        if matches!(chunk, QueryChunk::Columns(_)) {
            view.sets += 1;
        }
        if view.sets > 1 {
            return;
        }
        match chunk {
            QueryChunk::Columns(columns) => self.show_columns(id, columns, cx),
            QueryChunk::Rows(mut rows) => {
                if view.rows == 0 {
                    if let Some(bench) = &view.bench {
                        bench::first_batch(bench, window);
                    }
                }
                let (keep, truncated) =
                    settings::within_page_size(self.page_size, view.rows, rows.len());
                rows.truncate(keep);
                view.rows += keep;
                view.status =
                    t("editor.rowsRetrieved").replace("{{count}}", &view.rows.to_string());
                if truncated {
                    let hint = t("editor.batchSnapshotTruncated");
                    view.status.push_str(&format!(
                        " \u{b7} {}",
                        hint.replace("{{count}}", &view.rows.to_string())
                    ));
                }
                view.table.update(cx, |table, cx| {
                    table.delegate_mut().append(rows);
                    cx.notify();
                });
                if truncated {
                    if let Some(connection) = view.connection.clone() {
                        self.connected.insert(connection);
                    }
                    tab.stream.stop();
                }
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
        let input = tab.view.input.read(cx);
        let sql = sql_to_run(&input.selected_value(), &input.value());
        self.execute(id, sql, window, cx);
    }

    fn execute(&mut self, id: u64, sql: String, window: &mut Window, cx: &mut Context<Self>) {
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
        view.sql = sql.clone();
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
            backend::stream_query(
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

    /// The active tab's view and source, when its result may be edited now.
    fn editable(&self) -> Option<(&TabView, Arc<Source>)> {
        let tab = self.tabs.active();
        let view = &tab.view;
        // Rows still arriving would shift the indexes of new rows.
        let idle = !view.saving && !tab.stream.is_running();
        let source = view
            .source
            .clone()
            .filter(|_| idle && view.keys.is_some())?;
        Some((view, source))
    }

    fn update_grid(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut ResultGrid)) {
        self.tabs.active().view.table.update(cx, |table, cx| {
            f(table.delegate_mut());
            cx.notify();
        });
        cx.notify();
    }

    fn start_edit(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((view, _)) = self.editable() else {
            return;
        };
        let grid = view.table.read(cx).delegate();
        if grid.is_deleted(row) || col >= grid.columns().len() {
            return;
        }
        let text = grid.text(row, col);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.finish_edit(true, window, cx);
            }
        })
        .detach();
        let focus = input.clone();
        self.update_grid(cx, |grid| grid.editor = Some((row, col, input)));
        window.defer(cx, move |window, cx| {
            focus.update(cx, |input, cx| input.focus(window, cx))
        });
    }

    fn finish_edit(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.tabs.active().view.table.clone();
        table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            if let Some((row, col, input)) = grid.editor.take() {
                if commit {
                    let text = input.read(cx).value().to_string();
                    grid.commit(row, col, Value::String(text));
                }
                table.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        });
    }

    fn selected_row(&self, cx: &App) -> Option<usize> {
        match self.tabs.active().view.table.read(cx).selection() {
            TableSelection::Row(row) | TableSelection::Cell(row, _) => Some(row),
            _ => None,
        }
    }

    fn edit_selected(&mut self, _: &EditCell, window: &mut Window, cx: &mut Context<Self>) {
        // The input's Enter propagates up and would reopen a fresh editor with the old text.
        if self.tabs.active().view.table.read(cx).delegate().editor.is_some() {
            return;
        }
        if let TableSelection::Cell(row, col) = self.tabs.active().view.table.read(cx).selection() {
            self.start_edit(row, col, window, cx);
        }
    }

    fn set_null(&mut self, cx: &mut Context<Self>) {
        let Some((view, source)) = self.editable() else {
            return;
        };
        let TableSelection::Cell(row, col) = view.table.read(cx).selection() else {
            return;
        };
        let grid = view.table.read(cx).delegate();
        let nullable = grid
            .columns()
            .get(col)
            .is_some_and(|name| source.nullable(name));
        if nullable && !grid.is_deleted(row) {
            self.update_grid(cx, |grid| grid.commit(row, col, Value::Null));
        }
    }

    fn delete_row(&mut self, cx: &mut Context<Self>) {
        if self.editable().is_none() {
            return;
        }
        if let Some(row) = self.selected_row(cx) {
            self.update_grid(cx, |grid| grid.toggle_delete(row));
        }
    }

    fn new_row(&mut self, cx: &mut Context<Self>) {
        let Some((_, source)) = self.editable() else {
            return;
        };
        self.update_grid(cx, |grid| {
            let row = source.new_row(grid.columns());
            grid.edits.insert(row);
        });
        self.tabs.active().view.table.update(cx, |table, cx| {
            let last = table.delegate().rows_count(cx).saturating_sub(1);
            table.scroll_to_row(last, cx);
        });
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        if self.editable().is_some() {
            self.update_grid(cx, |grid| {
                grid.edits = Default::default();
                grid.editor = None;
            });
        }
    }

    fn save(&mut self, _: &SaveEdits, window: &mut Window, cx: &mut Context<Self>) {
        let Some((view, source)) = self.editable() else {
            return;
        };
        let keys = view.keys.clone().unwrap_or_default();
        let ops = view.table.read(cx).delegate().ops(&source, &keys);
        let tab = self.tabs.active_mut();
        let id = tab.id;
        let ops = match ops {
            Ok(ops) if ops.is_empty() => return,
            Ok(ops) => ops,
            Err(error) => {
                tab.view.status = format!("{}{error}", t("dataGrid.updateFailed"));
                cx.notify();
                return;
            }
        };
        tab.view.saving = true;
        let context = self.context.clone();
        let task = self
            .runtime
            .spawn(async move { edits::save(&context, &source, ops).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| {
                Err(edits::SaveError {
                    applied: Vec::new(),
                    message: error.to_string(),
                })
            });
            this.update_in(cx, |this, window, cx| {
                let Some(tab) = this.tabs.get_mut(id) else {
                    return;
                };
                tab.view.saving = false;
                match result {
                    Ok(()) => {
                        let sql = tab.view.sql.clone();
                        this.execute(id, sql, window, cx);
                    }
                    Err(error) => {
                        tab.view.status =
                            format!("{}{}", t("dataGrid.updateFailed"), error.message);
                        tab.view.table.update(cx, |table, cx| {
                            table.delegate_mut().edits.forget(&error.applied);
                            cx.notify();
                        });
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn data_toolbar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = &self.tabs.active().view;
        view.keys.as_ref()?;
        let pending = view.table.read(cx).delegate().edits.count();
        let busy = self.editable().is_none();
        let button = |id: &'static str, label: &'static str| {
            Button::new(id).ghost().label(label).disabled(busy)
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .child(
                    button("new-row", t("editor.newRow"))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.new_row(cx))),
                )
                .child(
                    button("delete-row", t("dataGrid.deleteRow"))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.delete_row(cx))),
                )
                .child(
                    button("set-null", t("dataGrid.setNull"))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_null(cx))),
                )
                .child(
                    button("submit", t("editor.submit"))
                        .tooltip(t("editor.submitChanges"))
                        .disabled(busy || pending == 0)
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.save(&SaveEdits, window, cx)
                        })),
                )
                .child(
                    button("rollback", t("editor.rollback"))
                        .tooltip(t("editor.rollbackChanges"))
                        .disabled(busy || pending == 0)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.discard(cx))),
                )
                .when(pending > 0, |bar| {
                    bar.child(
                        div()
                            .text_sm()
                            .text_color(hsla(&self.colors.text.accent))
                            .child(
                                t("editor.pendingCount").replace("{{count}}", &pending.to_string()),
                            ),
                    )
                })
                .into_any_element(),
        )
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
                    let (edit, delete) = (conn.clone(), conn.id.clone());
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
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(hsla(&c.text.muted))
                                    .child(div().flex_1().child(conn.params.driver.clone()))
                                    .child(
                                        Button::new(ElementId::from(format!("edit{}", conn.id)))
                                            .ghost()
                                            .xsmall()
                                            .label(t("connections.edit"))
                                            .on_click(cx.listener(
                                                move |this, _: &ClickEvent, window, cx| {
                                                    this.open_form(Some(edit.clone()), window, cx)
                                                },
                                            )),
                                    )
                                    .child(
                                        Button::new(ElementId::from(format!("delete{}", conn.id)))
                                            .ghost()
                                            .xsmall()
                                            .label(t("connections.delete"))
                                            .on_click(cx.listener(
                                                move |this, _: &ClickEvent, window, cx| {
                                                    this.confirm_delete(delete.clone(), window, cx)
                                                },
                                            )),
                                    ),
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(t("sidebar.connections"))
                    .child(
                        Button::new("new-connection")
                            .ghost()
                            .xsmall()
                            .label(t("connections.addConnection"))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.open_form(None, window, cx)
                            })),
                    ),
            )
            .child(list)
            .child(
                Button::new("settings")
                    .ghost()
                    .xsmall()
                    .label(t("sidebar.settings"))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_settings(window, cx)
                    })),
            );
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
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.activate(id, window, cx)
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
        let data_toolbar = self.data_toolbar(cx);
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
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::edit_selected))
            .on_action(
                cx.listener(|this, _: &Escape, window, cx| this.finish_edit(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.close_tab(this.tabs.active_id(), window, cx)
            }))
            .child(sidebar)
            .child(match (&self.settings, &self.form) {
                (Some(settings), _) => div().flex_1().h_full().child(settings.clone()),
                (None, Some(form)) => div().flex_1().h_full().child(form.clone()),
                (None, None) => div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(bar)
                    .child(Editor::new(&view.input).h(px(160.)))
                    .child(toolbar)
                    .children(data_toolbar)
                    .child(grid),
            })
    }
}
