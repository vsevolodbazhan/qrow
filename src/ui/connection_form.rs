use crate::model::{CatalogRefresh, CatalogSettings, ConnectionLifecycle, Profile, SharedCatalog};
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
/// The choices of the Schema refresh dropdown, in their order.
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
            Self::Disabled => "Qrow does not read or show schemas.",
            Self::Manual => "Qrow reads schemas when you refresh them.",
            Self::WhileConnected => "Qrow also refreshes schemas while a tab is connected.",
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

use super::{ProfileEditor, Qrow};
use gpui_kit::component::{
    IconName, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    combobox::{Combobox, ComboboxState},
    form::{Field, Form},
    input::Input,
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, Role, SharedString, TestSupportExt as _,
    WeakEntity, Window, div, prelude::*,
};

/// Element IDs of the connection form inputs, by field index. Index 6 is the
/// session parameters textarea, which has no ID setter in GPUI Kit 0.6.6.
pub(super) const FIELD_IDS: [&str; 14] = [
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
];
const DISCONNECT_AFTER: &str = "Disconnect after";
const KEEP_CONNECTED: &str = "Keep connected";
const DISABLED: &str = "Disabled";
const PRIVATE_CATALOG: &str = "This connection";
const NEW_SHARED_CATALOG: &str = "New shared catalog";
const ANY_MEMBER: &str = "Any connected connection";
const ENABLED: &str = "Enabled";

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
}

impl SelectItem for Row {
    type Value = usize;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &usize {
        &self.index
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

/// A choice between Disabled and Enabled.
pub(super) fn enabled_select(
    enabled: bool,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> ChoiceSelect {
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(vec![DISABLED.into(), ENABLED.into()]),
            Some(IndexPath::default().row(usize::from(enabled))),
            window,
            cx,
        )
    })
}

pub(super) fn is_enabled(select: &ChoiceSelect, cx: &App) -> bool {
    select
        .read(cx)
        .selected_value()
        .is_some_and(|choice| choice == ENABLED)
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

fn field(label: &'static str, description: Option<&'static str>, control: AnyElement) -> Field {
    Field::new()
        .label(label)
        .child(control)
        .when_some(description, |field, description| {
            field.description(description)
        })
}

pub(super) fn render_lifecycle(form: &ProfileEditor, cx: &mut Context<Qrow>) -> impl IntoElement {
    let saving = form.saving.is_some();
    let keep = keeps_connected(&form.idle_behavior, cx);
    let input = |index: usize, label: &'static str| {
        Input::new(&form.fields[index])
            .id(FIELD_IDS[index])
            .w_full()
            .disabled(saving)
            .aria_label(label)
            .into_any_element()
    };

    Form::vertical()
        .w_full()
        .child(field(
            "When idle",
            Some(
                "Releasing the session keeps SQL and downloaded results. It drops temporary views, session settings, and unfetched rows.",
            ),
            Select::new(&form.idle_behavior)
                .id("connection-idle-behavior")
                .w_full()
                .disabled(saving)
                .accessibility_label("When idle")
                .into_any_element(),
        ))
        .when(!keep, |el| {
            el.child(field(
                "Idle timeout",
                Some("Seconds of inactivity before Qrow releases the session."),
                input(7, "Idle timeout in seconds"),
            ))
        })
        .when(keep, |el| {
            el.child(field(
                "Keep-alive interval",
                Some("Seconds between keep-alive queries."),
                input(8, "Keep-alive interval in seconds"),
            ))
            .child(field(
                "Keep-alive query",
                Some(
                    "Runs only while idle and keeps the engine active. Use a lightweight, read-only query.",
                ),
                input(9, "Keep-alive query"),
            ))
        })
}

pub(super) fn render_schemas(
    form: &ProfileEditor,
    qrow: WeakEntity<Qrow>,
    cx: &App,
) -> impl IntoElement {
    let saving = form.saving.is_some();
    let mode = refresh_mode(&form.schema_refresh, cx);
    let catalog = chosen_catalog(&form.catalog_select, &form.catalog_choices, cx);
    let catalog_label = form
        .catalog_choices
        .iter()
        .find(|(choice, _)| *choice == catalog)
        .map(|(_, label)| label.clone())
        .unwrap_or_default();
    let has_new = form
        .catalog_choices
        .iter()
        .any(|(choice, _)| *choice == CatalogChoice::New);
    let shared = catalog != CatalogChoice::Private;
    let input = |index: usize, label: &'static str| {
        Input::new(&form.fields[index])
            .id(FIELD_IDS[index])
            .w_full()
            .disabled(saving)
            .aria_label(label)
            .into_any_element()
    };
    Form::vertical()
        .w_full()
        .child(field(
            "Schema refresh",
            Some(mode.help()),
            Select::new(&form.schema_refresh)
                .id("connection-schema-refresh")
                .w_full()
                .disabled(saving)
                .accessibility_label("Schema refresh")
                .into_any_element(),
        ))
        .when(mode != RefreshMode::Disabled, |el| {
            el.child(field(
                "Schema catalog",
                Some(if shared {
                    "Its refresh and schema settings apply to each connection that uses it. These connections must read the same metastore with the same permissions. Qrow cannot check this."
                } else {
                    "Only this connection uses the catalog. Connections that read the same metastore can share one."
                }),
                // The combobox has no accessibility of its own in GPUI Kit
                // 0.6.6, so this element names it and gives its value.
                div()
                    .id("connection-schema-catalog")
                    .test_support()
                    .role(Role::ComboBox)
                    .aria_label("Schema catalog")
                    .aria_value(catalog_label)
                    .w_full()
                    .child(
                        Combobox::new(&form.catalog_select)
                            .w_full()
                            .disabled(saving)
                            .search_placeholder("Search catalogs…")
                            // One save makes at most one new shared catalog.
                            .when(!has_new, |list| list.footer(move |_, _| {
                                let qrow = qrow.clone();
                                Button::new("connection-new-shared-catalog")
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::Plus)
                                    .label("New shared catalog…")
                                    .on_click(move |_, window, cx| {
                                        let _ = qrow.update(cx, |this, cx| {
                                            this.new_shared_catalog(window, cx)
                                        });
                                    })
                            })),
                    )
                    .into_any_element(),
            ))
        })
        .when(mode != RefreshMode::Disabled && shared, |el| {
            el.child(field(
                "Shared catalog name",
                Some("Shown in the settings of each connection that uses the catalog."),
                Input::new(&form.shared_name)
                    .id("connection-shared-catalog-name")
                    .w_full()
                    .disabled(saving)
                    .aria_label("Shared catalog name")
                    .into_any_element(),
            ))
            .child(field(
                "Preferred connection",
                Some("Automatic refreshes use this connection while one of its tabs is connected. Otherwise they use another connected connection."),
                Select::new(&form.preferred_select)
                    .id("connection-preferred-catalog-connection")
                    .w_full()
                    .disabled(saving)
                    .accessibility_label("Preferred connection")
                    .into_any_element(),
            ))
        })
        .when(mode == RefreshMode::WhileConnected, |el| {
            el.child(field(
                "Refresh period",
                Some("Minutes between automatic schema refreshes, from 5 to 10080."),
                input(12, "Refresh period in minutes"),
            ))
        })
        .when(mode != RefreshMode::Disabled, |el| {
            el.child(field(
                "Show schemas",
                Some("Patterns separated by commas, for example sales_*. Empty shows all schemas."),
                input(10, "Show schemas"),
            ))
            .child(field(
                "Hide schemas",
                Some("Patterns separated by commas. Hides a schema also when Show schemas matches it."),
                input(11, "Hide schemas"),
            ))
            .child(field(
                "Refresh timeout",
                Some(
                    "Minutes before Qrow stops a schema refresh, from 1 to 1440. The schemas that it read stay in the tree.",
                ),
                input(13, "Refresh timeout in minutes"),
            ))
            .child(field(
                "Schema refresh logs",
                Some("Records each request of a schema refresh in the Logs of each tab of the connection. Errors go to Logs also when this is off."),
                Select::new(&form.refresh_logs)
                    .id("connection-refresh-logs")
                    .w_full()
                    .disabled(saving)
                    .accessibility_label("Schema refresh logs")
                    .into_any_element(),
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
