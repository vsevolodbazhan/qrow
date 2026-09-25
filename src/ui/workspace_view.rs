use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Icon, Selectable, TitleBar, h_flex,
    input::Editor,
    spinner::Spinner,
    status_bar::StatusBar,
    tab::{Tab as QueryTab, TabBar},
    v_flex,
};
use qrow::model::copied_profile_name;
use std::rc::Rc;

const TAB_BAR_HEIGHT: f32 = 36.;

fn connection_name(profiles: &[Profile], id: Option<Uuid>) -> &str {
    id.and_then(|id| profiles.iter().find(|profile| profile.id == id))
        .map_or("No connection", |profile| profile.name.as_str())
}

fn connection_tooltip(profile: &Profile) -> String {
    format!("{} · {}", profile.host, profile.username)
}

fn query_status_label(status: &str, elapsed: Option<Duration>) -> String {
    let status = capitalize_status_details(status);
    match elapsed {
        Some(elapsed) => format!("{status} · {:.2} s", elapsed.as_secs_f64()),
        None => status,
    }
}

fn capitalize_status_details(status: &str) -> String {
    let mut capitalized = false;
    let mut label = String::with_capacity(status.len());

    for character in status.chars() {
        if capitalized && !character.is_whitespace() {
            label.extend(character.to_uppercase());
            capitalized = false;
        } else {
            label.push(character);
        }

        if character == '·' {
            capitalized = true;
        }
    }

    label
}

fn workspace_status(demo: bool, saving_enabled: bool, dirty: bool) -> &'static str {
    if demo {
        "Demo · Nothing is saved"
    } else if !saving_enabled {
        "Workspace saving disabled"
    } else if dirty {
        "Saving"
    } else {
        "Workspace saved"
    }
}

impl Qrow {
    fn connections(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_profile();
        let action_size = self.ui_px(28.);
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(self.ui_px(TAB_BAR_HEIGHT))
                    .flex_shrink_0()
                    .items_center()
                    .pl_3()
                    .pr_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Connections"),
                    )
                    .child(
                        Button::new("add-connection")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .flex_shrink_0()
                            .icon(IconName::Plus)
                            .accessibility_label("New Connection")
                            .tooltip("New Connection…")
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
                    .pt_2()
                    .gap_1()
                    .children(self.profiles.iter().map(|profile| {
                        let id = profile.id;
                        let busy = self.profile_busy(id);
                        let unread_error = self
                            .tabs
                            .iter()
                            .any(|tab| tab.saved.profile == Some(id) && tab.panel.unread_error);
                        let accessibility_label = format!(
                            "{}{}{}",
                            profile.name,
                            if busy { ", running" } else { "" },
                            if unread_error { ", unread error" } else { "" }
                        );
                        let restore = self.tabs[self.active].input.read(cx).focus_handle(cx);
                        let edited = profile.clone();
                        let duplicated = profile.clone();
                        let edit = Rc::new(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.edit_profile(edited.clone(), false, window, cx)
                        }));
                        let duplicate =
                            Rc::new(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                let mut profile = duplicated.clone();
                                profile.id = Uuid::new_v4();
                                profile.name = copied_profile_name(&profile.name, |name| {
                                    this.profiles.iter().any(|existing| existing.name == name)
                                });
                                this.edit_profile(profile, true, window, cx);
                            }));
                        let delete =
                            Rc::new(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.confirm_delete_profile(id, window, cx)
                            }));
                        let button = Button::new(SharedString::from(format!("profile-{id}")))
                            .ghost()
                            .small()
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .accessibility_label(accessibility_label)
                            .child(
                                h_flex()
                                    .h_full()
                                    .w_full()
                                    .min_w_0()
                                    .text_base()
                                    .line_height(relative(1.25))
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        gpui_kit::component::Icon::default()
                                            .path(crate::assets::SPARK_ICON)
                                            .size_4()
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(profile.name.clone()),
                                    )
                                    .when(busy, |el| {
                                        el.child(
                                            div()
                                                // Keep the loading glyph on the header action's centerline.
                                                .mr_0p5()
                                                .child(
                                                    Spinner::new()
                                                        .xsmall()
                                                        .color(cx.theme().muted_foreground),
                                                ),
                                        )
                                    })
                                    .when(unread_error, |el| {
                                        el.child(
                                            Icon::new(AssetIconName::TriangleAlert)
                                                .small()
                                                .text_color(cx.theme().danger),
                                        )
                                    }),
                            )
                            .selected(active == Some(id))
                            .text_color(cx.theme().sidebar_foreground)
                            .when(active == Some(id), |button| {
                                button
                                    .bg(cx.theme().sidebar_accent)
                                    .text_color(cx.theme().sidebar_accent_foreground)
                            })
                            .tooltip(connection_tooltip(profile))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.switch_profile(id, window, cx)
                            }));
                        h_flex()
                            .id(SharedString::from(format!("connection-{id}")))
                            .h(self.ui_px(32.))
                            .flex_shrink_0()
                            .child(button)
                            .context_menu(move |menu, _, _| {
                                menu.action_context(restore.clone())
                                    .item(
                                        PopupMenuItem::new("Edit Connection…")
                                            .on_click({
                                                let edit = edit.clone();
                                                move |event, window, cx| edit(event, window, cx)
                                            })
                                            .disabled(busy),
                                    )
                                    .item(PopupMenuItem::new("Duplicate").on_click({
                                        let duplicate = duplicate.clone();
                                        move |event, window, cx| duplicate(event, window, cx)
                                    }))
                                    .separator()
                                    .item(
                                        PopupMenuItem::new("Delete")
                                            .on_click({
                                                let delete = delete.clone();
                                                move |event, window, cx| delete(event, window, cx)
                                            })
                                            .disabled(busy),
                                    )
                            })
                    })),
            )
    }

    fn query_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_height = self.ui_px(TAB_BAR_HEIGHT);
        let visible = self.visible_tab_indices();
        TabBar::new("query-tabs")
            .selected_index(
                visible
                    .iter()
                    .position(|index| *index == self.active)
                    .unwrap_or(0),
            )
            .large()
            .h(tab_height)
            .min_h(tab_height)
            .max_h(tab_height)
            .prefix(
                h_flex().h(tab_height).px_2().flex_shrink_0().child(
                    Button::new("sidebar-toggle")
                        .ghost()
                        .small()
                        .w(self.ui_px(28.))
                        .h(self.ui_px(28.))
                        .flex_shrink_0()
                        .icon(IconName::PanelLeft)
                        .accessibility_label("Toggle Sidebar")
                        .tooltip("Toggle Sidebar · ⌘B")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar = !this.sidebar;
                            cx.notify();
                        })),
                ),
            )
            .children(visible.iter().map(|index| {
                let index = *index;
                let tab = &self.tabs[index];
                let id = tab.saved.id;
                QueryTab::new()
                    // Kit's large tab uses a fixed 36px height internally.
                    // Constrain it to the same scaled height as the bar and its tools.
                    .min_h(tab_height)
                    .max_h(tab_height)
                    // Right click does not activate the tab; the menu names its target.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |_, event: &MouseDownEvent, window, cx| {
                            let position = event.position;
                            cx.defer_in(window, move |this, window, cx| {
                                this.open_tab_menu(id, position, window, cx)
                            });
                        }),
                    )
                    .label(tab.saved.title.clone())
                    .aria_label(format!(
                        "{}{}{}",
                        tab.saved.title,
                        if tab.busy { ", running" } else { "" },
                        if tab.panel.unread_error {
                            ", unread error"
                        } else {
                            ""
                        },
                    ))
                    .suffix(
                        h_flex()
                            .gap_1()
                            .pr_2()
                            .when(tab.busy, |el| {
                                el.child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
                            })
                            .when(tab.panel.unread_error, |el| {
                                el.child(
                                    Icon::new(AssetIconName::TriangleAlert)
                                        .small()
                                        .text_color(cx.theme().danger),
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
                                .tooltip("Close Tab · ⌘W")
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
            .on_click(cx.listener(move |this, visible_index: &usize, window, cx| {
                if let Some(index) = this.visible_tab_indices().get(*visible_index).copied() {
                    this.activate(index, window, cx);
                }
            }))
            .suffix(
                h_flex().h(tab_height).px_2().flex_shrink_0().child(
                    Button::new("new-tab")
                        .ghost()
                        .small()
                        .w(self.ui_px(28.))
                        .h(self.ui_px(28.))
                        .icon(IconName::Plus)
                        .disabled(self.active_profile().is_none())
                        .accessibility_label("New Tab")
                        .tooltip("New Tab · ⌘T")
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
                    .disabled(!tab.can_disconnect())
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
            )
            .child(div().flex_1())
            .when(self.settings.assistant.enabled, |toolbar| {
                toolbar.child(
                    Button::new("toggle-assistant")
                        .ghost()
                        .small()
                        .label(if self.assistant_panel.unread && !self.assistant_panel.open {
                            "Assistant · Done"
                        } else { "Assistant" })
                        .selected(self.assistant_panel.open)
                        .tooltip("AI Assistant · ⌘J")
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_assistant(window, cx))),
                )
            })
    }

    fn query_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.tabs[self.active].panel.selected == Panel::Output {
            return self.output_panel(cx);
        }
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
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(self.panel_switcher(cx))
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
                    .child(button_pair::button_pair(
                        "pagination-buttons",
                        Button::new("previous-page")
                            .small()
                            .ghost()
                            .w_24()
                            .label("Previous")
                            .disabled(page == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                        Button::new("next-page")
                            .small()
                            .ghost()
                            .w_24()
                            .label("Next")
                            .disabled(page + 1 >= pages && (!tab.more || tab.busy))
                            .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                        cx,
                    )),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(results::view(
                &tab.table,
                self.dialog_open(),
                self.settings.ui_scale,
                cx,
            )))
            .into_any_element()
    }

    fn status_bar(&self) -> impl IntoElement {
        let tab = &self.tabs[self.active];
        let connection_name = connection_name(&self.profiles, tab.saved.profile);
        let status_label = query_status_label(&tab.status, tab.elapsed);
        let workspace_status =
            workspace_status(self.demo, self.saver.is_some(), self.dirty.is_some());

        StatusBar::new()
            .flex_shrink_0()
            .left(
                div()
                    .id("query-status")
                    .role(Role::Status)
                    .min_w_0()
                    .truncate()
                    .aria_label(status_label.clone())
                    .child(status_label),
            )
            .child(
                div()
                    .id("current-connection")
                    .role(Role::Label)
                    .min_w_0()
                    .truncate()
                    .aria_label(format!("Current connection: {connection_name}"))
                    .child(connection_name.to_owned()),
            )
            .right(
                div()
                    .id("workspace-status")
                    .role(Role::Status)
                    .min_w_0()
                    .truncate()
                    .aria_label(workspace_status)
                    .child(workspace_status),
            )
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

    fn assistant_splitter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("assistant-splitter")
            .relative()
            .flex_shrink_0()
            .w(self.ui_px(5.))
            .mx(self.ui_px(-2.))
            .h_full()
            .cursor(CursorStyle::ResizeLeftRight)
            .child(
                div()
                    .absolute()
                    .left(self.ui_px(2.))
                    .w(self.ui_px(1.))
                    .h_full()
                    .bg(if self.assistant_panel.resizing.is_some() {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.assistant_panel.resizing = Some((
                        event.position,
                        this.ui_px(this.settings.assistant.panel_width),
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
        let available = window.viewport_size().width
            - self.ui_px(420.)
            - if self.sidebar {
                self.sidebar_width
            } else {
                px(0.)
            };
        let assistant_width = self
            .ui_px(self.settings.assistant.panel_width)
            .min(available)
            .max(self.ui_px(360.));
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
                this.assistant_panel.auto_hidden_sidebar = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleAssistant, window, cx| {
                this.toggle_assistant(window, cx)
            }))
            .on_action(cx.listener(Self::open_about))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::increase_ui_scale))
            .on_action(cx.listener(Self::decrease_ui_scale))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if let Some((start, initial)) = this.assistant_panel.resizing {
                    if e.pressed_button != Some(MouseButton::Left) {
                        this.assistant_panel.resizing = None;
                        return;
                    }
                    let available = window.viewport_size().width
                        - this.ui_px(420.)
                        - if this.sidebar {
                            this.sidebar_width
                        } else {
                            px(0.)
                        };
                    let width = (initial + start.x - e.position.x).clamp(
                        this.ui_px(360.),
                        this.ui_px(640.).min(available).max(this.ui_px(360.)),
                    );
                    this.settings.assistant.panel_width = f32::from(width) / this.settings.ui_scale;
                    this.changed(cx);
                    return;
                }
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
                cx.listener(|this, _, _, _| {
                    this.resize = None;
                    this.assistant_panel.resizing = None;
                }),
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
                                div()
                                    .h(editor_height)
                                    .flex_shrink_0()
                                    .min_w_0()
                                    .line_height(relative(self.settings.editor_line_height))
                                    .child(
                                        Editor::new(&self.tabs[self.active].input)
                                            .appearance(false)
                                            .font_family(self.settings.editor_font_family.clone())
                                            .text_size(self.ui_px(self.settings.editor_font_size))
                                            .size_full()
                                            .aria_label("SQL Editor"),
                                    ),
                            )
                            .child(self.splitter(false, cx))
                            .child(div().flex_1().min_h_0().child(self.query_panel(cx))),
                    )
                    .when(
                        self.settings.assistant.enabled && self.assistant_panel.open,
                        |el| {
                            el.child(self.assistant_splitter(cx)).child(
                                div()
                                    .w(assistant_width)
                                    .flex_shrink_0()
                                    .min_h_0()
                                    .child(self.assistant_panel(cx)),
                            )
                        },
                    ),
            )
            .when_some(self.menu.as_ref(), |el, menu| {
                el.child(
                    deferred(
                        anchored()
                            .position(menu.position)
                            .snap_to_window_with_margin(px(8.))
                            .child(menu.view.clone()),
                    )
                    .with_priority(gpui_kit::base::POPUP_PRIORITY),
                )
            })
            .when_some(self.message.clone(), |el, message| {
                el.child(
                    div()
                        .id("workspace-message")
                        .px_3()
                        .py_2()
                        .text_color(cx.theme().warning)
                        .role(Role::Status)
                        .aria_label(message.clone())
                        .child(message),
                )
            })
            .child(self.status_bar())
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
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.content
                    .update(cx, |content, cx| content.request_quit(window, cx));
            }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn status_bar_formats_labels() {
        let profile = Profile {
            name: "Analytics".into(),
            ..Profile::default()
        };

        assert_eq!(
            connection_name(std::slice::from_ref(&profile), Some(profile.id)),
            "Analytics"
        );
        assert_eq!(
            connection_name(&[profile], Some(Uuid::new_v4())),
            "No connection"
        );
        let tooltip_profile = Profile {
            host: "kyuubi.example.com".into(),
            username: "aviaservice".into(),
            database: "initial_database".into(),
            ..Profile::default()
        };
        assert_eq!(
            connection_tooltip(&tooltip_profile),
            "kyuubi.example.com · aviaservice"
        );
        assert_eq!(query_status_label("Executing…", None), "Executing…");
        assert_eq!(
            query_status_label("Complete · demo data", Some(Duration::from_millis(842))),
            "Complete · Demo data · 0.84 s"
        );
        assert_eq!(
            query_status_label("Error · connection lost · retry later", None),
            "Error · Connection lost · Retry later"
        );
        assert_eq!(
            workspace_status(true, true, false),
            "Demo · Nothing is saved"
        );
        assert_eq!(
            workspace_status(false, false, false),
            "Workspace saving disabled"
        );
        assert_eq!(workspace_status(false, true, true), "Saving");
        assert_eq!(workspace_status(false, true, false), "Workspace saved");
    }
}
