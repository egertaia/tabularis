use super::backend;
use super::i18n::t;
use super::theme::{colors_from_source, hsla, Colors, FALLBACK_ID};
use crate::config::AppConfig;
use crate::runtime::RuntimeContext;
use gpui::{
    div, AppContext as _, ClickEvent, Context, Entity, EventEmitter, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_component::IndexPath;

const DEFAULT_PAGE_SIZE: u32 = 500;
const DEFAULT_FONT_SIZE: u32 = 14;
const FONT_SIZES: std::ops::RangeInclusive<u32> = 10..=20;

pub fn page_size(config: &AppConfig) -> u32 {
    config.result_page_size.unwrap_or(DEFAULT_PAGE_SIZE)
}

pub fn font_size(config: &AppConfig) -> u32 {
    config
        .editor_font_size
        .map_or(DEFAULT_FONT_SIZE, clamp_font_size)
}

fn clamp_font_size(size: u32) -> u32 {
    size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end())
}

pub fn theme_update(id: &str) -> AppConfig {
    AppConfig {
        theme: Some(id.into()),
        ..Default::default()
    }
}

pub fn page_size_update(size: u32) -> AppConfig {
    AppConfig {
        result_page_size: Some(size),
        ..Default::default()
    }
}

pub fn font_size_update(size: u32) -> AppConfig {
    AppConfig {
        editor_font_size: Some(size),
        ..Default::default()
    }
}

/// How many of a `batch` of rows fit under `limit` (0 = no limit), and whether more rows came than fit.
pub fn within_page_size(limit: u32, shown: usize, batch: usize) -> (usize, bool) {
    if limit == 0 {
        return (batch, false);
    }
    let room = (limit as usize).saturating_sub(shown);
    if batch > room {
        (room, true)
    } else {
        (batch, false)
    }
}

pub enum SettingsEvent {
    Theme(Box<Colors>),
    PageSize(u32),
    FontSize(u32),
    Closed,
}

#[derive(Clone)]
struct ThemeItem {
    id: String,
    name: SharedString,
    colors: Colors,
}

impl SelectItem for ThemeItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &String {
        &self.id
    }
}

pub struct SettingsView {
    colors: Colors,
    context: RuntimeContext,
    runtime: tokio::runtime::Handle,
    themes: Vec<ThemeItem>,
    picker: Entity<SelectState<Vec<ThemeItem>>>,
    page_size: Entity<InputState>,
    font_size: Entity<InputState>,
    saved_page_size: u32,
    saved_font_size: u32,
    error: Option<String>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

fn input(value: u32, window: &mut Window, cx: &mut Context<SettingsView>) -> Entity<InputState> {
    let state = cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
    cx.subscribe_in(&state, window, |this, _, event: &InputEvent, window, cx| {
        if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
            this.commit(window, cx);
        }
    })
    .detach();
    state
}

impl SettingsView {
    pub fn new(
        colors: Colors,
        context: RuntimeContext,
        runtime: tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = backend::load_config(&context);
        // Only legacy-format themes map onto the native colors.
        let themes: Vec<ThemeItem> = backend::theme_catalog(&context)
            .themes
            .into_iter()
            .filter(|theme| theme.available)
            .filter_map(|theme| {
                Some(ThemeItem {
                    colors: colors_from_source(&theme.source)?,
                    id: theme.id,
                    name: theme.name.into(),
                })
            })
            .collect();
        let active = config.theme.as_deref().unwrap_or(FALLBACK_ID);
        let selected = themes.iter().position(|theme| theme.id == active);
        let picker =
            cx.new(|cx| SelectState::new(themes.clone(), selected.map(IndexPath::new), window, cx));
        cx.subscribe_in(&picker, window, |this, _, event: &SelectEvent<_>, _, cx| {
            let SelectEvent::Confirm(Some(id)) = event else {
                return;
            };
            if let Some(theme) = this.themes.iter().find(|theme| &theme.id == id) {
                this.colors = theme.colors.clone();
                cx.emit(SettingsEvent::Theme(Box::new(theme.colors.clone())));
                this.save(theme_update(id), cx);
            }
        })
        .detach();
        let (saved_page_size, saved_font_size) = (page_size(&config), font_size(&config));
        Self {
            page_size: input(saved_page_size, window, cx),
            font_size: input(saved_font_size, window, cx),
            saved_page_size,
            saved_font_size,
            colors,
            context,
            runtime,
            themes,
            picker,
            error: None,
        }
    }

    /// Saves each number input that holds a new valid value; an invalid one reverts.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = |state: &Entity<InputState>| state.read(cx).value().trim().to_string();
        let page = text(&self.page_size).parse::<u32>().ok();
        let font = text(&self.font_size)
            .parse::<u32>()
            .ok()
            .map(clamp_font_size);
        let (saved_page, saved_font) = (self.saved_page_size, self.saved_font_size);
        let page = page.unwrap_or(saved_page);
        let font = font.unwrap_or(saved_font);
        for (state, value) in [(&self.page_size, page), (&self.font_size, font)] {
            if state.read(cx).value().as_ref() != value.to_string() {
                state.update(cx, |state, cx| {
                    state.set_value(value.to_string(), window, cx)
                });
            }
        }
        if page != saved_page {
            self.saved_page_size = page;
            cx.emit(SettingsEvent::PageSize(page));
            self.save(page_size_update(page), cx);
        }
        if font != saved_font {
            self.saved_font_size = font;
            cx.emit(SettingsEvent::FontSize(font));
            self.save(font_size_update(font), cx);
        }
    }

    fn save(&mut self, update: AppConfig, cx: &mut Context<Self>) {
        let context = self.context.clone();
        let task = self
            .runtime
            .spawn_blocking(move || backend::save_config(&context, update));
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = result {
                this.update(cx, |this, cx| {
                    this.error = Some(format!("{}: {error}", t("common.error")));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.colors;
        let muted = hsla(&c.text.muted);
        let field = |label: &'static str, description: Option<&'static str>, control| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label))
                .child(control)
                .children(description.map(|text| div().text_xs().text_color(muted).child(text)))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .max_w(gpui::px(520.))
            .child(div().text_lg().child(t("settings.title")))
            .child(field(
                t("settings.themeSelection"),
                None,
                Select::new(&self.picker)
                    .accessibility_label(t("settings.themeSelection"))
                    .into_any_element(),
            ))
            .child(field(
                t("settings.pageSize"),
                Some(t("settings.pageSizeDesc")),
                Input::new(&self.page_size)
                    .aria_label(t("settings.pageSize"))
                    .into_any_element(),
            ))
            .child(field(
                t("settings.appearance_editorFontSize"),
                None,
                Input::new(&self.font_size)
                    .aria_label(t("settings.appearance_editorFontSize"))
                    .into_any_element(),
            ))
            .children(self.error.clone().map(|error| {
                div()
                    .text_sm()
                    .text_color(hsla(&c.accent.error))
                    .child(error)
            }))
            .child(
                div().child(
                    Button::new("close-settings")
                        .label(t("common.close"))
                        .on_click(
                            cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(SettingsEvent::Closed)),
                        ),
                ),
            )
    }
}
