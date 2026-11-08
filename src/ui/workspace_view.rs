use super::assistant_view::ThreadStatus;
use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::tooltip::ManagedTooltipExt as _;
use gpui_kit::component::{
    Icon, Selectable as _, TitleBar,
    alert::Alert,
    h_flex,
    input::Editor,
    shimmer::ShimmerText,
    status_bar::StatusBar,
    tab::{Tab as QueryTab, TabBar},
    v_flex,
};

pub(super) const TAB_BAR_HEIGHT: f32 = 36.;

/// The left padding of the Linux title bar, in rems. It aligns the ink of the
/// application menu icon with the sidebar title: the 0.75 rem title inset,
/// less the 0.3125 rem between a small icon button and its 0.875 rem icon,
/// less the blank 3/24 at the left of the menu icon.
#[cfg(target_os = "linux")]
const APPLICATION_MENU_INSET_REMS: f32 = 0.75 - 0.3125 - 0.875 * 3. / 24.;

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
    #[cfg(target_os = "linux")]
    fn open_application_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let assistant_enabled = self.settings.assistant.enabled;
        let catalog_focused = self.catalog_has_focus(window, cx);
        let focus = window
            .focused(cx)
            .unwrap_or_else(|| self.tabs[self.active].input.read(cx).focus_handle(cx));
        let file_focus = focus.clone();
        let edit_focus = focus.clone();
        let view_focus = focus.clone();
        self.open_context_menu_from(
            // Under the menu button, below the 34 px title bar.
            point(window.rem_size() * APPLICATION_MENU_INSET_REMS, px(34.)),
            Some(focus),
            move |menu, window, cx| {
                menu.item(PopupMenuItem::new("About Qrow").action(Box::new(OpenAbout)))
                    .item(PopupMenuItem::new("Settings…").action(Box::new(OpenSettings)))
                    .separator()
                    .submenu("File", window, cx, move |menu, _, _| {
                        menu.action_context(file_focus.clone())
                            .item(PopupMenuItem::new("New query tab").action(Box::new(NewTab)))
                            .item(PopupMenuItem::new("Close tab").action(Box::new(CloseTab)))
                    })
                    .submenu("Edit", window, cx, move |menu, _, _| {
                        use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
                        let copy: Box<dyn Action> = if catalog_focused {
                            Box::new(CopyCatalogName)
                        } else {
                            Box::new(Copy)
                        };
                        menu.action_context(edit_focus.clone())
                            .item(PopupMenuItem::new("Undo").action(Box::new(Undo)))
                            .item(PopupMenuItem::new("Redo").action(Box::new(Redo)))
                            .separator()
                            .item(PopupMenuItem::new("Cut").action(Box::new(Cut)))
                            .item(PopupMenuItem::new("Copy").action(copy))
                            .item(PopupMenuItem::new("Paste").action(Box::new(Paste)))
                            .item(PopupMenuItem::new("Select all").action(Box::new(SelectAll)))
                    })
                    .submenu("View", window, cx, move |menu, _, _| {
                        let menu = menu
                            .action_context(view_focus.clone())
                            .item(
                                PopupMenuItem::new("Connections").action(Box::new(ShowConnections)),
                            )
                            .item(PopupMenuItem::new("Sign-Ins").action(Box::new(ShowSignIns)))
                            .item(
                                PopupMenuItem::new("Toggle sidebar")
                                    .action(Box::new(ToggleSidebar)),
                            )
                            .item(
                                PopupMenuItem::new("Toggle results")
                                    .action(Box::new(ToggleResults)),
                            );
                        let menu = if assistant_enabled {
                            menu.item(
                                PopupMenuItem::new("Toggle assistant")
                                    .action(Box::new(ToggleAssistant)),
                            )
                            .item(
                                PopupMenuItem::new("Maximize assistant")
                                    .action(Box::new(MaximizeAssistant)),
                            )
                            .item(
                                PopupMenuItem::new("Minimize assistant")
                                    .action(Box::new(MinimizeAssistant)),
                            )
                        } else {
                            menu
                        };
                        menu.item(PopupMenuItem::new("Activity").action(Box::new(ToggleActivity)))
                    })
                    .separator()
                    .item(PopupMenuItem::new("Quit Qrow").action(Box::new(Quit)))
            },
            window,
            cx,
        );
    }

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
        // Tabs keep at least 10.5 rems, so a short title shows whole, and
        // the selected tab keeps its whole title. When the other tabs
        // overflow the strip but 8.5 rems each would not, they shrink to
        // share the rest of the strip instead, so a few tabs never scroll.
        // The widths are from the last frame; the tab bar draws again when
        // they change.
        let rem = cx.theme().font_size;
        let room = self.tab_scroll.bounds().size.width - rem * 0.75;
        let selected = visible.iter().position(|index| *index == self.active);
        // Width from the shared minimum is not intrinsic title width. Reserve
        // extra room only for content that extends beyond the previous minimum.
        // Otherwise equally short tabs cannot share the available room evenly.
        let previous_minimum = self.tab_min_width.get().unwrap_or(rem * 10.5);
        let shared_width = room / visible.len().max(1) as f32;
        let reserved_width = visible
            .iter()
            .enumerate()
            .filter_map(|(position, index)| {
                let width = self.tab_scroll.bounds_for_item(position)?.size.width;
                Some(if *index == self.active {
                    width
                } else {
                    width.min(self.ui_px(MAX_QUERY_TAB_WIDTH))
                })
            })
            .filter(|width| *width > previous_minimum + px(0.5))
            .max()
            .unwrap_or(px(0.))
            .max(shared_width);
        let others = visible
            .len()
            .saturating_sub(usize::from(selected.is_some())) as f32;
        let fit = (others > 0. && room > px(0.) && rem * 10.5 * others + reserved_width > room)
            .then(|| (room - reserved_width) / others)
            .filter(|&width| width >= rem * 8.5);
        self.tab_min_width.set(Some(fit.unwrap_or(rem * 10.5)));
        TabBar::new("query-tabs")
            .track_scroll(&self.tab_scroll)
            // The selected tab shows its whole title, up to the visible strip.
            .grow_selected(true)
            .fit_width(fit)
            // Long titles end in an ellipsis, and a short bar shrinks the
            // titles before it scrolls, so a tab keeps its close button whole.
            .max_width(self.ui_px(MAX_QUERY_TAB_WIDTH))
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
            .children(visible.iter().enumerate().map(|(position, index)| {
                let index = *index;
                let tab = &self.tabs[index];
                let id = tab.saved.id;
                // The close button has a tooltip of its own, which the tab
                // tooltip must not replace when the pointer enters both.
                let over_close = std::rc::Rc::new(std::cell::Cell::new(false));
                // A tab that an edge of the strip cuts hides its dot and close
                // button, so no lone "×" shows at the edge. A click on it
                // selects it, which scrolls it into view.
                let cut = self
                    .tab_scroll
                    .bounds_for_item(position)
                    .is_some_and(|item| {
                        let (view, offset) = (self.tab_scroll.bounds(), self.tab_scroll.offset().x);
                        item.left() + offset < view.left() - px(0.5)
                            || item.right() + offset > view.right() + px(0.5)
                    });
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
                    // Every tab shows its full title and its status in a tooltip,
                    // so a cut title can be read. Tab tooltips use the managed
                    // overlay, which hides them when the strip scrolls.
                    .managed_tooltip_when(
                        {
                            let over_close = over_close.clone();
                            move || !over_close.get()
                        },
                        StatusTooltip::live(cx.entity().downgrade(), None, move |qrow, _| {
                            qrow.tabs
                                .iter()
                                .find(|tab| tab.saved.id == id)
                                .map(Tab::status_tooltip)
                        }),
                    )
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
                            .when(cut, |el| el.invisible())
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
                                        .managed_tooltip(tooltip)
                                        .child(status.dot(cx)),
                                )
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("close-tab-hover-{id}")))
                                    .on_hover(move |hovered, _, _| over_close.set(*hovered))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("close-tab-tip-{id}")))
                                            .managed_tooltip(
                                                StatusTooltip::new("Close tab", "")
                                                    .for_action(&CloseTab, None),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "close-tab-{}",
                                                    tab.saved.id
                                                )))
                                                .ghost()
                                                .small()
                                                .icon(IconName::Close)
                                                .accessibility_label(format!(
                                                    "Close {}",
                                                    tab.saved.title
                                                ))
                                                .disabled(tab.busy || assistant_busy)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.close_tab(index, window, cx);
                                                    },
                                                )),
                                            ),
                                    ),
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
                        .accessibility_label("New tab")
                        .map(|mut button| {
                            button.interactivity().tooltip(
                                StatusTooltip::new("New tab", "").for_action(&NewTab, None),
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
        let authenticating = active.is_some_and(|id| self.external_authentication_pending(id));
        h_flex()
            .id("query-toolbar")
            .test_support()
            .h_12()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(!tab.busy && !authenticating, |el| {
                let button = Button::new("run")
                    .primary()
                    .small()
                    .icon(IconName::ArrowRight)
                    .label("Run")
                    .map(|mut button| {
                        button.interactivity().tooltip(
                            StatusTooltip::new("Run query", "")
                                .detail("Run the selected SQL or the statement at the cursor.")
                                .for_action(&RunQuery, None),
                        );
                        button
                    })
                    .disabled(active.is_none() && !self.demo)
                    .on_click(cx.listener(|this, _, window, cx| this.run(&RunQuery, window, cx)));
                el.child(button)
            })
            .when(tab.busy || authenticating, |el| {
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
            // Hidden, not disabled, without an open session: a gray label
            // beside Run reads as a broken control.
            .when(tab.can_disconnect(), |el| {
                el.child(
                    Button::new("disconnect")
                        .ghost()
                        .small()
                        .label("Disconnect")
                        .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
                )
            })
            .when(authenticating, |el| {
                el.child(
                    div()
                        .id("connection-authentication-progress")
                        .test_support()
                        .role(Role::Status)
                        .aria_label("Waiting for Trino authentication")
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Waiting for Trino authentication…"),
                )
            })
            .child(div().flex_1())
    }

    fn toggle_results(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            return;
        }
        self.settings.results_minimized = !self.settings.results_minimized;
        self.resize = None;
        if !self.settings.results_minimized
            && !self.assistant_maximized()
            && !self.activity.read(cx).is_open()
        {
            self.tabs[self.active].panel.content_visible();
        }
        self.changed(cx);
    }

    pub(super) fn restore_results(&mut self, cx: &mut Context<Self>) {
        if self.settings.results_minimized {
            self.settings.results_minimized = false;
            self.changed(cx);
        }
    }

    pub(super) fn results_toggle_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let minimized = self.settings.results_minimized;
        let label = if minimized { "Expand" } else { "Collapse" };
        Button::new("toggle-results")
            .ghost()
            .small()
            .icon(if minimized {
                IconName::ChevronUp
            } else {
                IconName::ChevronDown
            })
            .accessibility_label(label)
            .map(|mut button| {
                button
                    .interactivity()
                    .tooltip(StatusTooltip::new(label, "").for_action(&ToggleResults, None));
                button
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_results(window, cx)))
    }

    fn query_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.settings.results_minimized {
            return self.result_tabs(cx).into_any_element();
        }
        let content = if self.tabs[self.active].panel.selected == Panel::Output {
            self.output_panel(cx)
        } else {
            let table = &self.tabs[self.active].shown_result().table;
            // Without a result set the toolbar has nothing to count, export,
            // or pin.
            let has_result = !table.read(cx).delegate().columns.is_empty();
            results::selection_boundary(table)
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .when(has_result, |el| el.child(self.result_toolbar(cx)))
                .child(div().flex_1().min_h_0().min_w_0().child(results::view(
                    table,
                    self.dialog_open(),
                    self.settings.ui_scale,
                    cx,
                )))
                .into_any_element()
        };
        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(self.result_tabs(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .opacity(
                        motion::content(
                            "query-panel",
                            format!(
                                "{}-{:?}-{}",
                                self.tabs[self.active].saved.id,
                                self.tabs[self.active].panel.selected,
                                if self.tabs[self.active].panel.selected == Panel::Results {
                                    self.tabs[self.active].shown_result().id
                                } else {
                                    0
                                }
                            ),
                            0,
                            self.settings.smooth_transitions,
                            window,
                            cx,
                        )
                        .opacity(),
                    )
                    .child(content),
            )
            .into_any_element()
    }

    /// The Assistant button of the status bar. Its dot shows the most urgent
    /// state of all conversations.
    fn assistant_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.assistant_status();
        let accessibility_label = match status {
            ThreadStatus::Failed => "Toggle assistant, reply failed",
            ThreadStatus::Ready => "Toggle assistant, reply ready",
            ThreadStatus::Approval => "Toggle assistant, waiting for approval",
            ThreadStatus::Working => "Toggle assistant, working",
            _ => "Toggle assistant",
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
        let mut label = String::from("Sign-Ins");
        let mut statuses = Vec::new();
        if working {
            label.push_str(", sign-in in progress");
            statuses.push("Sign-in running".to_owned());
        }
        match attention {
            0 => {}
            1 => {
                label.push_str(", 1 sign-in needs attention");
                statuses.insert(0, "1 needs attention".to_owned());
            }
            count => {
                label.push_str(&format!(", {count} sign-ins need attention"));
                statuses.insert(0, format!("{count} need attention"));
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
            StatusTooltip::new("Sign-Ins", statuses.join("\n")),
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
                "{unseen} unread {}",
                if unseen == 1 { "error" } else { "errors" }
            ));
        }
        if in_use {
            label.push_str(", in use");
            statuses.push("In use".into());
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

        // One line of information: the sidebar panels and the connection of
        // the tab on the left, the workspace state and tools on the right.
        let connected = tab.connected.then_some(DotStatus::Connected);
        StatusBar::new()
            .flex_shrink_0()
            // The default padding is a quarter rem, 3.5 pixels, which rounds
            // to an uneven space above and below the Activity button.
            .py(self.ui_px(4.))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(self.sidebar_buttons(cx))
                    .child(div().w_px().h_4().flex_shrink_0().bg(cx.theme().border))
                    .child(
                        h_flex()
                            .id("current-connection")
                            .test_support()
                            .role(Role::Label)
                            .min_w_0()
                            .gap_1p5()
                            .aria_label(format!("Current connection: {connection_name}"))
                            .when_some(connected, |el, status| el.child(status.dot(cx)))
                            .child(div().min_w_0().truncate().child(connection_name.to_owned())),
                    ),
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
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_1()
                            .when(self.settings.assistant.enabled, |el| {
                                el.child(self.assistant_button(cx))
                            })
                            .child(self.activity_button(cx)),
                    ),
            )
    }

    fn splitter(&self, horizontal: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = div()
            .id(if horizontal {
                "sidebar-splitter"
            } else {
                "editor-splitter"
            })
            .test_support()
            .absolute()
            .when(horizontal, |el| {
                el.left(self.ui_px(-2.))
                    .w(self.ui_px(5.))
                    .h_full()
                    .cursor(CursorStyle::ResizeLeftRight)
            })
            .when(!horizontal, |el| {
                el.top(self.ui_px(-2.))
                    .h(self.ui_px(5.))
                    .w_full()
                    .cursor(CursorStyle::ResizeUpDown)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.resize = Some((
                        horizontal,
                        e.position,
                        if horizontal {
                            this.shown_sidebar_width(window.viewport_size().width)
                        } else {
                            this.editor_height
                        },
                    ));
                    cx.stop_propagation();
                }),
            );
        // Reserve exactly the painted divider's width. The wider drag target
        // stays out of layout so separately rounded margins cannot leave a gap.
        div()
            .id(if horizontal {
                "sidebar-divider"
            } else {
                "editor-divider"
            })
            .test_support()
            .relative()
            .flex_shrink_0()
            .bg(
                if self.resize.is_some_and(|(axis, _, _)| axis == horizontal) {
                    cx.theme().primary
                } else {
                    cx.theme().border
                },
            )
            .when(horizontal, |el| el.w(self.ui_px(1.)).h_full())
            .when(!horizontal, |el| el.h(self.ui_px(1.)).w_full())
            .child(handle)
    }

    fn assistant_splitter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("assistant-splitter")
            .test_support()
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

/// The widest query tab, in points of the 100% scale.
const MAX_QUERY_TAB_WIDTH: f32 = 240.;

/// The least width that the assistant pane leaves to the editor and the
/// results. Below it, the pane narrows, which also collapses its
/// conversation list.
const MIN_WORK_AREA_WIDTH: f32 = 560.;
/// The width of the work area that a narrow window keeps before the sidebar
/// keeps its own width.
const SIDEBAR_YIELDS_TO_WIDTH: f32 = 560.;

impl Qrow {
    /// The width of the sidebar in a window that is `viewport` wide. A
    /// narrow window narrows the sidebar, down to its least width, before
    /// the work area gets narrower than [`SIDEBAR_YIELDS_TO_WIDTH`].
    pub(super) fn shown_sidebar_width(&self, viewport: Pixels) -> Pixels {
        let least = self.ui_px(180.);
        self.sidebar_width
            .min((viewport - self.ui_px(SIDEBAR_YIELDS_TO_WIDTH)).max(least))
    }

    pub(super) fn assistant_maximized(&self) -> bool {
        self.settings.assistant.enabled
            && self.assistant_state.open
            && self.assistant_state.maximized
    }

    /// The width of the assistant pane in a window that is `viewport` wide.
    pub(super) fn assistant_width(&self, viewport: Pixels) -> Pixels {
        if self.assistant_state.maximized {
            return viewport;
        }
        let available = viewport
            - self.ui_px(MIN_WORK_AREA_WIDTH)
            - if self.sidebar {
                self.shown_sidebar_width(viewport)
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
        let assistant_shown = self.settings.assistant.enabled && self.assistant_state.open;
        let animated_assistant_width = motion::panel_extent(
            "assistant-sidebar",
            assistant_shown,
            assistant_width,
            self.settings.smooth_transitions && self.settings.assistant.enabled,
            self.assistant_state.resizing.is_some(),
            window,
            cx,
        );
        // Opening keeps fixed-width content; maximize/restore follows the
        // presented width so controls and wrapping stay inside the moving pane.
        let assistant_content_width =
            if self.assistant_maximized() || animated_assistant_width > assistant_width {
                animated_assistant_width.max(px(1.))
            } else {
                assistant_width
            };
        if assistant_shown {
            self.assistant_pane.update(cx, |pane, cx| {
                pane.set_layout_width(assistant_content_width, cx)
            });
        }
        let activity_open = self.activity.read(cx).is_open();
        let activity_width = self.workspace_body_size.get().width;
        let animated_activity_width = motion::panel_extent(
            "activity-panel",
            activity_open,
            activity_width,
            self.settings.smooth_transitions,
            false,
            window,
            cx,
        );
        v_flex()
            .relative()
            .size_full()
            .key_context("Qrow")
            .track_focus(&self.focus)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::format_sql))
            .on_action(
                cx.listener(|this, _: &RunAndExport, window, cx| this.open_run_export(window, cx)),
            )
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::select_tab))
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
            .on_action(
                cx.listener(|this, _: &ToggleResults, window, cx| this.toggle_results(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleAssistant, window, cx| {
                this.toggle_assistant(window, cx)
            }))
            .on_action(cx.listener(|this, _: &MaximizeAssistant, window, cx| {
                this.set_assistant_maximized(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MinimizeAssistant, window, cx| {
                this.set_assistant_maximized(false, window, cx)
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
                        - this.ui_px(MIN_WORK_AREA_WIDTH)
                        - if this.sidebar {
                            this.shown_sidebar_width(window.viewport_size().width)
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
                TitleBar::new()
                    .bg(cx.theme().title_bar)
                    .on_close_window(
                        cx.listener(|this, _, window, cx| this.request_quit(window, cx)),
                    )
                    .when(cfg!(target_os = "linux"), |bar| {
                        #[cfg(target_os = "linux")]
                        let bar = bar.pl(rems(APPLICATION_MENU_INSET_REMS)).child(
                            Button::new("application-menu")
                                .ghost()
                                .small()
                                .icon(Icon::new(AssetIconName::Menu).small())
                                .accessibility_label("Application menu")
                                .tooltip("Application menu")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_application_menu(window, cx)
                                })),
                        );
                        bar
                    })
                    .child(
                        div()
                            .flex_1()
                            // Balance the fixed macOS traffic-light inset.
                            .when(cfg!(target_os = "macos"), |el| el.pr(px(80.)))
                            .text_center()
                            .font_weight(FontWeight::MEDIUM)
                            .child(if self.demo { "Qrow (Demo)" } else { "Qrow" }),
                    ),
            )
            // Activity takes over this area from the right. Its fixed-width
            // contents keep their layout during the transition.
            .child(
                div()
                    .id("workspace-body")
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .map(|el| {
                        use gpui_kit::component::ElementExt as _;
                        let size = self.workspace_body_size.clone();
                        el.on_prepaint(move |bounds, window, _| {
                            if size.replace(bounds.size) != bounds.size {
                                window.request_animation_frame();
                            }
                        })
                    })
                    .when(!activity_open, |el| {
                        el.child(
                            h_flex()
                                .items_stretch()
                                .size_full()
                                .min_h_0()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .min_h_0()
                                        .overflow_hidden()
                                        .when(!self.assistant_maximized(), |el| {
                                            el.child(changed_view(cx.entity().into(), window))
                                        }),
                                )
                                .when(assistant_shown || animated_assistant_width > px(0.), |el| {
                                    el.when(!self.assistant_maximized(), |el| {
                                        el.child(if assistant_shown {
                                            self.assistant_splitter(cx).into_any_element()
                                        } else {
                                            div()
                                                .w(self.ui_px(1.))
                                                .h_full()
                                                .bg(cx.theme().border)
                                                .into_any_element()
                                        })
                                    })
                                    .child(
                                        motion::sidebar_panel(
                                            "assistant-motion",
                                            assistant_shown,
                                            animated_assistant_width,
                                            assistant_content_width,
                                            changed_view(
                                                self.assistant_pane.clone().into(),
                                                window,
                                            )
                                            .into_any_element(),
                                            cx,
                                        ),
                                    )
                                }),
                        )
                    })
                    .when(activity_open || animated_activity_width > px(0.), |el| {
                        el.child(motion::takeover_panel(
                            activity_open,
                            animated_activity_width,
                            activity_width,
                            changed_view(self.activity.clone().into(), window),
                            cx,
                        ))
                    }),
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
                        .test_support()
                        .aria_label(message.clone())
                        .child(Alert::warning("workspace-alert", message).banner()),
                )
            })
            .child({
                let height = self.status_bar_height.clone();
                div()
                    .relative()
                    .w_full()
                    .flex_shrink_0()
                    .child(self.status_bar(cx))
                    .map(|el| {
                        use gpui_kit::component::ElementExt as _;
                        el.on_prepaint(move |bounds, _, _| height.set(bounds.size.height))
                    })
            })
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
        let results_expansion = motion::panel_fraction(
            "query-results",
            !self.settings.results_minimized,
            self.settings.smooth_transitions,
            self.resize.is_some_and(|(horizontal, _, _)| !horizontal),
            window,
            cx,
        );
        let results_settled = !self.settings.results_minimized && results_expansion == 1.;
        let sidebar_width = self.shown_sidebar_width(window.viewport_size().width);
        let animated_sidebar_width = motion::panel_extent(
            "connections-sidebar",
            self.sidebar,
            sidebar_width,
            self.settings.smooth_transitions,
            self.resize.is_some_and(|(horizontal, _, _)| horizontal),
            window,
            cx,
        );
        let sidebar_content = motion::content(
            "sidebar-panel",
            format!("{:?}", self.sidebar_panel),
            0,
            self.settings.smooth_transitions,
            window,
            cx,
        );
        let editor_content = motion::content(
            "query-editor",
            self.tabs[self.active].saved.id.to_string(),
            self.tabs[self.active].revision,
            self.settings.smooth_transitions,
            window,
            cx,
        );
        let editor = Editor::new(&self.tabs[self.active].input)
            .smooth_caret(self.settings.editor_smooth_caret)
            .smooth_scrolling(self.settings.editor_smooth_scrolling)
            .appearance(false)
            .context_menu_with_capabilities(|_, capabilities, _, cx| {
                gpui_kit::component::native_menu::NativeMenu::from(format_sql::editor_menu(
                    capabilities,
                    cx.read_from_clipboard().is_some(),
                ))
            })
            .font_family(self.settings.editor_font_family.clone())
            .text_size(self.ui_px(self.settings.editor_font_size))
            .size_full()
            .aria_label("SQL Editor");
        h_flex()
            .size_full()
            .items_stretch()
            .when(self.sidebar || animated_sidebar_width > px(0.), |el| {
                el.child(motion::sidebar_panel(
                    "sidebar-motion",
                    self.sidebar,
                    animated_sidebar_width,
                    sidebar_width,
                    div()
                        .relative()
                        .left(sidebar_content.offset())
                        .opacity(sidebar_content.opacity())
                        .track_focus(&self.sidebar_focus)
                        .size_full()
                        .map(|el| match self.sidebar_panel {
                            SidebarPanel::Connections => {
                                el.child(self.connections(window, cx).into_any_element())
                            }
                            SidebarPanel::SignIns => {
                                el.child(self.sign_ins_sidebar(cx).into_any_element())
                            }
                        })
                        .into_any_element(),
                    cx,
                ))
                .child(if self.sidebar {
                    self.splitter(true, cx).into_any_element()
                } else {
                    div()
                        .w(self.ui_px(1.))
                        .h_full()
                        .bg(cx.theme().border)
                        .into_any_element()
                })
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
                            .overflow_hidden()
                            .test_support()
                            .flex_basis(editor_height)
                            .flex_grow(1. - results_expansion)
                            .flex_shrink_0()
                            .min_h_0()
                            .min_w_0()
                            .line_height(relative(self.settings.editor_line_height))
                            .child(
                                div()
                                    .opacity(editor_content.opacity())
                                    .size_full()
                                    .child(editor),
                            ),
                    )
                    .when(results_settled, |el| el.child(self.splitter(false, cx)))
                    .when(!results_settled, |el| {
                        el.child(div().h(self.ui_px(1.)).bg(cx.theme().border))
                    })
                    .child(
                        div()
                            .id("results-motion")
                            .test_support()
                            .flex_basis(self.ui_px(result_tabs::RESULT_TAB_BAR_HEIGHT))
                            .flex_grow(results_expansion)
                            .flex_shrink_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.query_panel(window, cx)),
                    ),
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
                let primary = if cfg!(target_os = "macos") {
                    key.modifiers.platform && !key.modifiers.control
                } else {
                    key.modifiers.control && !key.modifiers.platform
                };
                if !primary || key.modifiers.alt {
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
