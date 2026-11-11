use super::bench::{self, Bench};
use super::grid::ResultGrid;
use super::i18n::t;
use super::theme::Colors;
use super::theme::hsla;
use crate::application::queries::{self, QueryRequestScope};
use crate::commands::QueryCancellationState;
use crate::drivers::driver_trait::QueryChunk;
use crate::models::SavedConnection;
use crate::runtime::RuntimeContext;
use gpui::{
    App, AppContext as _, ClickEvent, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, KeyBinding, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Window, actions, div, px,
};
use gpui_component::Disableable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Editor, EditorState};
use gpui_component::table::{DataTable, TableState};
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

pub struct Shell {
    colors: Colors,
    context: RuntimeContext,
    runtime: tokio::runtime::Handle,
    cancellation: Arc<QueryCancellationState>,
    connections: Result<Vec<SavedConnection>, String>,
    selected: Option<String>,
    connected: HashSet<String>,
    input: Entity<EditorState>,
    table: Entity<TableState<ResultGrid>>,
    status: String,
    error: Option<String>,
    // Bumped when a run is dropped so its late chunks and result are ignored.
    run_id: u64,
    in_flight: Option<tokio::task::AbortHandle>,
    rows: usize,
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
            connected: HashSet::new(),
            status: String::new(),
            error: None,
            run_id: 0,
            in_flight: None,
            rows: 0,
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
        self.stop();
        self.error = None;
        self.status.clear();
        self.show_columns(Vec::new(), cx);
        cx.notify();
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
        let (context, cancellation, run_id) =
            (self.context.clone(), self.cancellation.clone(), self.run_id);
        let id = connection_id.clone();
        let (sender, mut chunks) = tokio::sync::mpsc::unbounded_channel();
        let task = self.runtime.spawn(async move {
            queries::stream_query(
                &context,
                &cancellation,
                QueryRequestScope::DESKTOP,
                id,
                sql,
                None,
                None,
                None,
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
            Ok(connections) => div()
                .flex()
                .flex_col()
                .children(connections.iter().map(|conn| {
                    let selected = self.selected.as_deref() == Some(&conn.id);
                    let id = conn.id.clone();
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
                        }))
                        .child(div().truncate().child(conn.name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(hsla(&c.text.muted))
                                .child(conn.params.driver.clone()),
                        )
                })),
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
