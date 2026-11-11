use super::i18n::t;
use super::theme::hsla;
use super::theme::Colors;
use crate::models::SavedConnection;
use gpui::{
    div, px, ClickEvent, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
};

pub struct Shell {
    colors: Colors,
    connections: Result<Vec<SavedConnection>, String>,
    selected: Option<String>,
}

impl Shell {
    pub fn new(colors: Colors, connections: Result<Vec<SavedConnection>, String>) -> Self {
        Self {
            colors,
            connections,
            selected: None,
        }
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
            Ok(connections) => div().flex().flex_col().children(connections.iter().map(|conn| {
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
                        this.selected = Some(id.clone());
                        cx.notify();
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
        let input = div()
            .h(px(96.))
            .w_full()
            .bg(hsla(&c.bg.elevated))
            .border_b_1()
            .border_color(hsla(&c.border.default));
        let grid = div().flex_1().w_full().bg(hsla(&c.bg.base));
        div()
            .size_full()
            .flex()
            .bg(hsla(&c.bg.base))
            .text_color(hsla(&c.text.primary))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(input)
                    .child(grid),
            )
    }
}
