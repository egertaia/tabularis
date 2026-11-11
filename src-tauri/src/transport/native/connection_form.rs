use super::backend;
use super::i18n::t;
use super::theme::{hsla, Colors};
use crate::drivers::driver_trait::{ConnectionFieldOverride, PluginManifest};
use crate::models::{ConnectionParams, DatabaseSelection, SavedConnection};
use crate::runtime::{state::ApplicationState, RuntimeContext};
use gpui::{
    div, prelude::FluentBuilder as _, AppContext as _, ClickEvent, Context, Entity, EventEmitter,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_component::{Disableable as _, IndexPath};
use std::sync::Arc;

/// Which connection inputs a driver shows, decided by its capabilities like the Tauri modal.
#[derive(Default, Debug, PartialEq)]
pub struct Fields {
    pub host: bool,
    pub port: bool,
    pub username: bool,
    pub password: bool,
    pub database: bool,
    pub path: bool,
}

fn visible(field: Option<&ConnectionFieldOverride>) -> bool {
    !field.is_some_and(|field| field.hidden)
}

pub fn fields(driver: &PluginManifest, saved: Option<&ConnectionParams>) -> Fields {
    let caps = &driver.capabilities;
    if caps.no_connection_required {
        return Fields::default();
    }
    if caps.file_based || caps.folder_based {
        return Fields {
            path: true,
            ..Fields::default()
        };
    }
    let overrides = driver.connection_fields.clone().unwrap_or_default();
    let uri = caps.connection_uri;
    // Multi-database connections pick databases on a tab this form does not have.
    let multi_database = !caps.schemas
        || saved.is_some_and(|params| match &params.database {
            DatabaseSelection::Multiple(_) => true,
            DatabaseSelection::Single(name) => name.is_empty(),
        });
    Fields {
        host: !uri && visible(overrides.host.as_ref()),
        port: !uri && visible(overrides.port.as_ref()),
        username: !uri && visible(overrides.username.as_ref()),
        password: visible(overrides.password.as_ref()),
        database: !uri
            && !caps.single_database
            && !multi_database
            && visible(overrides.database.as_ref()),
        path: false,
    }
}

#[derive(Default)]
pub struct Values {
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub read_only: bool,
}

/// The i18n key of the first reason the form cannot be saved.
pub fn save_error(
    driver: &PluginManifest,
    saved: Option<&ConnectionParams>,
    values: &Values,
) -> Option<&'static str> {
    let caps = &driver.capabilities;
    let fields = fields(driver, saved);
    if values.name.trim().is_empty() {
        Some("newConnection.nameRequired")
    } else if caps.connection_uri
        && !saved.is_some_and(|params| params.connection_uri_in_keychain == Some(true))
    {
        Some("newConnection.connectionUriRequired")
    } else if (fields.database || fields.path) && values.database.trim().is_empty() {
        Some("newConnection.dbNameRequired")
    } else if values.read_only && !caps.readonly && !caps.read_only_queries {
        Some("readOnly.unsupported")
    } else {
        None
    }
}

/// Edits start from the saved (redacted) params so settings this form lacks are kept.
pub fn params(
    driver: &PluginManifest,
    saved: Option<&ConnectionParams>,
    values: &Values,
) -> ConnectionParams {
    let fields = fields(driver, saved);
    let mut params = saved.cloned().unwrap_or_else(|| ConnectionParams {
        driver: driver.id.clone(),
        save_in_keychain: Some(true),
        database: DatabaseSelection::Single(if driver.capabilities.single_database {
            driver.id.clone()
        } else {
            String::new()
        }),
        ..ConnectionParams::default()
    });
    if fields.host {
        params.host = Some(values.host.clone());
    }
    if fields.port {
        params.port = values.port.trim().parse().ok();
    }
    if fields.username {
        params.username = Some(values.username.clone());
    }
    if fields.password {
        // An omitted password keeps the stored secret on update, as in the Tauri modal.
        params.password = (!values.password.trim().is_empty()).then(|| values.password.clone());
    }
    if fields.database || fields.path {
        params.database = DatabaseSelection::Single(values.database.trim().to_string());
    }
    params.read_only = values.read_only;
    params
}

pub enum FormEvent {
    Saved { plain_text: bool },
    Cancelled,
}

#[derive(Clone)]
struct DriverItem {
    id: String,
    name: SharedString,
}

impl SelectItem for DriverItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &String {
        &self.id
    }
}

pub struct ConnectionForm {
    colors: Colors,
    context: RuntimeContext,
    state: Arc<ApplicationState>,
    runtime: tokio::runtime::Handle,
    drivers: Vec<PluginManifest>,
    driver: usize,
    saved: Option<SavedConnection>,
    picker: Entity<SelectState<Vec<DriverItem>>>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    database: Entity<InputState>,
    read_only: bool,
    message: Option<Result<String, String>>,
    busy: bool,
}

impl EventEmitter<FormEvent> for ConnectionForm {}

fn input(
    value: String,
    window: &mut Window,
    cx: &mut Context<ConnectionForm>,
) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).default_value(value))
}

impl ConnectionForm {
    /// `driver` indexes `drivers`; for an edit it must be the saved connection's driver.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        colors: Colors,
        context: RuntimeContext,
        state: Arc<ApplicationState>,
        runtime: tokio::runtime::Handle,
        drivers: Vec<PluginManifest>,
        driver: usize,
        saved: Option<SavedConnection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let items = drivers
            .iter()
            .map(|driver| DriverItem {
                id: driver.id.clone(),
                name: driver.name.clone().into(),
            })
            .collect();
        let picker = cx.new(|cx| SelectState::new(items, Some(IndexPath::new(driver)), window, cx));
        cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &SelectEvent<_>, window, cx| {
                let SelectEvent::Confirm(Some(id)) = event else {
                    return;
                };
                if let Some(index) = this.drivers.iter().position(|driver| &driver.id == id) {
                    this.driver = index;
                    this.reset(String::new(), window, cx);
                }
            },
        )
        .detach();
        let params = saved.as_ref().map(|saved| saved.params.clone());
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        let database = match params.as_ref().map(|params| &params.database) {
            Some(DatabaseSelection::Single(name)) => name.clone(),
            _ => String::new(),
        };
        let mut form = Self {
            name: input(
                saved
                    .as_ref()
                    .map(|saved| saved.name.clone())
                    .unwrap_or_default(),
                window,
                cx,
            ),
            host: input(
                params.as_ref().map(|p| text(&p.host)).unwrap_or_default(),
                window,
                cx,
            ),
            port: input(
                params
                    .as_ref()
                    .and_then(|p| p.port)
                    .map(|port| port.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            ),
            username: input(
                params
                    .as_ref()
                    .map(|p| text(&p.username))
                    .unwrap_or_default(),
                window,
                cx,
            ),
            password: cx.new(|cx| InputState::new(window, cx).masked(true)),
            database: input(database, window, cx),
            read_only: params.as_ref().is_some_and(|p| p.read_only),
            colors,
            context,
            state,
            runtime,
            drivers,
            driver,
            saved,
            picker,
            message: None,
            busy: false,
        };
        if form.saved.is_none() {
            form.reset("localhost".into(), window, cx);
        } else {
            form.set_placeholders(window, cx);
        }
        form
    }

    fn manifest(&self) -> &PluginManifest {
        &self.drivers[self.driver]
    }

    fn reset(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let driver = self.manifest();
        let port = driver
            .default_port
            .map(|port| port.to_string())
            .unwrap_or_default();
        let username = driver.default_username.clone();
        for (state, value) in [
            (&self.host, host),
            (&self.port, port),
            (&self.username, username),
            (&self.password, String::new()),
            (&self.database, String::new()),
        ] {
            state.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        self.read_only = false;
        self.message = None;
        self.set_placeholders(window, cx);
        cx.notify();
    }

    fn set_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let driver = self.manifest();
        let overrides = driver.connection_fields.clone().unwrap_or_default();
        let placeholder = |field: Option<ConnectionFieldOverride>, fallback: String| {
            field
                .and_then(|field| field.placeholder)
                .unwrap_or(fallback)
        };
        let path = if driver.capabilities.folder_based {
            "newConnection.folderPathPlaceholder"
        } else if driver.capabilities.file_based {
            "newConnection.filePathPlaceholder"
        } else {
            "newConnection.dbNamePlaceholder"
        };
        let password = if self.saved.is_some() {
            "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}".to_string()
        } else {
            placeholder(
                overrides.password,
                t("newConnection.passwordPlaceholder").into(),
            )
        };
        for (state, value) in [
            (&self.host, placeholder(overrides.host, "localhost".into())),
            (
                &self.port,
                placeholder(
                    overrides.port,
                    driver
                        .default_port
                        .map(|port| port.to_string())
                        .unwrap_or_default(),
                ),
            ),
            (
                &self.username,
                placeholder(
                    overrides.username,
                    t("newConnection.usernamePlaceholder").into(),
                ),
            ),
            (&self.password, password),
            (
                &self.database,
                placeholder(overrides.database, t(path).into()),
            ),
        ] {
            state.update(cx, |state, cx| state.set_placeholder(value, window, cx));
        }
    }

    fn values(&self, cx: &Context<Self>) -> Values {
        let text = |state: &Entity<InputState>| state.read(cx).value().to_string();
        Values {
            name: text(&self.name),
            host: text(&self.host),
            port: text(&self.port),
            username: text(&self.username),
            password: text(&self.password),
            database: text(&self.database),
            read_only: self.read_only,
        }
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        let saved = self.saved.as_ref();
        let params = params(self.manifest(), saved.map(|s| &s.params), &self.values(cx));
        let id = saved.map(|saved| saved.id.clone());
        let (context, state) = (self.context.clone(), self.state.clone());
        let task = self
            .runtime
            .spawn(async move { backend::test_connection(&context, &state, params, id).await });
        self.busy = true;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update(cx, |this, cx| {
                this.busy = false;
                this.message = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let values = self.values(cx);
        let saved = self.saved.clone();
        let saved_params = saved.as_ref().map(|saved| &saved.params);
        if let Some(key) = save_error(self.manifest(), saved_params, &values) {
            self.message = Some(Err(t(key).into()));
            cx.notify();
            return;
        }
        let params = params(self.manifest(), saved_params, &values);
        let wants_keychain = params.save_in_keychain == Some(true);
        let (context, state) = (self.context.clone(), self.state.clone());
        let name = values.name;
        let task = self.runtime.spawn(async move {
            match saved {
                Some(saved) => {
                    backend::update_connection(&context, &state, &saved, name, params).await
                }
                None => backend::save_connection(&context, &state, name, params).await,
            }
        });
        self.busy = true;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(saved) => cx.emit(FormEvent::Saved {
                        plain_text: wants_keychain && saved.params.save_in_keychain == Some(false),
                    }),
                    Err(error) => {
                        this.message =
                            Some(Err(format!("{}: {error}", t("newConnection.failSave"))));
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

impl Render for ConnectionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.colors;
        let driver = self.manifest();
        let caps = &driver.capabilities;
        let shown = fields(driver, self.saved.as_ref().map(|saved| &saved.params));
        let overrides = driver.connection_fields.clone().unwrap_or_default();
        let label = |field: Option<ConnectionFieldOverride>, key: &'static str| -> SharedString {
            field
                .and_then(|field| field.label)
                .map(SharedString::from)
                .unwrap_or(t(key).into())
        };
        let field = |label: SharedString, state: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(hsla(&c.text.muted))
                        .child(label.clone()),
                )
                .child(Input::new(state).aria_label(label))
        };
        let path_label = if caps.folder_based {
            "newConnection.folderPath"
        } else {
            "newConnection.filePath"
        };
        let supports_read_only = caps.readonly || caps.read_only_queries;
        let title = if self.saved.is_some() {
            "newConnection.titleEdit"
        } else {
            "newConnection.titleNew"
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .max_w(gpui::px(520.))
            .child(div().text_lg().child(t(title)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(hsla(&c.text.muted))
                            .child(t("newConnection.dbType")),
                    )
                    .child(match &self.saved {
                        None => Select::new(&self.picker)
                            .accessibility_label(t("newConnection.dbType"))
                            .into_any_element(),
                        Some(_) => div().child(driver.name.clone()).into_any_element(),
                    }),
            )
            .child(field(t("newConnection.name").into(), &self.name))
            .when(shown.host, |d| {
                d.child(field(
                    label(overrides.host.clone(), "newConnection.host"),
                    &self.host,
                ))
            })
            .when(shown.port, |d| {
                d.child(field(
                    label(overrides.port.clone(), "newConnection.port"),
                    &self.port,
                ))
            })
            .when(shown.username, |d| {
                d.child(field(
                    label(overrides.username.clone(), "newConnection.username"),
                    &self.username,
                ))
            })
            .when(shown.password, |d| {
                d.child(field(
                    label(overrides.password.clone(), "newConnection.password"),
                    &self.password,
                ))
            })
            .when(shown.database, |d| {
                d.child(field(
                    label(overrides.database.clone(), "newConnection.dbName"),
                    &self.database,
                ))
            })
            .when(shown.path, |d| {
                d.child(field(t(path_label).into(), &self.database))
            })
            .child(
                Checkbox::new("read-only")
                    .label(t("readOnly.label"))
                    .checked(self.read_only || caps.readonly)
                    .disabled(caps.readonly || (!self.read_only && !supports_read_only))
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.read_only = *checked;
                        cx.notify();
                    })),
            )
            .child(div().text_xs().text_color(hsla(&c.text.muted)).child(t(
                if supports_read_only {
                    "readOnly.description"
                } else {
                    "readOnly.unsupported"
                },
            )))
            .children(self.message.clone().map(|message| {
                let (color, text) = match message {
                    Ok(text) => (&c.accent.success, text),
                    Err(text) => (&c.accent.error, text),
                };
                div().text_sm().text_color(hsla(color)).child(text)
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("test-connection")
                            .label(t("newConnection.testConnection"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test(cx))),
                    )
                    .child(
                        Button::new("cancel-connection")
                            .ghost()
                            .label(t("common.cancel"))
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                cx.emit(FormEvent::Cancelled)
                            })),
                    )
                    .child(
                        Button::new("save-connection")
                            .primary()
                            .label(t("common.save"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save(cx))),
                    ),
            )
    }
}
