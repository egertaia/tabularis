mod backend;
mod bench;
mod connection_form;
mod edits;
mod grid;
mod i18n;
mod settings;
mod shell;
mod tabs;
#[cfg(test)]
mod tests;
mod theme;
mod tree;

use crate::runtime::{state::ApplicationState, RuntimeContext};
use gpui::{App, AppContext as _, WindowOptions};
use gpui_component::Root;

/// Blocks on the main thread until the last window closes. Database work runs on the tokio runtime.
pub fn run(context: RuntimeContext, runtime: tokio::runtime::Handle) {
    let state = std::sync::Arc::new(ApplicationState::default());
    let config = backend::load_config(&context);
    let (page_size, font_size) = (settings::page_size(&config), settings::font_size(&config));
    let colors = theme::load(&context);
    let connections = backend::load_redacted_connections(&context.paths.connections_file());
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        theme::install(&colors, cx);
        theme::set_editor_font_size(font_size, cx);
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
                shell::Shell::new(
                    window,
                    cx,
                    colors,
                    context,
                    runtime,
                    state,
                    connections,
                    page_size,
                )
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
