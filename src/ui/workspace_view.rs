use super::assistant_view::ThreadStatus;
use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Icon, Selectable as _, TitleBar, h_flex,
    input::Editor,
    shimmer::ShimmerText,
    status_bar::StatusBar,
    tab::{Tab as QueryTab, TabBar},
    v_flex,
};

pub(super) const TAB_BAR_HEIGHT: f32 = 36.;

fn connection_name(profiles: &[Profile], id: Option<Uuid>) -> &str {
    id.and_then(|id| profiles.iter().find(|profile| profile.id == id))
        .map_or("No connection", |profile| profile.name.as_str())
}

fn workspace_status(demo: bool, saving_enabled: bool, dirty: bool) -> &'static str {
    if demo {
        "Demo changes are not saved"
    } else if !saving_enabled {
        "Workspace saving disabled"
    } else if dirty {
        "Saving"
    } else {
        "Workspace saved"
    }
}

impl Qrow {
    pub(super) fn sidebar_header(
        &self,
        title: &'static str,
        cx: &App,
    ) -> impl IntoElement + ParentElement {
        h_flex()
            .id("sidebar-header")
            .test_support()
            .relative()
            .h(self.ui_px(TAB_BAR_HEIGHT))
            .flex_shrink_0()
            .pl_3()
            .pr_2()
            .gap_1()
            // Match TabBar's painted separator without reducing the content height.
            .child(
                div()
                    .id("sidebar-header-border")
                    .test_support()
                    .absolute()
                    .inset_0()
                    .border_b_1()
                    .border_color(cx.theme().border),
            )
            .child(
                div()
                    .id("sidebar-title")
                    .test_support()
                    .flex_1()
                    .text_base()
                    .line_height(relative(1.25))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
    }

    fn query_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_height = self.ui_px(TAB_BAR_HEIGHT);
        let visible = self.visible_tab_indices();
        TabBar::new("query-tabs")
            .track_scroll(&self.tab_scroll)
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
            .children(visible.iter().map(|index| {
                let index = *index;
                let tab = &self.tabs[index];
                let id = tab.saved.id;
                let assistant_busy = self.settings.assistant.enabled && self.assistant_tab_busy(id);
                let title_generating =
                    self.assistant
                        .conversation_for_tab(id)
                        .is_some_and(|conversation| {
                            conversation.title_follows_conversation
                                && self.assistant_title_generating(&conversation.thread_id)
                        });
                QueryTab::new()
                    // Kit's large tab uses a fixed 36px height internally.
                    // Constrain it to the same scaled height as the bar and its tools.
                    .min_h(tab_height)
                    .max_h(tab_height)
                    // A tab without a dot shows its status on the tab itself.
                    .when(tab.dot_status().is_none(), |query_tab| {
                        query_tab.map(|mut query_tab| {
                            query_tab.interactivity().tooltip(StatusTooltip::live(
                                cx.entity().downgrade(),
                                None,
                                move |qrow, _| {
                                    qrow.tabs
                                        .iter()
                                        .find(|tab| tab.saved.id == id)
                                        .map(Tab::status_tooltip)
                                },
                            ));
                            query_tab
                        })
                    })
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
                    .label(if title_generating {
                        String::new()
                    } else {
                        tab.saved.title.clone()
                    })
                    .when(title_generating, |query_tab| {
                        query_tab.child(
                            ShimmerText::new(tab.saved.title.clone())
                                .id(format!("assistant-tab-title-{id}")),
                        )
                    })
                    .aria_label(format!(
                        "{}{}{}{}{}",
                        tab.saved.title,
                        tab.work_suffix(),
                        if tab.panel.unread_error {
                            ", unread error"
                        } else {
                            ""
                        },
                        if tab.panel.has_unread_success() {
                            ", unread query result"
                        } else {
                            ""
                        },
                        if title_generating {
                            ", generating title"
                        } else {
                            ""
                        },
                    ))
                    .suffix(
                        h_flex()
                            .gap_1()
                            .pr_2()
                            .when_some(tab.dot_status(), |el, status| {
                                let label = format!("{}{}", tab.saved.title, tab.status_suffix());
                                let tooltip = StatusTooltip::live(
                                    cx.entity().downgrade(),
                                    None,
                                    move |qrow, _| {
                                        qrow.tabs
                                            .iter()
                                            .find(|tab| tab.saved.id == id)
                                            .map(Tab::status_tooltip)
                                    },
                                );
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("query-status-{id}")))
                                        .test_support()
                                        .role(Role::Status)
                                        .aria_label(label)
                                        .tooltip(tooltip)
                                        .child(status.dot(cx)),
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
                                .map(|mut button| {
                                    button.interactivity().tooltip(
                                        StatusTooltip::new("Close Tab", "")
                                            .for_action(&CloseTab, None),
                                    );
                                    button
                                })
                                .disabled(tab.busy || assistant_busy)
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
                h_flex().h(tab_height).px_2().gap_1().flex_shrink_0().child(
                    Button::new("new-tab")
                        .ghost()
                        .small()
                        .w(self.ui_px(28.))
                        .h(self.ui_px(28.))
                        .icon(IconName::Plus)
                        .disabled(self.active_profile().is_none())
                        .accessibility_label("New Tab")
                        .map(|mut button| {
                            button.interactivity().tooltip(
                                StatusTooltip::new("New Tab", "").for_action(&NewTab, None),
                            );
                            button
                        })
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
                        .map(|mut button| { button.interactivity().tooltip(StatusTooltip::new("Run Query", "")
                            .detail("Run the selected SQL or the editor contents. One statement only.")
                            .for_action(&RunQuery, None)); button })
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
    }

    fn query_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.tabs[self.active].panel.selected == Panel::Output {
            return self.output_panel(cx);
        }
        let tab = &self.tabs[self.active];
        results::selection_boundary(&tab.table)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.result_toolbar(cx))
            .child(div().flex_1().min_h_0().min_w_0().child(results::view(
                &tab.table,
                self.dialog_open(),
                self.settings.ui_scale,
                cx,
            )))
            .into_any_element()
    }

    /// The Assistant button of the status bar. Its dot shows the most urgent
    /// state of all conversations.
    fn assistant_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.assistant_status();
        let accessibility_label = match status {
            ThreadStatus::Failed => "Toggle Assistant, reply failed",
            ThreadStatus::Ready => "Toggle Assistant, reply ready",
            ThreadStatus::Approval => "Toggle Assistant, waiting for approval",
            ThreadStatus::Working => "Toggle Assistant, working",
            _ => "Toggle Assistant",
        };
        let toggle = Button::new("toggle-assistant")
            .ghost()
            .small()
            .selected(self.assistant_state.open)
            .accessibility_label(accessibility_label)
            .icon(Icon::new(AssetIconName::Bot).small())
            .on_click(cx.listener(|this, _, window, cx| this.toggle_assistant(window, cx)));
        div()
            .id("assistant-tooltip-trigger")
            .tooltip(StatusTooltip::live(
                cx.entity().downgrade(),
                Some(&ToggleAssistant),
                |qrow, _| {
                    Some(StatusTooltip::new(
                        "Assistant",
                        qrow.assistant_status().tooltip_status(),
                    ))
                },
            ))
            .child(DotStatus::on_button(status.dot_status(), toggle, cx))
    }

    /// The dot, the accessibility label, and the tooltip of the Sign-ins
    /// button. The dot shows sign-ins that need attention, then browser
    /// sign-ins that run.
    fn sign_ins_status(&self) -> (Option<DotStatus>, String, StatusTooltip) {
        let attention = self.sign_ins_needing_attention();
        let working = self.sign_ins_working();
        let mut label = String::from("Sign-ins");
        let mut statuses = Vec::new();
        if working {
            label.push_str(", sign-in in progress");
            statuses.push("Sign-in Running".to_owned());
        }
        match attention {
            0 => {}
            1 => {
                label.push_str(", 1 sign-in needs attention");
                statuses.insert(0, "1 Needs Attention".to_owned());
            }
            count => {
                label.push_str(&format!(", {count} sign-ins need attention"));
                statuses.insert(0, format!("{count} Need Attention"));
            }
        }
        let status = if attention > 0 {
            Some(DotStatus::Attention)
        } else if working {
            Some(DotStatus::Working)
        } else {
            None
        };
        (
            status,
            label,
            StatusTooltip::new("Sign-ins", statuses.join("\n")),
        )
    }

    /// The buttons of the status bar that choose the panel of the sidebar.
    /// The button of the visible panel hides the sidebar.
    fn sidebar_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = |panel| self.sidebar && self.sidebar_panel == panel;
        let (status, label, _) = self.sign_ins_status();
        let connections = Button::new("show-connections")
            .ghost()
            .small()
            .selected(shown(SidebarPanel::Connections))
            .icon(Icon::new(AssetIconName::Plug).small())
            .accessibility_label("Connections")
            .on_click(cx.listener(|this, _, window, cx| {
                this.show_sidebar_panel(SidebarPanel::Connections, window, cx)
            }));
        let sign_ins = Button::new("show-sign-ins")
            .ghost()
            .small()
            .selected(shown(SidebarPanel::SignIns))
            .icon(Icon::new(AssetIconName::KeyRound).small())
            .accessibility_label(label)
            .on_click(cx.listener(|this, _, window, cx| {
                this.show_sidebar_panel(SidebarPanel::SignIns, window, cx)
            }));
        h_flex()
            .flex_shrink_0()
            .gap_1()
            .child(
                div()
                    .id("connections-tooltip-trigger")
                    .tooltip(
                        StatusTooltip::new("Connections", "").for_action(&ShowConnections, None),
                    )
                    .child(connections),
            )
            .child(
                div()
                    .id("sign-ins-tooltip-trigger")
                    .tooltip(StatusTooltip::live(
                        cx.entity().downgrade(),
                        Some(&ShowSignIns),
                        |qrow, _| Some(qrow.sign_ins_status().2),
                    ))
                    .child(DotStatus::on_button(status, sign_ins, cx)),
            )
    }

    /// Shows `panel` in the sidebar, or hides the sidebar when it shows
    /// `panel` already.
    pub(super) fn show_sidebar_panel(
        &mut self,
        panel: SidebarPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = !(self.sidebar && self.sidebar_panel == panel);
        if self.sidebar_panel != panel {
            self.catalog
                .state
                .update(cx, |tree, cx| tree.set_selected_index(None, cx));
        }
        self.sidebar_panel = panel;
        self.set_sidebar(shown, window, cx);
    }

    /// Shows or hides the sidebar. A hidden sidebar gives its focus to the
    /// SQL editor: without a focused element, shortcuts like ⌘B reach no
    /// handler. A focus outside the sidebar, like in Activity, stays.
    pub(super) fn set_sidebar(&mut self, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.sidebar && self.sidebar_focus.contains_focused(window, cx);
        self.sidebar = shown;
        if !shown {
            self.catalog
                .state
                .update(cx, |tree, cx| tree.set_selected_index(None, cx));
        }
        if !shown && had_focus {
            let editor = self.tabs[self.active].input.clone();
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
        cx.notify();
    }

    /// Activity keeps its icon, with a dot for unread errors or current work.
    fn activity_status(&self, cx: &App) -> (Option<DotStatus>, String, StatusTooltip) {
        let view = self.activity.read(cx);
        let unseen = view.activity().unseen_errors();
        let in_use = self
            .profiles
            .iter()
            .any(|profile| self.connection_dot_status(profile.id, cx) == Some(DotStatus::Working));
        let unseen = unseen
            + self
                .tabs
                .iter()
                .filter(|tab| tab.panel.unread_error)
                .count();
        let status = if unseen > 0 {
            Some(DotStatus::Error)
        } else if in_use {
            Some(DotStatus::Working)
        } else {
            None
        };
        let mut label = String::from("Activity");
        let mut statuses = Vec::new();
        if unseen > 0 {
            statuses.push(format!(
                "{unseen} Unread {}",
                if unseen == 1 { "Error" } else { "Errors" }
            ));
        }
        if in_use {
            label.push_str(", in use");
            statuses.push("In Use".into());
        }
        match unseen {
            0 => {}
            1 => label.push_str(", 1 unseen error"),
            count => label.push_str(&format!(", {count} unseen errors")),
        }
        let tooltip = StatusTooltip::new(
            "Activity",
            if statuses.is_empty() {
                "Idle".into()
            } else {
                statuses.join("\n")
            },
        );
        (status, label, tooltip)
    }

    fn activity_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, label, _) = self.activity_status(cx);
        let button = Button::new("toggle-activity")
            .ghost()
            .small()
            .selected(self.activity.read(cx).is_open())
            .icon(Icon::new(AssetIconName::Activity).small())
            .accessibility_label(label)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_activity(window, cx)));
        div()
            .id("activity-tooltip-trigger")
            .tooltip(StatusTooltip::live(
                cx.entity().downgrade(),
                Some(&ToggleActivity),
                |qrow, cx| Some(qrow.activity_status(cx).2),
            ))
            .child(DotStatus::on_button(status, button, cx))
    }

    /// The status of the last work of the active tab, like "Complete" or
    /// "Error: Connection failed". The dot tooltip of the tab shows it.
    pub fn active_tab_status(&self) -> String {
        self.tabs[self.active].status_label()
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = &self.tabs[self.active];
        let connection_name = connection_name(&self.profiles, tab.saved.profile);
        let workspace_status =
            workspace_status(self.demo, self.saver.is_some(), self.dirty.is_some());

        // Equal-width columns keep the connection name at the window center.
        // StatusBar centers its middle region only between the left and right
        // items, so labels of different widths would move it off center.
        StatusBar::new()
            .flex_shrink_0()
            // The default padding is a quarter rem, 3.5 pixels, which rounds
            // to an uneven space above and below the Activity button.
            .py(self.ui_px(4.))
            .child(h_flex().flex_1().min_w_0().child(self.sidebar_buttons(cx)))
            .child(
                div()
                    .id("current-connection")
                    .test_support()
                    .role(Role::Label)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_center()
                    .aria_label(format!("Current connection: {connection_name}"))
                    .child(connection_name.to_owned()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_end()
                    .gap_2()
                    .child(
                        div()
                            .id("workspace-status")
                            .test_support()
                            .role(Role::Status)
                            .min_w_0()
                            .truncate()
                            .aria_label(workspace_status)
                            .child(workspace_status),
                    )
                    .when(self.settings.assistant.enabled, |el| {
                        el.child(self.assistant_button(cx))
                    })
                    .child(self.activity_button(cx)),
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
            .when(horizontal, |el| {
                // Overlap the divider so display scaling cannot leave a pixel gap.
                el.child(
                    div()
                        .absolute()
                        .left(self.ui_px(2.))
                        .w(self.ui_px(3.))
                        .h_full()
                        .bg(cx.theme().background),
                )
                .child(
                    div()
                        .absolute()
                        .left(self.ui_px(2.))
                        .w(self.ui_px(3.))
                        .top_0()
                        .h(self.ui_px(TAB_BAR_HEIGHT))
                        .bg(cx.theme().tokens.tab_bar)
                        .border_b_1()
                        .border_color(cx.theme().border),
                )
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
            // The backing starts under the divider to avoid a rounded gap.
            .child(
                div()
                    .absolute()
                    .left(self.ui_px(2.))
                    .w(self.ui_px(3.))
                    .h_full()
                    .bg(cx.theme().sidebar),
            )
            .child(
                div()
                    .absolute()
                    .left(self.ui_px(2.))
                    .w(self.ui_px(3.))
                    .top_0()
                    .h(self.ui_px(TAB_BAR_HEIGHT))
                    .border_b_1()
                    .border_color(cx.theme().border),
            )
            .child(
                div()
                    .absolute()
                    .left(self.ui_px(2.))
                    .w(self.ui_px(1.))
                    .h_full()
                    .bg(if self.assistant_state.resizing.is_some() {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.assistant_state.resizing = Some((
                        event.position,
                        this.ui_px(this.settings.assistant.panel_width),
                    ));
                    cx.stop_propagation();
                }),
            )
    }
}

/// A view that renders again only when it changes. GPUI does not keep the
/// accessibility nodes of a view that it does not render again, so the view
/// renders in each frame while an accessibility client, like VoiceOver, reads
/// the window.
fn changed_view(view: AnyView, window: &Window) -> AnyElement {
    if window.is_a11y_active() {
        view.into_any_element()
    } else {
        view.cached(StyleRefinement::default().size_full())
            .into_any_element()
    }
}

impl Qrow {
    /// The width of the assistant pane in a window that is `viewport` wide.
    pub(super) fn assistant_width(&self, viewport: Pixels) -> Pixels {
        let available = viewport
            - self.ui_px(420.)
            - if self.sidebar {
                self.sidebar_width
            } else {
                px(0.)
            };
        self.ui_px(self.settings.assistant.panel_width)
            .min(available)
            .max(self.ui_px(crate::model::MIN_ASSISTANT_PANEL_WIDTH))
    }

    /// The window around the workspace and the assistant pane: the title bar,
    /// the menus, the status bar, and the commands of the window. The window
    /// renders it for each frame. The workspace and the pane are views of
    /// their own, so each renders again only when it changes.
    pub(super) fn render_shell(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let assistant_width = self.assistant_width(window.viewport_size().width);
        let activity_open = self.activity.read(cx).is_open();
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
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| {
                this.set_sidebar(!this.sidebar, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowConnections, window, cx| {
                this.show_sidebar_panel(SidebarPanel::Connections, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowSignIns, window, cx| {
                this.show_sidebar_panel(SidebarPanel::SignIns, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleAssistant, window, cx| {
                this.toggle_assistant(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleActivity, window, cx| {
                    this.toggle_activity(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &SendAssistantMessage, window, cx| {
                this.send_assistant(window, cx)
            }))
            .on_action(cx.listener(Self::open_about))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::increase_ui_scale))
            .on_action(cx.listener(Self::decrease_ui_scale))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if let Some((start, initial)) = this.assistant_state.resizing {
                    if e.pressed_button != Some(MouseButton::Left) {
                        this.assistant_state.resizing = None;
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
                        this.ui_px(crate::model::MIN_ASSISTANT_PANEL_WIDTH),
                        this.ui_px(crate::model::MAX_ASSISTANT_PANEL_WIDTH)
                            .min(available)
                            .max(this.ui_px(crate::model::MIN_ASSISTANT_PANEL_WIDTH)),
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
                    this.assistant_state.resizing = None;
                }),
            )
            .child(
                TitleBar::new().bg(cx.theme().title_bar).child(
                    // Balance TitleBar's fixed 80 px traffic-light inset. It
                    // does not follow the UI scale, so this padding must not.
                    div()
                        .flex_1()
                        .pr(px(80.))
                        .text_center()
                        .font_weight(FontWeight::MEDIUM)
                        .child(if self.demo { "Qrow (Demo)" } else { "Qrow" }),
                ),
            )
            // Activity covers the workspace and the assistant pane while it
            // is open.
            .when(activity_open, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(changed_view(self.activity.clone().into(), window)),
                )
            })
            .when(!activity_open, |el| {
                el.child(
                    h_flex()
                        .items_stretch()
                        .flex_1()
                        .min_h_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .child(changed_view(cx.entity().into(), window)),
                        )
                        .when(
                            self.settings.assistant.enabled && self.assistant_state.open,
                            |el| {
                                el.child(self.assistant_splitter(cx)).child(
                                    div().w(assistant_width).flex_shrink_0().min_h_0().child(
                                        changed_view(self.assistant_pane.clone().into(), window),
                                    ),
                                )
                            },
                        ),
                )
            })
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
                        .test_support()
                        .px_3()
                        .py_2()
                        .text_color(cx.theme().warning)
                        .role(Role::Status)
                        .aria_label(message.clone())
                        .child(message),
                )
            })
            .child(self.status_bar(cx))
    }
}
impl Render for Qrow {
    /// The workspace: the connections, the query tabs, the editor, and the
    /// results. The window shell around it has the assistant pane.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Split positions are measured window geometry. Clamp without mutating retained state during render.
        let editor_height = self
            .editor_height
            .min(window.viewport_size().height - self.ui_px(294.))
            .max(self.ui_px(100.));
        h_flex()
            .size_full()
            .items_stretch()
            .when(self.sidebar, |el| {
                el.child(
                    div()
                        .track_focus(&self.sidebar_focus)
                        .w(self.sidebar_width)
                        .flex_shrink_0()
                        .map(|el| match self.sidebar_panel {
                            SidebarPanel::Connections => {
                                el.child(self.connections(window, cx).into_any_element())
                            }
                            SidebarPanel::SignIns => {
                                el.child(self.sign_ins_sidebar(cx).into_any_element())
                            }
                        }),
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
                        // GPUI Kit's Editor has no ID setter; tests find the
                        // editor through its container.
                        div()
                            .id("sql-editor")
                            .test_support()
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
    }
}

/// Own window overlays outside Qrow so their builders can read its retained state.
pub struct WindowView {
    content: Entity<Qrow>,
}
impl WindowView {
    pub fn new(content: Entity<Qrow>) -> Self {
        Self { content }
    }
}
impl Render for WindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::Root;
        let shell = self.content.update(cx, |content, cx| {
            content.render_shell(window, cx).into_any_element()
        });
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
            .child(shell)
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
        assert_eq!(
            workspace_status(true, true, false),
            "Demo changes are not saved"
        );
        assert_eq!(
            workspace_status(false, false, false),
            "Workspace saving disabled"
        );
        assert_eq!(workspace_status(false, true, true), "Saving");
        assert_eq!(workspace_status(false, true, false), "Workspace saved");
    }
}
