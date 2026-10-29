//! Export controls owned by the connection form.
use super::{RowSelect, choice_select, chosen};
use crate::{
    model::{
        DatabaseType,
        transfer::{IncrementalCollect, Transfer, TransferPreset, TransferSettings, TrinoSpooling},
    },
    ui::{ProfileEditor, Qrow, profile_view::connection_row},
};
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    ActiveTheme,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent},
    setting::{SettingGroup, SettingPage},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub(in crate::ui) struct TransferForm {
    preset: RowSelect,
    incremental: RowSelect,
    request: Entity<InputState>,
    speed: Entity<InputState>,
    count: Entity<InputState>,
    postgres_count: Entity<InputState>,
    spooling: RowSelect,
    readonly: [Entity<InputState>; 5],
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

fn readonly_values(settings: TransferSettings) -> [String; 5] {
    [
        settings.incremental_collect.label().into(),
        format!("{} MiB", settings.request_mib),
        if settings.speed_limit_mb == 0 {
            "Off".into()
        } else {
            format!("{} MB/s", settings.speed_limit_mb)
        },
        settings.trino_spooling.label().into(),
        settings.concurrent_exports.to_string(),
    ]
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
        let count = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(transfer.custom.concurrent_exports.to_string())
        });
        let postgres_count = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(transfer.settings().concurrent_exports.to_string())
        });
        let spooling = choice_select(
            &spooling_choices(),
            &transfer.custom.trino_spooling,
            window,
            cx,
        );
        let request = cx.new(|cx| {
            InputState::new(window, cx).default_value(transfer.custom.request_mib.to_string())
        });
        let speed = cx.new(|cx| {
            InputState::new(window, cx).default_value(transfer.custom.speed_limit_mb.to_string())
        });
        let mut subscriptions: Vec<_> = [&preset, &incremental, &spooling]
            .into_iter()
            .map(|select| {
                cx.subscribe_in(select, window, |this, _, event, window, cx| {
                    if matches!(event, SelectEvent::Confirm(Some(_))) {
                        if let Some(form) = &this.form {
                            form.transfer.sync_readonly(window, cx);
                        }
                        cx.notify();
                    }
                })
            })
            .collect();
        for input in [&request, &speed, &count, &postgres_count] {
            subscriptions.push(cx.subscribe_in(input, window, |_, _, event, _, cx| {
                if matches!(
                    event,
                    InputEvent::Change | InputEvent::Focus | InputEvent::Blur
                ) {
                    cx.notify();
                }
            }));
        }
        Self {
            preset,
            incremental,
            count,
            postgres_count,
            spooling,
            readonly: readonly_values(transfer.settings())
                .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value))),
            request,
            speed,
            _subscriptions: subscriptions,
        }
    }
    pub(in crate::ui) fn switch_engine(
        &self,
        previous: DatabaseType,
        next: DatabaseType,
        window: &mut Window,
        cx: &mut App,
    ) {
        let preset = chosen(&self.preset, &presets(), cx);
        let count = if preset == TransferPreset::Custom {
            self.count.read(cx).value().to_string()
        } else {
            Transfer {
                preset,
                ..Transfer::default()
            }
            .settings()
            .concurrent_exports
            .to_string()
        };
        if next == DatabaseType::Postgres && previous != DatabaseType::Postgres {
            self.postgres_count.update(cx, |input, cx| {
                input.set_value(count, window, cx);
            });
        } else if previous == DatabaseType::Postgres && next != DatabaseType::Postgres {
            let postgres_count = self.postgres_count.read(cx).value().to_string();
            let same_count = match (
                postgres_count.trim().parse::<u8>(),
                count.trim().parse::<u8>(),
            ) {
                (Ok(left), Ok(right)) => left == right,
                _ => postgres_count == count,
            };
            if !same_count {
                self.count.update(cx, |input, cx| {
                    input.set_value(postgres_count, window, cx);
                });
                self.preset.update(cx, |select, cx| {
                    select.set_selected_value(&3usize, window, cx);
                });
            }
        }
    }
    fn sync_readonly(&self, window: &mut Window, cx: &mut App) {
        let settings = Transfer {
            preset: chosen(&self.preset, &presets(), cx),
            ..Transfer::default()
        }
        .settings();
        for (input, value) in self.readonly.iter().zip(readonly_values(settings)) {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }
    pub(in crate::ui) fn value(
        &self,
        previous: &Transfer,
        engine: DatabaseType,
        cx: &App,
    ) -> anyhow::Result<Transfer> {
        let mut transfer = previous.clone();
        if engine == DatabaseType::Postgres {
            let count = parse_count(&self.postgres_count, cx)?;
            if count != previous.settings().concurrent_exports {
                transfer.custom = previous.settings();
                transfer.preset = TransferPreset::Custom;
                transfer.custom.concurrent_exports = count;
            }
            transfer.validate()?;
            return Ok(transfer);
        }
        transfer.preset = chosen(&self.preset, &presets(), cx);
        if transfer.preset == TransferPreset::Custom {
            transfer.custom.concurrent_exports = parse_count(&self.count, cx)?;
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

fn parse_count(input: &Entity<InputState>, cx: &App) -> anyhow::Result<u8> {
    input.read(cx).value().trim().parse().map_err(|_| {
        anyhow::anyhow!("Exports at the same time must be a whole number from 0 to 255.")
    })
}

fn count_input(input: &Entity<InputState>, form: &ProfileEditor) -> AnyElement {
    Input::new(input)
        .focus_ring(false)
        .id("connection-transfer-count")
        .w_full()
        .disabled(form.saving.is_some())
        .aria_label("Exports at the Same Time")
        .into_any_element()
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
    unit: (&'static str, &'static str),
    window: &Window,
    cx: &App,
) -> AnyElement {
    let (unit, unit_id) = unit;
    let value = input.read(cx).value();
    // Bound the measurement for invalid paste data. Long input uses the
    // regular suffix so that the unit cannot cover the text or caret.
    let measured: String = value.chars().take(32).collect();
    let complete = measured.len() == value.len();
    let font_size = window.rem_size() * 0.875;
    let run = TextRun {
        len: measured.len(),
        font: window.text_style().font(),
        color: cx.theme().foreground,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let width = window
        .text_system()
        .shape_line(measured.into(), font_size, &[run], None)
        .width;
    let adjacent = complete && width <= window.rem_size() * 7.;
    let disabled = form.saving.is_some();
    div()
        .relative()
        .w_full()
        .child(
            Input::new(input)
                .focus_ring(false)
                .id(id)
                .w_full()
                .disabled(disabled)
                .aria_label(label)
                .when(!adjacent, |view| {
                    view.suffix(div().id(unit_id).test_support().child(unit))
                }),
        )
        .when(adjacent, |view| {
            view.child(
                div()
                    .id(unit_id)
                    .test_support()
                    .absolute()
                    .left(px(14.) + width)
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(font_size)
                    .when(disabled, |view| view.opacity(0.5))
                    .child(unit),
            )
        })
        .into_any_element()
}

fn readonly(
    input: &Entity<InputState>,
    form: &ProfileEditor,
    id: &'static str,
    label: &'static str,
) -> AnyElement {
    Input::new(input)
        .readonly(true)
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
    let postgres = form.profile.database_type == DatabaseType::Postgres;
    let mut group = SettingGroup::new();
    if !postgres {
        group = group.item(connection_row(
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
    }
    if kyuubi {
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
                    readonly(
                        &form.transfer.readonly[0],
                        form,
                        "connection-transfer-incremental",
                        "Incremental Collect",
                    )
                }
            },
        ));
        group = group.item(connection_row(
            owner,
            "Data per Request",
            "Estimate from 1 to 32 MiB; wider rows can exceed it.",
            &["export", "fetch", "request", "size"],
            false,
            move |_, form, window, cx| {
                if custom {
                    input(
                        &form.transfer.request,
                        form,
                        "connection-transfer-request",
                        "Data per Request in MiB",
                        ("MiB", "connection-transfer-request-unit"),
                        window,
                        cx,
                    )
                } else {
                    readonly(
                        &form.transfer.readonly[1],
                        form,
                        "connection-transfer-request",
                        "Data per Request in MiB",
                    )
                }
            },
        ));
        group = group.item(connection_row(
            owner,
            "Speed Limit",
            "Spool MB/s from 0 to 1000; zero disables the limit.",
            &["export", "speed", "rate"],
            false,
            move |_, form, window, cx| {
                if custom {
                    input(
                        &form.transfer.speed,
                        form,
                        "connection-transfer-speed",
                        "Speed Limit in MB per Second",
                        ("MB/s", "connection-transfer-speed-unit"),
                        window,
                        cx,
                    )
                } else {
                    readonly(
                        &form.transfer.readonly[2],
                        form,
                        "connection-transfer-speed",
                        "Speed Limit in MB per Second",
                    )
                }
            },
        ));
    }
    if form.profile.database_type == DatabaseType::Trino {
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
                    readonly(
                        &form.transfer.readonly[3],
                        form,
                        "connection-transfer-spooling",
                        "Spooled Results",
                    )
                }
            },
        ));
    }
    group = group.item(connection_row(
        owner,
        "Exports at the Same Time",
        "Maximum per connection; 0 disables the limit.",
        &["export", "concurrent", "limit"],
        false,
        move |_, form, _, _| {
            if postgres {
                count_input(&form.transfer.postgres_count, form)
            } else if custom {
                count_input(&form.transfer.count, form)
            } else {
                readonly(
                    &form.transfer.readonly[4],
                    form,
                    "connection-transfer-count",
                    "Exports at the Same Time",
                )
            }
        },
    ));
    SettingPage::new("Export").resettable(false).group(group)
}
