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

use super::{ProfileEditor, Qrow};
use gpui_kit::component::{input::Input, radio::Radio};
use gpui_kit::{Context, IntoElement, div, prelude::*};

pub(super) fn render_lifecycle(
    form: &ProfileEditor,
    labels: &[&'static str],
    cx: &mut Context<Qrow>,
) -> impl IntoElement {
    let saving = form.saving.is_some();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child("When idle")
        .children([(false, "Disconnect after"), (true, "Keep connected")].map(|(keep, label)| {
            Radio::new(label)
                .label(label)
                .checked(form.keep_connected == keep)
                .disabled(saving)
                .on_change(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = &mut this.form {
                        form.keep_connected = keep;
                    }
                    cx.notify();
                }))
        }))
        .children((7..10).filter(|i| (*i > 7) == form.keep_connected).map(|i| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(labels[i]))
                .child(Input::new(&form.fields[i]).disabled(saving))
        }))
        .when(form.keep_connected, |el| {
            el.child(div().text_sm().child("Heartbeats run only while idle and keep the engine active. Use a lightweight, read-only query."))
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
