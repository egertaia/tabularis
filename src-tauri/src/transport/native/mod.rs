mod backend;
mod bench;
mod connection_form;
mod edits;
mod grid;
mod i18n;
mod shell;
mod tabs;
#[cfg(test)]
mod tests;
mod theme;
mod tree;

use crate::runtime::{state::ApplicationState, RuntimeContext};
use gpui::{App, AppContext as _, WindowOptions};
use gpui_component::{Root, Theme};

/// Blocks on the main thread until the last window closes. Database work runs on the tokio runtime.
pub fn run(context: RuntimeContext, runtime: tokio::runtime::Handle) {
    let state = std::sync::Arc::new(ApplicationState::default());
    let colors = theme::load(&context);
    let connections = backend::load_redacted_connections(&context.paths.connections_file());
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::update(cx, |t| {
            theme::apply(&colors, &mut t.colors);
            t.highlight_theme = theme::highlight_theme(&colors);
        });
        shell::bind_keys(cx);
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
            let bench = bench::Bench::from_env();
            let view = cx.new(|cx| {
                shell::Shell::new(window, cx, colors, context, runtime, state, connections)
            });
            if let Some(bench) = bench {
                view.update(cx, |shell, cx| shell.start_bench(bench, window, cx));
            }
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("Failed to open window");
        cx.activate(true);
    });
}
