use super::*;
use gpui_kit::component::{
    Selectable, TitleBar, h_flex,
    input::Editor,
    tab::{Tab as QueryTab, TabBar},
    v_flex,
};

impl Qrow {
    fn connections(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.tabs[self.active].saved.profile;
        let busy = self.tabs[self.active].busy;
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h_12()
                    .pl_3()
                    .pr_2()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Connections"),
                    )
                    .child(
                        Button::new("add-connection")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .accessibility_label("New connection")
                            .tooltip("New connection…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_profile(Profile::default(), true, window, cx)
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("connections-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .gap_1()
                    .children(self.profiles.iter().map(|profile| {
                        let id = profile.id;
                        let edit = profile.clone();
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(SharedString::from(format!("profile-{id}")))
                                    .ghost()
                                    .small()
                                    .flex_1()
                                    .min_w_0()
                                    .accessibility_label(profile.name.clone())
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_2()
                                            .child(
                                                gpui_kit::component::Icon::default()
                                                    .path(crate::assets::SPARK_ICON)
                                                    .size_3p5()
                                                    .flex_shrink_0(),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .child(profile.name.clone()),
                                            ),
                                    )
                                    .selected(active == Some(id))
                                    .text_color(cx.theme().sidebar_foreground)
                                    .when(active == Some(id), |button| {
                                        button
                                            .bg(cx.theme().sidebar_accent)
                                            .text_color(cx.theme().sidebar_accent_foreground)
                                    })
                                    .disabled(busy)
                                    .tooltip(format!("{} · {}", profile.host, profile.database))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.switch_profile(id, cx)
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("edit-profile-{id}")))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Settings2)
                                    .text_color(cx.theme().sidebar_foreground)
                                    .accessibility_label(format!("Edit {}", profile.name))
                                    .tooltip("Edit connection…")
                                    .disabled(
                                        self.tabs
                                            .iter()
                                            .any(|t| t.saved.profile == Some(id) && t.busy),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.edit_profile(edit.clone(), false, window, cx)
                                    })),
                            )
                    }))
                    .when(self.profiles.is_empty(), |el| {
                        el.child(
                            v_flex().p_2().gap_2().child("No connections").child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Add a Kyuubi connection to run SQL."),
                            ),
                        )
                    }),
            )
    }

    fn query_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_height = self.ui_px(36.);
        TabBar::new("query-tabs")
            .selected_index(self.active)
            .large()
            .h(tab_height)
            .min_h(tab_height)
            .max_h(tab_height)
            .prefix(
                h_flex().h(self.ui_px(36.)).px_2().flex_shrink_0().child(
                    Button::new("sidebar-toggle")
                        .ghost()
                        .small()
                        .w(self.ui_px(28.))
                        .h(self.ui_px(28.))
                        .flex_shrink_0()
                        .icon(IconName::PanelLeft)
                        .accessibility_label("Toggle sidebar")
                        .tooltip("Toggle sidebar · ⌘B")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar = !this.sidebar;
                            cx.notify();
                        })),
                ),
            )
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                QueryTab::new()
                    // Kit's large tab uses a fixed 36px height internally.
                    // Constrain it to the same scaled height as the bar and its tools.
                    .min_h(tab_height)
                    .max_h(tab_height)
                    .label(tab.saved.title.clone())
                    .aria_label(format!(
                        "{}{}",
                        tab.saved.title,
                        if tab.busy { ", running" } else { "" }
                    ))
                    .suffix(
                        h_flex()
                            .gap_1()
                            .pr_2()
                            .when(tab.busy, |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Running"),
                                )
                            })
                            .child(
                                Button::new(SharedString::from(format!(
                                    "close-tab-{}",
                                    tab.saved.id
                                )))
                                .ghost()
                                .small()
                                .icon(IconName::Close)
                                .accessibility_label(format!("Close {}", tab.saved.title))
                                .tooltip("Close tab · ⌘W")
                                .disabled(tab.busy)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.close_tab(index, window, cx);
                                    },
                                )),
                            ),
                    )
            }))
            .on_click(
                cx.listener(|this, index: &usize, window, cx| this.activate(*index, window, cx)),
            )
            .suffix(
                h_flex().h(self.ui_px(36.)).px_2().flex_shrink_0().child(
                    Button::new("new-tab")
                        .ghost()
                        .small()
                        .w(self.ui_px(28.))
                        .h(self.ui_px(28.))
                        .icon(IconName::Plus)
                        .accessibility_label("New tab")
                        .tooltip("New tab · ⌘T")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)),
                        ),
                ),
            )
    }

    fn query_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = &self.tabs[self.active];
        let active = tab.saved.profile;
        h_flex()
            .h_12()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(!tab.busy, |el| {
                el.child(
                    Button::new("run")
                        .primary()
                        .small()
                        .icon(IconName::ArrowRight)
                        .label("Run")
                        .tooltip("Run SQL selection, or editor contents if nothing is selected. One statement only · ⌘Enter")
                        .disabled(active.is_none() && !self.demo)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.run(&RunQuery, window, cx)),
                        ),
                )
            })
            .when(tab.busy, |el| {
                el.child(
                    Button::new("cancel")
                        .small()
                        .label(if tab.cancelling {
                            "Cancelling"
                        } else {
                            "Cancel"
                        })
                        .disabled(tab.cancelling)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                )
            })
            .child(
                Button::new("disconnect")
                    .ghost()
                    .small()
                    .label("Disconnect")
                    .disabled(tab.busy || !tab.connected)
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
            )
    }

    fn results_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = &self.tabs[self.active];
        let data = tab.table.read(cx).delegate();
        let page = data.pagination.page();
        let pages = data.pagination.pages(data.rows.len());
        let range = data.pagination.range(data.rows.len());
        let count = if range.is_empty() {
            format!("0 rows · {} columns", data.columns.len())
        } else {
            format!(
                "Rows {}–{} · {} loaded · {} columns",
                range.start + 1,
                range.end,
                data.rows.len(),
                data.columns.len()
            )
        };
        let page_label = format!("Page {}", page + 1);
        results::selection_boundary(&tab.table)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                h_flex()
                    .h_10()
                    .flex_shrink_0()
                    .px_3()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().font_weight(FontWeight::MEDIUM).child("Results"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(count),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("page-label")
                            .role(Role::Label)
                            .text_xs()
                            .aria_label(page_label.clone())
                            .child(page_label),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_stretch()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded_md()
                            .overflow_hidden()
                            .child(
                                Button::new("previous-page")
                                    .small()
                                    .ghost()
                                    .rounded_none()
                                    .w_24()
                                    .label("Previous")
                                    .disabled(page == 0)
                                    .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                            )
                            .child(div().w_px().bg(cx.theme().border))
                            .child(
                                Button::new("next-page")
                                    .small()
                                    .ghost()
                                    .rounded_none()
                                    .w_24()
                                    .label("Next")
                                    .disabled(page + 1 >= pages && (!tab.more || tab.busy))
                                    .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                            ),
                    ),
            )
            .when_some(tab.error.clone(), |el, error| {
                el.child(
                    v_flex()
                        .id("query-error")
                        .max_h_32()
                        .overflow_y_scroll()
                        .p_3()
                        .gap_1()
                        .text_color(cx.theme().danger)
                        .child(div().font_weight(FontWeight::MEDIUM).child("Query failed"))
                        .child(error),
                )
            })
            .child(div().flex_1().min_h_0().min_w_0().child(results::view(
                &tab.table,
                self.form.is_some() || self.settings_open,
                self.settings.ui_scale,
                cx,
            )))
    }

    fn status_bar(&self, cx: &App) -> impl IntoElement {
        let tab = &self.tabs[self.active];
        h_flex()
            .h_8()
            .px_3()
            .gap_3()
            .flex_shrink_0()
            .text_xs()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .id("query-status")
                    .role(Role::Status)
                    .min_w_0()
                    .truncate()
                    .aria_label(tab.status.clone())
                    .child(tab.status.clone()),
            )
            .child(div().flex_1())
            .when_some(tab.elapsed, |el, elapsed| {
                el.child(format!("{:.2} s", elapsed.as_secs_f64()))
            })
            .child(if self.demo {
                "Demo · nothing is saved"
            } else if self.saver.is_none() {
                "Workspace saving disabled"
            } else if self.dirty.is_some() {
                "Saving"
            } else {
                "Workspace saved"
            })
    }

    fn splitter(&self, horizontal: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(if horizontal {
                "sidebar-splitter"
            } else {
                "editor-splitter"
            })
            .relative()
            .flex_shrink_0()
            .when(horizontal, |el| {
                el.w(self.ui_px(5.))
                    .mx(self.ui_px(-2.))
                    .h_full()
                    .cursor(CursorStyle::ResizeLeftRight)
            })
            .when(!horizontal, |el| {
                el.h(self.ui_px(5.))
                    .my(self.ui_px(-2.))
                    .w_full()
                    .cursor(CursorStyle::ResizeUpDown)
            })
            .child(
                div()
                    .absolute()
                    .bg(
                        if self.resize.is_some_and(|(axis, _, _)| axis == horizontal) {
                            cx.theme().primary
                        } else {
                            cx.theme().border
                        },
                    )
                    .when(horizontal, |el| {
                        el.left(self.ui_px(2.)).w(self.ui_px(1.)).h_full()
                    })
                    .when(!horizontal, |el| {
                        el.top(self.ui_px(2.)).h(self.ui_px(1.)).w_full()
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.resize = Some((
                        horizontal,
                        e.position,
                        if horizontal {
                            this.sidebar_width
                        } else {
                            this.editor_height
                        },
                    ));
                    cx.stop_propagation();
                }),
            )
    }
}

impl Render for Qrow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Split positions are measured window geometry. Clamp without mutating retained state during render.
        let editor_height = self
            .editor_height
            .min(window.viewport_size().height - self.ui_px(294.))
            .max(self.ui_px(100.));
        v_flex()
            .relative()
            .size_full()
            .key_context("Qrow")
            .track_focus(&self.focus)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::new_tab))
            .on_action(
                cx.listener(|this, _: &CloseTab, window, cx| {
                    this.close_tab(this.active, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar = !this.sidebar;
                cx.notify();
            }))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::increase_ui_scale))
            .on_action(cx.listener(Self::decrease_ui_scale))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                let Some((horizontal, start, initial)) = this.resize else {
                    return;
                };
                if e.pressed_button != Some(MouseButton::Left) {
                    this.resize = None;
                    return;
                }
                if horizontal {
                    this.sidebar_width = (initial + e.position.x - start.x)
                        .clamp(this.ui_px(180.), this.ui_px(360.));
                } else {
                    this.editor_height = (initial + e.position.y - start.y).clamp(
                        this.ui_px(100.),
                        window.viewport_size().height - this.ui_px(294.),
                    );
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resize = None),
            )
            .child(
                TitleBar::new().bg(cx.theme().title_bar).child(
                    div()
                        .flex_1()
                        .pr(self.ui_px(80.))
                        .text_center()
                        .font_weight(FontWeight::MEDIUM)
                        .child(if self.demo { "Qrow · Demo" } else { "Qrow" }),
                ),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .when(self.sidebar, |el| {
                        el.child(
                            div()
                                .w(self.sidebar_width)
                                .flex_shrink_0()
                                .child(self.connections(cx)),
                        )
                        .child(self.splitter(true, cx))
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.query_tabs(cx))
                            .child(self.query_toolbar(cx))
                            .child(
                                div().h(editor_height).flex_shrink_0().min_w_0().child(
                                    Editor::new(&self.tabs[self.active].input)
                                        .appearance(false)
                                        .font_family(self.settings.editor_font_family.clone())
                                        .text_size(self.ui_px(self.settings.editor_font_size))
                                        .size_full()
                                        .aria_label("SQL editor"),
                                ),
                            )
                            .child(self.splitter(false, cx))
                            .child(div().flex_1().min_h_0().child(self.results_panel(cx))),
                    ),
            )
            .when_some(self.message.clone(), |el, message| {
                el.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(cx.theme().warning)
                        .child(message),
                )
            })
            .child(self.status_bar(cx))
    }
}

/// Own window overlays outside Qrow so their builders can read its retained state.
pub(crate) struct WindowView {
    content: Entity<Qrow>,
}
impl WindowView {
    pub(crate) fn new(content: Entity<Qrow>) -> Self {
        Self { content }
    }
}
impl Render for WindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::Root;
        div()
            .size_full()
            // Capture above editor and dialog focus scopes, before text input consumes keys.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if !key.modifiers.platform || key.modifiers.control || key.modifiers.alt {
                    return;
                }
                let change = match key.key.as_str() {
                    "+" | "=" => UI_SCALE_STEP,
                    "-" => -UI_SCALE_STEP,
                    _ => return,
                };
                this.content.update(cx, |content, cx| {
                    content.adjust_ui_scale(change, window, cx)
                });
                cx.stop_propagation();
                window.prevent_default();
            }))
            .child(self.content.clone())
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}
