use crate::model::{CatalogRefresh, CatalogSettings, ConnectionLifecycle, Profile};

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
pub(super) fn parse_refresh_policy(
    period: &str,
    timeout: &str,
    automatic: bool,
    settings: &mut CatalogSettings,
) -> anyhow::Result<()> {
    settings.refresh = if automatic {
        let minutes = period
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("Refresh period must be a whole number of minutes."))?;
        CatalogRefresh::WhileConnected { minutes }
    } else {
        CatalogRefresh::Manual
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
    IndexPath,
    form::{Field, Form},
    input::Input,
    select::{SearchableVec, Select, SelectEvent, SelectState},
};
use gpui_kit::{AnyElement, App, Context, Entity, IntoElement, Window, prelude::*};

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
const MANUAL: &str = "Manual";
const WHILE_CONNECTED: &str = "While connected";
const DISABLED: &str = "Disabled";
const ENABLED: &str = "Enabled";

/// A dropdown of a few fixed choices.
pub(super) type ChoiceSelect = Entity<SelectState<SearchableVec<String>>>;

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

/// A choice between Manual and While connected schema refresh.
pub(super) fn schema_refresh_select(
    automatic: bool,
    window: &mut Window,
    cx: &mut Context<Qrow>,
) -> ChoiceSelect {
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(vec![MANUAL.into(), WHILE_CONNECTED.into()]),
            Some(IndexPath::default().row(usize::from(automatic))),
            window,
            cx,
        )
    })
}

pub(super) fn refreshes_automatically(select: &ChoiceSelect, cx: &App) -> bool {
    select
        .read(cx)
        .selected_value()
        .is_some_and(|choice| choice == WHILE_CONNECTED)
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

pub(super) fn render_schemas(form: &ProfileEditor, cx: &App) -> impl IntoElement {
    let saving = form.saving.is_some();
    let automatic = refreshes_automatically(&form.schema_refresh, cx);
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
            "Schema refresh",
            Some(
                "While connected reads the schemas again after each refresh period, only while a tab of this connection is connected. Manual reads them only when you select Refresh.",
            ),
            Select::new(&form.schema_refresh)
                .id("connection-schema-refresh")
                .w_full()
                .disabled(saving)
                .accessibility_label("Schema refresh")
                .into_any_element(),
        ))
        .when(automatic, |el| {
            el.child(field(
                "Refresh period",
                Some("Minutes between automatic schema refreshes, from 5 to 10080."),
                input(12, "Refresh period in minutes"),
            ))
        })
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
    fn only_an_automatic_schema_refresh_reads_its_period() {
        let mut settings = CatalogSettings::default();
        parse_refresh_policy("invalid", " 10 ", false, &mut settings).unwrap();
        assert_eq!(settings.refresh, CatalogRefresh::Manual);
        assert_eq!(settings.timeout_minutes, 10);
        parse_refresh_policy(" 15 ", "10", true, &mut settings).unwrap();
        assert_eq!(
            settings.refresh,
            CatalogRefresh::WhileConnected { minutes: 15 }
        );
        assert!(parse_refresh_policy("invalid", "10", true, &mut settings).is_err());
        assert!(parse_refresh_policy("4", "10", true, &mut settings).is_err());
        assert!(parse_refresh_policy("15", "0", true, &mut settings).is_err());
        assert!(parse_refresh_policy("15", "1.5", false, &mut settings).is_err());
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
}
