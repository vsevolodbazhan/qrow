use crate::model::{
    CatalogColumnReads, CatalogRefresh, CatalogSettings, ConnectionLifecycle, Profile,
    SharedCatalog,
};
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::ActiveTheme;
use uuid::Uuid;

pub(super) fn profile_name_is_taken(profiles: &[Profile], candidate: &Profile) -> bool {
    profiles
        .iter()
        .any(|profile| profile.id != candidate.id && profile.name == candidate.name)
}

/// Schema patterns from a form field: separated by commas, without blanks.
pub(super) fn parse_patterns(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Read the schema refresh fields into `settings`. Only an automatic
/// refresh reads the period.
/// The choices of the Schema Refresh dropdown, in their order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RefreshMode {
    Disabled,
    Manual,
    WhileConnected,
}

const REFRESH_MODES: [(RefreshMode, &str); 3] = [
    (RefreshMode::Disabled, "Disabled"),
    (RefreshMode::Manual, "Manual"),
    (RefreshMode::WhileConnected, "While connected"),
];

impl RefreshMode {
    /// What the mode does, in one line below the dropdown.
    fn help(self) -> &'static str {
        match self {
            Self::Disabled => "Never introspect schemas.",
            Self::Manual => "Introspect schemas on demand.",
            Self::WhileConnected => {
                "Introspect schemas on demand and periodically while a tab is connected."
            }
        }
    }

    pub(super) fn of(refresh: CatalogRefresh) -> Self {
        match refresh {
            CatalogRefresh::Disabled => Self::Disabled,
            CatalogRefresh::Manual => Self::Manual,
            CatalogRefresh::WhileConnected => Self::WhileConnected,
        }
    }
}

/// Read the schema refresh fields into `settings`. A mode reads only the
/// fields that it shows: Disabled reads none, and only While connected
/// reads the period.
pub(super) fn parse_refresh_policy(
    period: &str,
    timeout: &str,
    mode: RefreshMode,
    settings: &mut CatalogSettings,
) -> anyhow::Result<()> {
    if mode == RefreshMode::Disabled {
        settings.refresh = CatalogRefresh::Disabled;
        return Ok(());
    }
    settings.refresh = match mode {
        RefreshMode::Disabled | RefreshMode::Manual => CatalogRefresh::Manual,
        RefreshMode::WhileConnected => {
            settings.refresh_minutes = period.trim().parse().map_err(|_| {
                anyhow::anyhow!("Refresh period must be a whole number of minutes.")
            })?;
            CatalogRefresh::WhileConnected
        }
    };
    settings.timeout_minutes = timeout
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("Refresh timeout must be a whole number of minutes."))?;
    settings.validate()
}

pub(super) fn parse_lifecycle(
    values: &[String],
    keep_connected: bool,
    previous: &ConnectionLifecycle,
) -> anyhow::Result<ConnectionLifecycle> {
    let mut policy = previous.clone();
    if keep_connected {
        policy.keep_alive_seconds = values[1].trim().parse().map_err(|_| {
            anyhow::anyhow!("Keep-alive interval must be a whole number of seconds.")
        })?;
        anyhow::ensure!(
            policy.keep_alive_seconds > 0,
            "Keep-alive interval must be greater than zero."
        );
        policy.keep_alive_sql = values[2].trim().to_owned();
    } else {
        policy.idle_seconds = values[0]
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("Idle timeout must be a whole number of seconds."))?;
        policy.keep_alive_seconds = 0;
    }
    policy.validate()?;
    Ok(policy)
}

use super::profile_view::{connection_row, form_input};
use super::{ProfileEditor, Qrow};
use crate::model::{MAX_ASSISTANT_NOTES_BYTES, SignIn, SignInProvider};
use gpui_kit::component::{
    IconName, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    combobox::{Combobox, ComboboxState},
    h_flex,
    input::{Input, Textarea},
    label::Label,
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    setting::{SettingGroup, SettingPage},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, Role, SharedString, TestSupportExt as _,
    WeakEntity, Window, div, prelude::*,
};

/// Element IDs of the connection form inputs, by field index. Index 6 is the
/// session parameters textarea, which has no ID setter in GPUI Kit 0.6.6.
pub(super) const FIELD_IDS: [&str; 16] = [
    "connection-name",
    "connection-host",
    "connection-port",
    "connection-username",
    "connection-password",
    "connection-database",
    "connection-parameters",
    "connection-idle-timeout",
    "connection-keep-alive-interval",
    "connection-keep-alive-query",
    "connection-show-schemas",
    "connection-hide-schemas",
    "connection-refresh-period",
    "connection-refresh-timeout",
    "connection-response-timeout",
    "connection-trino-schema",
];
const PASSWORD: &str = "Password";
const SIGN_IN: &str = "Sign-in";
const DISCONNECT_AFTER: &str = "Disconnect after";
const KEEP_CONNECTED: &str = "Keep connected";
const PRIVATE_CATALOG: &str = "This connection";
const NEW_SHARED_CATALOG: &str = "New shared catalog";
const ANY_MEMBER: &str = "Any connected connection";

/// A dropdown of a few fixed choices.
pub(super) type ChoiceSelect = Entity<SelectState<SearchableVec<String>>>;

/// The schema catalog that a connection uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogChoice {
    Private,
    Shared(Uuid),
    New,
}

/// The choices of the Schema catalog list: the catalog of the connection
/// and each shared catalog. The New shared catalog button adds a new one.
pub(super) fn catalog_choices(shared: &[SharedCatalog]) -> Vec<(CatalogChoice, String)> {
    std::iter::once((CatalogChoice::Private, PRIVATE_CATALOG.to_owned()))
        .chain(
            shared
                .iter()
                .map(|catalog| (CatalogChoice::Shared(catalog.id), catalog.name.clone())),
        )
        .collect()
}

/// Add the choice of a new shared catalog, if it is not there. Returns its row.
pub(super) fn add_new_catalog(choices: &mut Vec<(CatalogChoice, String)>) -> usize {
    match choices
        .iter()
        .position(|(choice, _)| *choice == CatalogChoice::New)
    {
        Some(row) => row,
        None => {
            choices.push((CatalogChoice::New, NEW_SHARED_CATALOG.to_owned()));
            choices.len() - 1
        }
    }
}

/// Remove the choice of a new shared catalog. Returns whether it was there.
pub(super) fn remove_new_catalog(choices: &mut Vec<(CatalogChoice, String)>) -> bool {
    let count = choices.len();
    choices.retain(|(choice, _)| *choice != CatalogChoice::New);
    choices.len() != count
}

/// Whether a shared catalog can have `name`: the dropdown must tell it
/// from the other choices.
pub(super) fn shared_name_is_taken(shared: &[SharedCatalog], candidate: &SharedCatalog) -> bool {
    let name = candidate.name.trim();
    name == PRIVATE_CATALOG
        || name == NEW_SHARED_CATALOG
        || shared
            .iter()
            .any(|catalog| catalog.id != candidate.id && catalog.name.trim() == name)
}

/// The choices of the Preferred connection dropdown: any member, or one of
/// the members. `profile` is the edited one.
pub(super) fn preferred_choices(
    profile: &Profile,
    catalog: CatalogChoice,
    profiles: &[Profile],
) -> Vec<(Option<Uuid>, String)> {
    let shared = match catalog {
        CatalogChoice::Shared(id) => Some(id),
        CatalogChoice::Private | CatalogChoice::New => None,
    };
    let mut members: Vec<(Option<Uuid>, String)> = profiles
        .iter()
        // A member with browsing off keeps its place, so a save of another
        // member does not clear the preference.
        .filter(|member| {
            member.id != profile.id && shared.is_some() && member.shared_catalog == shared
        })
        .map(|member| (Some(member.id), member.name.clone()))
        .collect();
    let name = if profile.name.trim().is_empty() {
        PRIVATE_CATALOG.to_owned()
    } else {
        profile.name.clone()
    };
    match profiles.iter().position(|member| member.id == profile.id) {
        // The edited connection keeps its place in the sidebar order.
        Some(index) => {
            let before = profiles[..index]
                .iter()
                .filter(|member| members.iter().any(|(id, _)| *id == Some(member.id)))
                .count();
            members.insert(before, (Some(profile.id), name));
        }
        None => members.push((Some(profile.id), name)),
    }
    std::iter::once((None, ANY_MEMBER.to_owned()))
        .chain(members)
        .collect()
}

/// A dropdown row that knows its position, so two rows with the same
/// label stay apart.
#[derive(Clone)]
pub(super) struct Row {
    label: SharedString,
    index: usize,
    sign_in: Option<(Uuid, SignInProvider)>,
}

impl SelectItem for Row {
    type Value = usize;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &usize {
        &self.index
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some((id, provider)) = self.sign_in else {
            return self.label.clone().into_any_element();
        };
        let provider = match provider {
            SignInProvider::Oidc => "OIDC",
            SignInProvider::TrinoExternal => "Trino",
        };
        h_flex()
            .id(format!("sign-in-choice-{id}"))
            .test_support()
            .role(Role::ListItem)
            .aria_label(format!("{}, {provider}", self.label))
            .w_full()
            .items_baseline()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(Label::new(self.label.clone())),
            )
            .child(
                Label::new(provider)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .flex_shrink_0(),
            )
            .into_any_element()
    }
}

/// A dropdown of choices whose labels can repeat.
pub(super) type RowSelect = Entity<SelectState<SearchableVec<Row>>>;

/// A dropdown of choices whose labels can repeat, searchable, with room
/// for a command below the list.
pub(super) type RowCombobox = Entity<ComboboxState<SearchableVec<Row>>>;

fn rows<T>(choices: &[(T, String)]) -> SearchableVec<Row> {
    SearchableVec::new(
        choices
            .iter()
            .enumerate()
            .map(|(index, (_, label))| Row {
                label: label.clone().into(),
                index,
                sign_in: None,
            })
            .collect::<Vec<_>>(),
    )
}

fn row_of<T: PartialEq>(choices: &[(T, String)], selected: &T) -> usize {
    choices
        .iter()
        .position(|(choice, _)| choice == selected)
        .unwrap_or_default()
}

/// A dropdown of `choices` with `selected` chosen, or the first choice.
pub(super) fn choice_select<T: PartialEq>(
    choices: &[(T, String)],
    selected: &T,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> RowSelect {
    let row = row_of(choices, selected);
    let rows = rows(choices);
    cx.new(|cx| SelectState::new(rows, Some(IndexPath::default().row(row)), window, cx))
}

pub(super) fn column_read_choices() -> [(CatalogColumnReads, String); 2] {
    [
        (CatalogColumnReads::Table, "One relation at a time".into()),
        (CatalogColumnReads::Schema, "Whole schema".into()),
    ]
}

/// The Schema catalog list, with `selected` chosen.
pub(super) fn catalog_combobox(
    choices: &[(CatalogChoice, String)],
    selected: &CatalogChoice,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> RowCombobox {
    let row = row_of(choices, selected);
    let rows = rows(choices);
    cx.new(|cx| {
        ComboboxState::new(rows, vec![IndexPath::default().row(row)], window, cx).searchable(true)
    })
}

/// Give the Schema catalog list new `choices`, with `selected` chosen and
/// an empty search.
pub(super) fn set_catalog_choices(
    combobox: &RowCombobox,
    choices: &[(CatalogChoice, String)],
    selected: &CatalogChoice,
    window: &mut Window,
    cx: &mut App,
) {
    let row = row_of(choices, selected);
    combobox.update(cx, |combobox, cx| {
        combobox.set_items(rows(choices), window, cx);
        combobox.set_selected_values(&[row], window, cx);
    });
}

/// The catalog that the Schema catalog list shows.
pub(super) fn chosen_catalog(
    combobox: &RowCombobox,
    choices: &[(CatalogChoice, String)],
    cx: &App,
) -> CatalogChoice {
    let row = combobox.read(cx).selected_value().unwrap_or_default();
    choices
        .get(row)
        .map_or(CatalogChoice::Private, |(choice, _)| *choice)
}

/// The choice that `select` shows, or the first choice.
pub(super) fn chosen<T: Clone>(select: &RowSelect, choices: &[(T, String)], cx: &App) -> T {
    let row = select
        .read(cx)
        .selected_value()
        .copied()
        .unwrap_or_default();
    choices.get(row).unwrap_or(&choices[0]).0.clone()
}

/// The Authentication or Sign-in dropdown of a connection.
pub(super) type AuthenticationSelect = ChoiceSelect;

pub(super) fn authentication_select(
    authentication: crate::model::Authentication,
    database_type: crate::model::DatabaseType,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> AuthenticationSelect {
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(authentication_choices(database_type, authentication)),
            Some(IndexPath::default().row(usize::from(
                authentication != crate::model::Authentication::Password,
            ))),
            window,
            cx,
        )
    })
}

const EXTERNAL: &str = "External";

fn authentication_choices(
    database_type: crate::model::DatabaseType,
    authentication: crate::model::Authentication,
) -> Vec<String> {
    if database_type == crate::model::DatabaseType::Trino {
        let mut choices = vec![PASSWORD.into(), EXTERNAL.into()];
        // Preserve previously saved direct OIDC connections without offering
        // that setup to new Trino connections.
        if matches!(authentication, crate::model::Authentication::Oidc { .. }) {
            choices.push(SIGN_IN.into());
        }
        choices
    } else {
        vec![PASSWORD.into(), SIGN_IN.into()]
    }
}

pub(super) fn reset_authentication(
    select: &AuthenticationSelect,
    database_type: crate::model::DatabaseType,
    window: &mut Window,
    cx: &mut App,
) {
    select.update(cx, |select, cx| {
        select.set_items(
            SearchableVec::new(authentication_choices(
                database_type,
                crate::model::Authentication::Password,
            )),
            window,
            cx,
        );
        select.set_selected_value(&PASSWORD.to_owned(), window, cx);
    });
}

pub(super) fn uses_external(select: &AuthenticationSelect, cx: &App) -> bool {
    select
        .read(cx)
        .selected_value()
        .is_some_and(|choice| choice == EXTERNAL)
}

pub(super) fn uses_sign_in_from_event(event: &SelectEvent<SearchableVec<String>>) -> Option<bool> {
    let SelectEvent::Confirm(Some(choice)) = event else {
        return None;
    };
    match choice.as_str() {
        PASSWORD | EXTERNAL => Some(false),
        SIGN_IN => Some(true),
        _ => None,
    }
}

pub(super) fn uses_sign_in(select: &AuthenticationSelect, cx: &App) -> bool {
    select
        .read(cx)
        .selected_value()
        .is_some_and(|choice| choice == SIGN_IN)
}

/// The choices of the Sign-in list: each sign-in by name. Names are unique.
pub(super) fn sign_in_choices(
    sign_ins: &[SignIn],
    _database_type: crate::model::DatabaseType,
) -> Vec<(Uuid, String)> {
    sign_ins
        .iter()
        .filter(|sign_in| sign_in.provider == crate::model::SignInProvider::Oidc)
        .map(|sign_in| (sign_in.id, sign_in.name.clone()))
        .collect()
}

/// The rows of the Sign-in list that show `selected`, if it is a choice.
fn sign_in_rows(choices: &[(Uuid, String)], selected: Option<Uuid>) -> Vec<usize> {
    selected
        .and_then(|id| choices.iter().position(|(choice, _)| *choice == id))
        .into_iter()
        .collect()
}

/// The Sign-in list, with `selected` chosen. A list without a choice shows
/// its placeholder.
pub(super) fn sign_in_combobox(
    choices: &[(Uuid, String)],
    sign_ins: &[SignIn],
    selected: Option<Uuid>,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> RowCombobox {
    let selection = sign_in_rows(choices, selected)
        .into_iter()
        .map(|row| IndexPath::default().row(row))
        .collect();
    let rows = SearchableVec::new(
        choices
            .iter()
            .enumerate()
            .map(|(index, (id, name))| Row {
                label: name.clone().into(),
                index,
                sign_in: sign_ins
                    .iter()
                    .find(|sign_in| sign_in.id == *id)
                    .map(|sign_in| (*id, sign_in.provider)),
            })
            .collect::<Vec<_>>(),
    );
    cx.new(|cx| ComboboxState::new(rows, selection, window, cx).searchable(true))
}

/// The sign-in that the Sign-in list shows, if any.
pub(super) fn chosen_sign_in(
    combobox: &RowCombobox,
    choices: &[(Uuid, String)],
    cx: &App,
) -> Option<Uuid> {
    let row = combobox.read(cx).selected_value()?;
    choices.get(row).map(|(id, _)| *id)
}

pub(super) fn idle_behavior_select(
    keep_connected: bool,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> ChoiceSelect {
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(vec![DISCONNECT_AFTER.into(), KEEP_CONNECTED.into()]),
            Some(IndexPath::default().row(usize::from(keep_connected))),
            window,
            cx,
        )
    })
}

/// The Schema refresh dropdown, with `mode` selected.
pub(super) fn schema_refresh_select(
    mode: RefreshMode,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> ChoiceSelect {
    let row = REFRESH_MODES
        .iter()
        .position(|(choice, _)| *choice == mode)
        .unwrap_or_default();
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(
                REFRESH_MODES
                    .iter()
                    .map(|(_, label)| (*label).to_owned())
                    .collect::<Vec<_>>(),
            ),
            Some(IndexPath::default().row(row)),
            window,
            cx,
        )
    })
}

pub(super) fn refresh_mode(select: &ChoiceSelect, cx: &App) -> RefreshMode {
    let selected = select.read(cx).selected_value().cloned();
    REFRESH_MODES
        .iter()
        .find(|(_, label)| selected.as_deref() == Some(*label))
        .map_or(RefreshMode::Disabled, |(mode, _)| *mode)
}

pub(super) fn keep_connected_from_event(
    event: &SelectEvent<SearchableVec<String>>,
) -> Option<bool> {
    let SelectEvent::Confirm(Some(choice)) = event else {
        return None;
    };
    match choice.as_str() {
        DISCONNECT_AFTER => Some(false),
        KEEP_CONNECTED => Some(true),
        _ => None,
    }
}

pub(super) fn keeps_connected(select: &ChoiceSelect, cx: &App) -> bool {
    select
        .read(cx)
        .selected_value()
        .is_some_and(|choice| choice == KEEP_CONNECTED)
}

/// The notes length from which the form shows the byte count.
const NOTES_COUNT_FROM: usize = MAX_ASSISTANT_NOTES_BYTES * 3 / 4;

/// Whether the form shows the byte count of notes of `bytes`.
pub(super) fn counts_notes(bytes: usize) -> bool {
    bytes >= NOTES_COUNT_FROM
}

/// The help text below the notes: the byte count near the limit.
fn notes_description(bytes: usize) -> String {
    if counts_notes(bytes) {
        format!(
            "{} of {} bytes. Do not enter secrets.",
            group_digits(bytes),
            group_digits(MAX_ASSISTANT_NOTES_BYTES)
        )
    } else {
        "Sent to the assistant and saved as plain text. Do not enter secrets.".to_owned()
    }
}

fn group_digits(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The notes that the assistant reads about the connection.
pub(super) fn assistant_page(qrow: &WeakEntity<Qrow>) -> SettingPage {
    SettingPage::new("Assistant")
        .resettable(false)
        .group(SettingGroup::new().item(connection_row(
            qrow,
            "Assistant Notes",
            "Sent to the assistant and saved as plain text. Do not enter secrets.",
            &["assistant", "notes", "codex", "claude", "context"],
            true,
            |_, form, _, cx| {
                let bytes = form.assistant_notes.read(cx).value().len();
                // The textarea has no element ID setter in GPUI Kit 0.6.6, so
                // this element gives tests one.
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div()
                            .id("connection-assistant-notes")
                            .test_support()
                            .w_full()
                            .child(
                                Textarea::new(&form.assistant_notes)
                                    .focus_ring(false)
                                    .w_full()
                                    .disabled(form.saving.is_some())
                                    .aria_label("Assistant Notes"),
                            ),
                    )
                    .when(counts_notes(bytes), |notes| {
                        notes.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(notes_description(bytes)),
                        )
                    })
                    .into_any_element()
            },
        )))
}

/// The schema browsing of the connection: the refresh, the shared catalog,
/// and the schemas that the tree shows.
pub(super) fn catalog_page(form: &ProfileEditor, qrow: &WeakEntity<Qrow>, cx: &App) -> SettingPage {
    let mode = refresh_mode(&form.schema_refresh, cx);
    let catalog = chosen_catalog(&form.catalog_select, &form.catalog_choices, cx);
    let shared = catalog != CatalogChoice::Private;
    let browses = mode != RefreshMode::Disabled;
    let fields = SettingGroup::new()
        .item(connection_row(
            qrow,
            "Schema Refresh",
            mode.help(),
            &["schemas", "introspect", "catalog", "browse"],
            false,
            |_, form, _, _| {
                Select::new(&form.schema_refresh)
                    .focus_ring(false)
                    .id("connection-schema-refresh")
                    .w_full()
                    .disabled(form.saving.is_some())
                    .accessibility_label("Schema Refresh")
                    .into_any_element()
            },
        ))
        .items((mode == RefreshMode::WhileConnected).then(|| {
            connection_row(
                qrow,
                "Refresh Period",
                "Minutes between automatic schema refreshes, from 5 to 10080.",
                &["schemas", "minutes", "automatic"],
                false,
                |_, form, _, _| form_input(form, 12, "Refresh Period in Minutes"),
            )
        }))
        .items(browses.then(|| {
            connection_row(
                qrow,
                "Refresh Timeout",
                "Minutes before a schema refresh stops, from 1 to 1440.",
                &["schemas", "minutes"],
                false,
                |_, form, _, _| form_input(form, 13, "Refresh Timeout in Minutes"),
            )
        }))
        .items(browses.then(|| {
            connection_row(
                qrow,
                "Column Reads",
                "Per-relation reads use less driver memory; schema reads send fewer requests.",
                &["columns", "memory", "driver", "requests"],
                false,
                |_, form, _, _| {
                    Select::new(&form.column_reads)
                        .focus_ring(false)
                        .id("connection-column-reads")
                        .w_full()
                        .disabled(form.saving.is_some())
                        .accessibility_label("Column Reads")
                        .into_any_element()
                },
            )
        }))
        .items(browses.then(|| connection_row(
            qrow,
            "Schema Catalog",
            if shared {
                "Connections that share this catalog must read the same metastore with the same permissions."
            } else {
                "Connections that read the same metastore can share one catalog."
            },
            &["shared", "catalog", "metastore"],
            false,
            |_, form, _, cx| catalog_field(form, cx),
        )))
        .items(if browses && shared { vec![connection_row(
                    qrow,
                    "Shared Catalog Name",
                    "Shown in the settings of each connection that uses the catalog.",
                    &["shared", "catalog"],
                    false,
                    |_, form, _, _| {
                        Input::new(&form.shared_name)
                            .focus_ring(false)
                            .id("connection-shared-catalog-name")
                            .w_full()
                            .disabled(form.saving.is_some())
                            .aria_label("Shared Catalog Name")
                            .into_any_element()
                    },
                ), connection_row(
                    qrow,
                    "Preferred Connection",
                    "Automatic refreshes use this connection first while it is connected.",
                    &["shared", "catalog", "refresh"],
                    false,
                    |_, form, _, _| {
                        Select::new(&form.preferred_select)
                            .focus_ring(false)
                            .id("connection-preferred-catalog-connection")
                            .w_full()
                            .disabled(form.saving.is_some())
                            .accessibility_label("Preferred Connection")
                            .into_any_element()
                    },
                )] } else { vec![] })
        .items(browses.then(|| connection_row(
            qrow,
            "Visible Schemas",
            "Patterns separated by commas, for example sales_*. Empty shows all schemas.",
            &["schemas", "filter", "patterns", "include"],
            false,
            |_, form, _, _| form_input(form, 10, "Visible Schemas"),
        )))
        .items(browses.then(|| connection_row(
            qrow,
            "Hidden Schemas",
            "Patterns separated by commas. Hides a schema also when Visible Schemas matches it.",
            &["schemas", "filter", "patterns", "exclude"],
            false,
            |_, form, _, _| form_input(form, 11, "Hidden Schemas"),
        )));
    SettingPage::new("Catalog").resettable(false).group(fields)
}

/// The list of schema catalogs, with a button that adds a shared catalog.
fn catalog_field(form: &ProfileEditor, cx: &mut Context<Qrow>) -> AnyElement {
    let catalog = chosen_catalog(&form.catalog_select, &form.catalog_choices, cx);
    let label = form
        .catalog_choices
        .iter()
        .find(|(choice, _)| *choice == catalog)
        .map(|(_, label)| label.clone())
        .unwrap_or_default();
    let has_new = form
        .catalog_choices
        .iter()
        .any(|(choice, _)| *choice == CatalogChoice::New);
    let qrow = cx.weak_entity();
    // The combobox has no accessibility of its own in GPUI Kit 0.6.6, so this
    // element names it and gives its value.
    div()
        .id("connection-schema-catalog")
        .test_support()
        .role(Role::ComboBox)
        .aria_label("Schema Catalog")
        .aria_value(label)
        .w_full()
        .child(
            Combobox::new(&form.catalog_select)
                .focus_ring(false)
                .w_full()
                .disabled(form.saving.is_some())
                .search_placeholder("Search catalogs…")
                // One save makes at most one new shared catalog.
                .when(!has_new, |list| {
                    list.footer(move |_, _| {
                        let qrow = qrow.clone();
                        Button::new("connection-new-shared-catalog")
                            .ghost()
                            .small()
                            .w_full()
                            .justify_start()
                            .icon(IconName::Plus)
                            .label("New shared catalog…")
                            .on_click(move |_, window, cx| {
                                let _ =
                                    qrow.update(cx, |this, cx| this.new_shared_catalog(window, cx));
                            })
                    })
                }),
        )
        .into_any_element()
}

pub(super) fn database_type_choices() -> Vec<(crate::model::DatabaseType, String)> {
    use crate::model::DatabaseType;
    [
        DatabaseType::Kyuubi,
        DatabaseType::Postgres,
        DatabaseType::Trino,
    ]
    .into_iter()
    .map(|kind| (kind, kind.label().into()))
    .collect()
}

pub(super) fn postgres_ssl_mode_choices() -> Vec<(crate::model::PostgresSslMode, String)> {
    use crate::model::PostgresSslMode;
    vec![
        (PostgresSslMode::Disable, "Disabled".into()),
        (PostgresSslMode::Require, "Require TLS".into()),
        (PostgresSslMode::VerifyFull, "Verify certificate".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notes_help_counts_bytes_near_the_limit() {
        assert!(notes_description(0).starts_with("Sent to the assistant"));
        assert!(notes_description(NOTES_COUNT_FROM - 1).starts_with("Sent to the assistant"));
        assert_eq!(
            notes_description(NOTES_COUNT_FROM),
            "12,288 of 16,384 bytes. Do not enter secrets."
        );
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn schema_patterns_are_split_at_commas_without_blanks() {
        assert_eq!(parse_patterns(" sales_*, ,ops ,"), ["sales_*", "ops"]);
        assert!(parse_patterns("  ").is_empty());
    }

    #[test]
    fn each_schema_refresh_mode_reads_only_the_fields_that_it_shows() {
        let mut settings = CatalogSettings::default();
        parse_refresh_policy("invalid", " 10 ", RefreshMode::Manual, &mut settings).unwrap();
        assert_eq!(settings.refresh, CatalogRefresh::Manual);
        assert_eq!(settings.timeout_minutes, 10);
        parse_refresh_policy(" 15 ", "10", RefreshMode::WhileConnected, &mut settings).unwrap();
        assert_eq!(settings.refresh, CatalogRefresh::WhileConnected);
        assert_eq!(settings.refresh_minutes, 15);
        // Disabled and Manual keep the hidden period, and Disabled keeps the
        // hidden timeout.
        parse_refresh_policy("invalid", "invalid", RefreshMode::Disabled, &mut settings).unwrap();
        assert_eq!(settings.refresh, CatalogRefresh::Disabled);
        assert_eq!(settings.timeout_minutes, 10);
        assert_eq!(settings.refresh_minutes, 15);
        parse_refresh_policy("invalid", "10", RefreshMode::Manual, &mut settings).unwrap();
        assert_eq!(settings.refresh_minutes, 15);
        let mode = RefreshMode::WhileConnected;
        assert!(parse_refresh_policy("invalid", "10", mode, &mut settings).is_err());
        assert!(parse_refresh_policy("4", "10", mode, &mut settings).is_err());
        assert!(parse_refresh_policy("15", "0", mode, &mut settings).is_err());
        assert!(parse_refresh_policy("15", "1.5", RefreshMode::Manual, &mut settings).is_err());
        assert_eq!(
            RefreshMode::of(CatalogRefresh::WhileConnected),
            RefreshMode::WhileConnected
        );
    }

    #[test]
    fn connection_names_are_unique_except_for_the_profile_being_edited() {
        let existing = Profile {
            name: "Analytics".into(),
            ..Profile::default()
        };
        let unchanged = existing.clone();
        let duplicate = Profile {
            name: existing.name.clone(),
            ..Profile::default()
        };

        assert!(!profile_name_is_taken(
            std::slice::from_ref(&existing),
            &unchanged
        ));
        assert!(profile_name_is_taken(&[existing], &duplicate));
    }

    #[test]
    fn only_the_selected_modes_fields_are_validated() {
        let previous = ConnectionLifecycle::default();
        let idle = parse_lifecycle(
            &["60".into(), "invalid".into(), "".into()],
            false,
            &previous,
        )
        .unwrap();
        assert_eq!(idle.idle_seconds, 60);
        assert_eq!(idle.keep_alive_seconds, 0);
        let keep = parse_lifecycle(
            &["invalid".into(), "30".into(), "SELECT 2".into()],
            true,
            &previous,
        )
        .unwrap();
        assert_eq!(keep.keep_alive_seconds, 30);
        assert_eq!(keep.keep_alive_sql, "SELECT 2");
        assert!(
            parse_lifecycle(
                &["60".into(), "0".into(), "SELECT 1".into()],
                true,
                &previous
            )
            .is_err()
        );
    }

    #[test]
    fn authentication_choices_map_to_methods() {
        assert_eq!(
            uses_sign_in_from_event(&SelectEvent::Confirm(Some(PASSWORD.into()))),
            Some(false)
        );
        assert_eq!(
            uses_sign_in_from_event(&SelectEvent::Confirm(Some(SIGN_IN.into()))),
            Some(true)
        );
        assert_eq!(uses_sign_in_from_event(&SelectEvent::Confirm(None)), None);
    }

    #[test]
    fn dropdown_choices_map_to_lifecycle_modes() {
        assert!(
            !keep_connected_from_event(&SelectEvent::Confirm(Some(DISCONNECT_AFTER.into())))
                .unwrap()
        );
        assert!(
            keep_connected_from_event(&SelectEvent::Confirm(Some(KEEP_CONNECTED.into()))).unwrap()
        );
        assert!(keep_connected_from_event(&SelectEvent::Confirm(Some("Unknown".into()))).is_none());
        assert!(keep_connected_from_event(&SelectEvent::Confirm(None)).is_none());
    }

    #[test]
    fn catalog_choices_name_each_shared_catalog_and_reject_ambiguous_names() {
        let lake = SharedCatalog {
            id: Uuid::new_v4(),
            name: "Lake".into(),
            settings: CatalogSettings::default(),
            preferred: None,
        };
        let mut choices = catalog_choices(std::slice::from_ref(&lake));
        let labels: Vec<_> = choices.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(labels, [PRIVATE_CATALOG, "Lake"]);
        assert_eq!(choices[1].0, CatalogChoice::Shared(lake.id));
        // The New shared catalog button adds its choice once.
        assert_eq!(add_new_catalog(&mut choices), 2);
        assert_eq!(add_new_catalog(&mut choices), 2);
        assert_eq!(
            choices[2],
            (CatalogChoice::New, NEW_SHARED_CATALOG.to_owned())
        );
        // Another choice removes it, and the button shows again.
        assert!(remove_new_catalog(&mut choices));
        assert!(!remove_new_catalog(&mut choices));
        assert_eq!(choices.len(), 2);

        let shared = std::slice::from_ref(&lake);
        assert!(!shared_name_is_taken(shared, &lake));
        for name in [" Lake ", PRIVATE_CATALOG, NEW_SHARED_CATALOG] {
            let candidate = SharedCatalog {
                id: Uuid::new_v4(),
                name: name.into(),
                ..lake.clone()
            };
            assert!(shared_name_is_taken(shared, &candidate), "{name}");
        }
    }

    #[test]
    fn preferred_choices_list_the_members_in_sidebar_order() {
        let catalog = Uuid::new_v4();
        let member = |name: &str, refresh: CatalogRefresh| Profile {
            name: name.into(),
            catalog: CatalogSettings {
                refresh,
                ..CatalogSettings::default()
            },
            shared_catalog: Some(catalog),
            ..Profile::default()
        };
        let first = member("first", CatalogRefresh::Manual);
        let edited = member("edited", CatalogRefresh::Manual);
        let off = member("off", CatalogRefresh::Disabled);
        let last = member("last", CatalogRefresh::Manual);
        let profiles = [first, edited.clone(), off, last.clone()];

        let choices = preferred_choices(&edited, CatalogChoice::Shared(catalog), &profiles);
        let labels: Vec<_> = choices.iter().map(|(_, label)| label.as_str()).collect();
        // A member with browsing off stays, so its preference stays.
        assert_eq!(labels, [ANY_MEMBER, "first", "edited", "off", "last"]);
        assert_eq!(choices[0].0, None);
        assert_eq!(choices[4].0, Some(last.id));

        // A new catalog has only the edited connection, with a name.
        let unnamed = Profile {
            name: String::new(),
            ..Profile::default()
        };
        let choices = preferred_choices(&unnamed, CatalogChoice::New, &profiles);
        let labels: Vec<_> = choices.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(labels, [ANY_MEMBER, PRIVATE_CATALOG]);
    }
}
