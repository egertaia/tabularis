use super::grid::ResultGrid;
use super::i18n::t;
use super::theme::Colors;
use super::theme::hsla;
use crate::application::queries::{self, QueryRequestScope, QueryResponsePolicy};
use crate::commands::QueryCancellationState;
use crate::models::SavedConnection;
use crate::runtime::RuntimeContext;
use gpui::{
    App, AppContext as _, ClickEvent, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, KeyBinding, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Window, actions, div, px,
};
use gpui_component::Disableable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Textarea, TextareaState};
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

pub struct Shell {
    colors: Colors,
    context: RuntimeContext,
    runtime: tokio::runtime::Handle,
    cancellation: Arc<QueryCancellationState>,
    connections: Result<Vec<SavedConnection>, String>,
    selected: Option<String>,
    connected: HashSet<String>,
    input: Entity<TextareaState>,
    table: Entity<TableState<ResultGrid>>,
    status: String,
    error: Option<String>,
    running: bool,
    // Bumped when the selection changes so a late result is dropped.
    run_id: u64,
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
            input: cx.new(|cx| TextareaState::new(window, cx)),
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
            running: false,
            run_id: 0,
        }
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.run_id += 1;
        self.running = false;
        self.error = None;
        self.status.clear();
        self.show_rows(Vec::new(), Vec::new(), cx);
        cx.notify();
    }

    fn show_rows(
        &mut self,
        columns: Vec<String>,
        rows: Vec<Vec<serde_json::Value>>,
        cx: &mut Context<Self>,
    ) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().set(columns, rows);
            table.refresh(cx);
            cx.notify();
        });
    }

    fn run(&mut self, _: &RunQuery, _: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(connection_id) = self.selected.clone() else {
            self.status = t("sidebar.selectConnection").into();
            cx.notify();
            return;
        };
        let sql = self.input.read(cx).value().to_string();
        let key = if self.connected.contains(&connection_id) {
            "editor.executingQuery"
        } else {
            "connections.connecting"
        };
        self.status = t(key).into();
        self.error = None;
        self.running = true;
        let (context, cancellation, run_id) =
            (self.context.clone(), self.cancellation.clone(), self.run_id);
        let id = connection_id.clone();
        let task = self.runtime.spawn(async move {
            queries::execute_query(
                &context,
                &cancellation,
                QueryRequestScope::DESKTOP,
                QueryResponsePolicy::Unbounded,
                id,
                sql,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update(cx, |this, cx| {
                if this.run_id != run_id {
                    return;
                }
                this.running = false;
                match result {
                    Ok(result) => {
                        this.connected.insert(connection_id);
                        this.status = format!(
                            "{} \u{b7} {} ms",
                            t("editor.rowsRetrieved")
                                .replace("{{count}}", &result.rows.len().to_string()),
                            result.fetch_time_ms.unwrap_or_default().round()
                        );
                        this.show_rows(result.columns, result.rows, cx);
                    }
                    Err(error) => {
                        this.status.clear();
                        this.error = Some(error);
                        this.show_rows(Vec::new(), Vec::new(), cx);
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
                    .disabled(self.selected.is_none() || self.running)
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
                    .child(Textarea::new(&self.input).h(px(96.)))
                    .child(toolbar)
                    .child(grid),
            )
    }
}
