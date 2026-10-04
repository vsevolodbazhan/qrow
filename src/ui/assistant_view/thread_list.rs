//! The list of conversations and its search.
use super::*;

pub(super) fn conversation_age(last_activity: u64, now: u64) -> String {
    if last_activity == 0 {
        return "Earlier".into();
    }
    let elapsed = now.saturating_sub(last_activity);
    if elapsed < 60 {
        "Now".into()
    } else if elapsed < 3_600 {
        format!("{}m", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h", elapsed / 3_600)
    } else if elapsed < 604_800 {
        format!("{}d", elapsed / 86_400)
    } else {
        format!("{}w", elapsed / 604_800)
    }
}

impl Qrow {
    /// Names the connection of a conversation. A conversation whose tab
    /// closed names the connection of its next tab.
    pub(super) fn conversation_place(&self, conversation: &AssistantConversation) -> String {
        let (profile, closed) = match conversation
            .tab_id
            .and_then(|tab| self.tabs.iter().find(|candidate| candidate.saved.id == tab))
        {
            Some(tab) => (tab.saved.profile, false),
            None => (conversation.detached_profile, true),
        };
        let name = profile
            .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
            .map_or("No connection", |profile| profile.name.as_str());
        if closed {
            format!("{name} · Tab closed")
        } else {
            name.to_owned()
        }
    }
}

impl AssistantPane {
    /// Whether the lowercase `text` contains `search`, which is lowercase.
    fn search_matches(&self, text: &str, search: &str) -> bool {
        let mut lowercase = self.lowercase.borrow_mut();
        if let Some(cached) = lowercase.get(text) {
            return cached.contains(search);
        }
        let cached = text.to_lowercase();
        let matches = cached.contains(search);
        lowercase.insert(text.to_owned(), cached);
        matches
    }

    pub(super) fn thread_list(
        &self,
        qrow: &Qrow,
        narrow: bool,
        width: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut conversations: Vec<_> = qrow.assistant.conversations.iter().collect();
        conversations.sort_by_key(|conversation| std::cmp::Reverse(conversation.last_activity));
        let search = self.thread_search.read(cx).value().to_lowercase();
        if search.is_empty() {
            self.lowercase.borrow_mut().clear();
        }
        let displayed = qrow.displayed_thread();
        v_flex()
            .w(if narrow { width } else { qrow.ui_px(230.) })
            .max_w_full()
            .h_full()
            .flex_shrink_0()
            .bg(cx.theme().sidebar)
            .when(!narrow, |list| list.border_l_1())
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .h(qrow.ui_px(36.))
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .when(narrow, |row| {
                        row.child(
                            Button::new("assistant-back-to-thread")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowLeft)
                                .accessibility_label("Back to Conversation")
                                .tooltip("Back to Conversation")
                                .on_click(cx.listener(|pane, _, window, cx| {
                                    pane.thread_list_override = Some(false);
                                    pane.read_visible_reply(window, cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Input::new(&self.thread_search)
                            .small()
                            .flex_1()
                            .min_w_0()
                            .aria_label("Search Conversations"),
                    ),
            )
            .child(
                v_flex()
                    .id("assistant-thread-list")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .children(
                        conversations
                            .into_iter()
                            .map(|conversation| {
                                (conversation, qrow.conversation_place(conversation))
                            })
                            .filter(|(conversation, place)| {
                                search.is_empty()
                                    || self.search_matches(&conversation.title, &search)
                                    || self.search_matches(place, &search)
                            })
                            .map(|(conversation, place)| {
                                let id = conversation.thread_id.clone();
                                let status = qrow.thread_status(&id);
                                let tooltip_thread = id.clone();
                                let tooltip = StatusTooltip::live(self.qrow.clone(), None, move |qrow, _| {
                                    qrow.assistant.conversations.iter().find(|conversation| conversation.thread_id == tooltip_thread).map(|conversation| {
                                        StatusTooltip::new(conversation.title.clone(), qrow.thread_status(&tooltip_thread).tooltip_status())
                                    })
                                });
                                let row = h_flex()
                                    .id(SharedString::from(format!("assistant-thread-row-{id}")))
                                    .tooltip(tooltip)
                                    .w_full()
                                    .h(qrow.ui_px(48.))
                                    .flex_shrink_0();
                                let selected = displayed.as_deref() == Some(id.as_str());
                                let label = conversation.title.clone();
                                let generating = qrow.assistant_title_generating(&id);
                                let accessible = format!(
                                    "{label}, {place}{}{}",
                                    status.accessible_suffix(),
                                    if generating { ", generating title" } else { "" }
                                );
                                let menu_thread = id.clone();
                                let button = Button::new(format!("assistant-thread-{id}"))
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h_full()
                                    .justify_start()
                                    .accessibility_label(accessible)
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_2()
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .when(generating, |title| {
                                                                title.child(
                                                                    ShimmerText::new(label.clone())
                                                                        .id(format!("assistant-title-row-{id}")),
                                                                )
                                                            })
                                                            .when(!generating, |title| title.child(label)),
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(format!(
                                                                "{place} · {}",
                                                                conversation_age(
                                                                    conversation.last_activity,
                                                                    unix_now_seconds(),
                                                                )
                                                            )),
                                                    ),
                                            )
                                            .when_some(
                                                qrow.assistant_status_dot(status, cx),
                                                |row, dot| {
                                                    row.child(div().flex_none().child(dot))
                                                },
                                            ),
                                    )
                                    .selected(selected)
                                    .when(selected, |button| {
                                        button
                                            .bg(cx.theme().sidebar_accent)
                                            .text_color(cx.theme().sidebar_accent_foreground)
                                    })
                                    .on_click(on_qrow(&self.qrow, move |this, _, window, cx| {
                                        this.select_assistant_thread(&id, window, cx);
                                    }));
                                row.child(button)
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        on_qrow(&self.qrow, move |_, event: &MouseDownEvent, window, cx| {
                                            let (thread, position) = (menu_thread.clone(), event.position);
                                            cx.defer_in(window, move |this, window, cx| {
                                                this.open_thread_menu(&thread, position, window, cx)
                                            });
                                        }),
                                    )
                                    .into_any_element()
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn conversation_age_handles_unknown_and_recent_activity() {
        assert_eq!(conversation_age(0, 1_000), "Earlier");
        assert_eq!(conversation_age(990, 1_000), "Now");
        assert_eq!(conversation_age(880, 1_000), "2m");
        assert_eq!(conversation_age(1_000, 999), "Now");
    }
}
