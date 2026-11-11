mod i18n;
mod shell;
#[cfg(test)]
mod tests;
mod theme;

use crate::application::connections;
use crate::runtime::RuntimeContext;
use gpui::{App, AppContext as _, WindowOptions};
use gpui_component::{Root, Theme};

/// Blocks on the main thread until the last window closes. The handle is kept
/// for later milestones, which run database work on the tokio runtime.
pub fn run(context: RuntimeContext, _runtime: tokio::runtime::Handle) {
    let colors = theme::load(&context);
    let connections = connections::load_redacted_connections(&context.paths.connections_file());
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::update(cx, |t| theme::apply(&colors, &mut t.colors));
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let options = WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Tabularis".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            let view = cx.new(|_| shell::Shell::new(colors, connections));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("Failed to open window");
        cx.activate(true);
    });
}
