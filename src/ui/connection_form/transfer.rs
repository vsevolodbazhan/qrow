//! Export controls owned by the connection form.
use super::{RowSelect, choice_select, chosen};
use crate::{
    model::{
        DatabaseType,
        transfer::{IncrementalCollect, Transfer, TransferPreset, TrinoSpooling},
    },
    ui::{ProfileEditor, Qrow, profile_view::connection_row},
};
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    input::{Input, InputState},
    select::{Select, SelectEvent},
    setting::{SettingGroup, SettingPage},
};
use gpui_kit::*;

pub(in crate::ui) struct TransferForm {
    preset: RowSelect,
    incremental: RowSelect,
    request: Entity<InputState>,
    speed: Entity<InputState>,
    count: RowSelect,
    spooling: RowSelect,
    _subscriptions: Vec<Subscription>,
}

fn presets() -> Vec<(TransferPreset, String)> {
    TransferPreset::ALL
        .into_iter()
        .map(|value| (value, value.label().into()))
        .collect()
}
fn incremental_choices() -> Vec<(IncrementalCollect, String)> {
    IncrementalCollect::ALL
        .into_iter()
        .map(|value| (value, value.label().into()))
        .collect()
}
fn spooling_choices() -> Vec<(TrinoSpooling, String)> {
    TrinoSpooling::ALL
        .into_iter()
        .map(|value| (value, value.label().into()))
        .collect()
}
fn counts() -> Vec<(u8, String)> {
    vec![(1, "1".into()), (2, "2".into())]
}

impl TransferForm {
    pub(in crate::ui) fn new(
        transfer: &Transfer,
        window: &mut Window,
        cx: &mut Context<Qrow>,
    ) -> Self {
        let preset = choice_select(&presets(), &transfer.preset, window, cx);
        let incremental = choice_select(
            &incremental_choices(),
            &transfer.custom.incremental_collect,
            window,
            cx,
        );
        let count = choice_select(&counts(), &transfer.custom.concurrent_exports, window, cx);
        let spooling = choice_select(
            &spooling_choices(),
            &transfer.custom.trino_spooling,
            window,
            cx,
        );
        let subscriptions = [&preset, &incremental, &count, &spooling]
            .into_iter()
            .map(|select| {
                cx.subscribe_in(select, window, |_, _, event, _, cx| {
                    if matches!(event, SelectEvent::Confirm(Some(_))) {
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            preset,
            incremental,
            count,
            spooling,
            request: cx.new(|cx| {
                InputState::new(window, cx).default_value(transfer.custom.request_mib.to_string())
            }),
            speed: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(transfer.custom.speed_limit_mb.to_string())
            }),
            _subscriptions: subscriptions,
        }
    }
    pub(in crate::ui) fn value(
        &self,
        previous: &Transfer,
        engine: DatabaseType,
        cx: &App,
    ) -> anyhow::Result<Transfer> {
        let mut transfer = previous.clone();
        transfer.preset = chosen(&self.preset, &presets(), cx);
        if transfer.preset == TransferPreset::Custom {
            transfer.custom.concurrent_exports = chosen(&self.count, &counts(), cx);
            if engine == DatabaseType::Trino {
                transfer.custom.trino_spooling = chosen(&self.spooling, &spooling_choices(), cx);
            }
            if engine == DatabaseType::Kyuubi {
                transfer.custom.incremental_collect =
                    chosen(&self.incremental, &incremental_choices(), cx);
                transfer.custom.request_mib =
                    self.request.read(cx).value().trim().parse().map_err(|_| {
                        anyhow::anyhow!("Data per request must be a whole number of MiB.")
                    })?;
                transfer.custom.speed_limit_mb =
                    self.speed.read(cx).value().trim().parse().map_err(|_| {
                        anyhow::anyhow!("Speed limit must be a whole number of MB/s.")
                    })?;
            }
        }
        transfer.validate()?;
        Ok(transfer)
    }
}

fn select(
    select: &RowSelect,
    form: &ProfileEditor,
    id: &'static str,
    label: &'static str,
) -> AnyElement {
    Select::new(select)
        .focus_ring(false)
        .id(id)
        .w_full()
        .disabled(form.saving.is_some())
        .accessibility_label(label)
        .into_any_element()
}
fn input(
    input: &Entity<InputState>,
    form: &ProfileEditor,
    id: &'static str,
    label: &'static str,
) -> AnyElement {
    Input::new(input)
        .focus_ring(false)
        .id(id)
        .w_full()
        .disabled(form.saving.is_some())
        .aria_label(label)
        .into_any_element()
}

pub(in crate::ui) fn page(form: &ProfileEditor, owner: &WeakEntity<Qrow>, cx: &App) -> SettingPage {
    let preset = chosen(&form.transfer.preset, &presets(), cx);
    let custom = preset == TransferPreset::Custom;
    let kyuubi = form.profile.database_type == DatabaseType::Kyuubi;
    let settings = Transfer {
        preset,
        ..form.profile.transfer.clone()
    }
    .settings();
    let mut group = SettingGroup::new().item(connection_row(
        owner,
        "Transfer Preset",
        "Choose how exports use this connection.",
        &["export", "transfer", "preset"],
        false,
        |_, form, _, _| {
            select(
                &form.transfer.preset,
                form,
                "connection-transfer-preset",
                "Transfer Preset",
            )
        },
    ));
    if kyuubi {
        let incremental = settings.incremental_collect.label().to_owned();
        group = group.item(connection_row(
            owner,
            "Incremental Collect",
            "Applies to new exports; Inherit keeps session settings.",
            &["export", "spark", "memory", "incremental"],
            false,
            move |_, form, _, _| {
                if custom {
                    select(
                        &form.transfer.incremental,
                        form,
                        "connection-transfer-incremental",
                        "Incremental Collect",
                    )
                } else {
                    gpui_kit::div()
                        .child(incremental.clone())
                        .into_any_element()
                }
            },
        ));
        let request = format!("{} MiB", settings.request_mib);
        group = group.item(connection_row(
            owner,
            "Data per Request",
            "Estimate from 1 to 32 MiB; wider rows can exceed it.",
            &["export", "fetch", "request", "size"],
            false,
            move |_, form, _, _| {
                if custom {
                    input(
                        &form.transfer.request,
                        form,
                        "connection-transfer-request",
                        "Data per Request in MiB",
                    )
                } else {
                    gpui_kit::div().child(request.clone()).into_any_element()
                }
            },
        ));
        let speed = if settings.speed_limit_mb == 0 {
            "Off".into()
        } else {
            format!("{} MB/s", settings.speed_limit_mb)
        };
        group = group.item(connection_row(
            owner,
            "Speed Limit",
            "Spool MB/s from 0 to 1000; zero disables the limit.",
            &["export", "speed", "rate"],
            false,
            move |_, form, _, _| {
                if custom {
                    input(
                        &form.transfer.speed,
                        form,
                        "connection-transfer-speed",
                        "Speed Limit in MB per Second",
                    )
                } else {
                    gpui_kit::div().child(speed.clone()).into_any_element()
                }
            },
        ));
    }
    if form.profile.database_type == DatabaseType::Trino {
        let mode = settings.trino_spooling.label().to_owned();
        group = group.item(connection_row(
            owner,
            "Spooled Results",
            "Applies to new exports when supported by the server.",
            &["export", "trino", "spool", "segment", "parallel"],
            false,
            move |_, form, _, _| {
                if custom {
                    select(
                        &form.transfer.spooling,
                        form,
                        "connection-transfer-spooling",
                        "Spooled Results",
                    )
                } else {
                    gpui_kit::div().child(mode.clone()).into_any_element()
                }
            },
        ));
    }
    let count = settings.concurrent_exports.to_string();
    group = group.item(connection_row(
        owner,
        "Exports at the Same Time",
        "Maximum per connection, including file publication.",
        &["export", "concurrent", "limit"],
        false,
        move |_, form, _, _| {
            if custom {
                select(
                    &form.transfer.count,
                    form,
                    "connection-transfer-count",
                    "Exports at the Same Time",
                )
            } else {
                gpui_kit::div().child(count.clone()).into_any_element()
            }
        },
    ));
    SettingPage::new("Export").resettable(false).group(group)
}
