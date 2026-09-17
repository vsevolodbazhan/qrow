use qrow::model::ConnectionLifecycle;

pub(super) fn parse_lifecycle(
    values: &[String],
    keep_connected: bool,
    previous: &ConnectionLifecycle,
) -> anyhow::Result<ConnectionLifecycle> {
    let mut policy = previous.clone();
    if keep_connected {
        policy.keep_alive_seconds = values[1].trim().parse().map_err(|_| {
            anyhow::anyhow!("Heartbeat interval must be a whole number of seconds.")
        })?;
        anyhow::ensure!(
            policy.keep_alive_seconds > 0,
            "Heartbeat interval must be greater than zero."
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

use super::setting_row::Rows;
use super::{ProfileEditor, Qrow};
use gpui_kit::component::{input::Input, radio::Radio, v_flex};
use gpui_kit::{AnyElement, Context, IntoElement, prelude::*};

pub(super) fn render_lifecycle(
    form: &ProfileEditor,
    rows: &Rows,
    cx: &mut Context<Qrow>,
) -> impl IntoElement {
    let saving = form.saving.is_some();
    let keep = form.keep_connected;
    let field = |index: usize, label: &'static str| {
        Input::new(&form.fields[index])
            .disabled(saving)
            .aria_label(label)
            .into_any_element()
    };
    let choice: AnyElement = v_flex()
        .gap_2()
        .children(
            [(false, "Disconnect"), (true, "Keep Connected")].map(|(value, label)| {
                Radio::new(label)
                    .label(label)
                    .checked(keep == value)
                    .disabled(saving)
                    .on_change(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = &mut this.form {
                            form.keep_connected = value;
                        }
                        cx.notify();
                    }))
            }),
        )
        .into_any_element();
    v_flex()
        .child(rows.row(
            "When Idle",
            "Releasing the session keeps SQL and downloaded results. It drops temporary views, session settings, and unfetched rows.",
            choice,
            cx,
        ))
        .when(!keep, |el| {
            el.child(rows.row(
                "Idle Timeout",
                "Seconds of inactivity before the session is released.",
                field(7, "Idle Timeout in Seconds"),
                cx,
            ))
        })
        .when(keep, |el| {
            el.child(rows.row(
                "Heartbeat Interval",
                "Seconds between keep-alive queries.",
                field(8, "Heartbeat Interval in Seconds"),
                cx,
            ))
            .child(rows.row(
                "Heartbeat SQL",
                "Runs only while idle, and keeps the engine active. Use a lightweight, read-only query.",
                field(9, "Heartbeat SQL"),
                cx,
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
