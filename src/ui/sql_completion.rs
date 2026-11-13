//! The autocomplete popup of the SQL editor. ⌃Space opens it with the
//! schemas, tables, views, and columns that fit the cursor, from the cached
//! metadata of the tab's connection. [`crate::completion`] finds the
//! suggestions; this module owns the popup, its keys, and the reads that it
//! asks the catalog worker for.

use super::{Qrow, ShowCompletions};
use crate::catalog::Scope;
use crate::completion::{self, Completion, Item, ItemKind, Need, Request};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, MoveDown, MovePageDown, MovePageUp, MoveUp,
};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use uuid::Uuid;

/// The most rows that the list shows without scrolling.
const MAX_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 26.;
const MIN_WIDTH: f32 = 320.;
const MAX_WIDTH: f32 = 440.;
const DETAILS_WIDTH: f32 = 260.;
/// The padding of the popup and of a row, the icon, and the gaps of a row.
const ROW_CHROME: f32 = 4. * 2. + 8. * 2. + 14. + 8. + 2.;
const GAP: f32 = 8.;

/// Why the popup changes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Trigger {
    /// The user asked for suggestions.
    Open,
    /// The text or the cursor of the editor changed.
    Edit,
    /// The cached metadata changed.
    Metadata,
}

/// The open popup.
pub(super) struct CompletionPopup {
    tab: Uuid,
    profile: Uuid,
    request: Request,
    completion: Completion,
    /// Whether the connection reads metadata at all.
    browses: bool,
    /// Whether the window holds the cache of the connection.
    loaded: bool,
    /// Whether Qrow ever read the schema list of the connection.
    read: bool,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// The reads that this popup started, so that it starts each once.
    requested: HashSet<Need>,
}

impl CompletionPopup {
    /// The query tab of the popup.
    pub(super) fn tab(&self) -> Uuid {
        self.tab
    }
}

/// What the popup shows when it has no suggestions.
enum Body {
    Items,
    Loading(Vec<Text>),
    Failed {
        title: &'static str,
        message: Vec<Text>,
    },
    /// The connection has no cached metadata.
    NotRead,
    /// The connection does not read metadata.
    Off,
    NoSuggestions,
}

/// A part of a message: plain text or a name in code font.
enum Text {
    Plain(String),
    Code(String),
}

/// The measured layout of the list.
struct Layout {
    width: Pixels,
    /// Whether the kinds and types at the right of the rows show.
    details: bool,
    /// Whether the list cuts each name.
    cut: Vec<bool>,
}

impl Qrow {
    /// ⌃Space: opens the popup, or runs its Refresh or Retry when it is open.
    pub(super) fn show_completions(
        &mut self,
        _: &ShowCompletions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.tabs[self.active].saved.id;
        if let Some(popup) = self.completion.as_ref().filter(|popup| popup.tab == tab) {
            let profile = popup.profile;
            let scopes = self.completion_retry_scopes(popup);
            if !scopes.is_empty() {
                for scope in scopes {
                    self.refresh_catalog(profile, scope, cx);
                }
                self.update_completion(Trigger::Metadata, cx);
                return;
            }
        }
        self.update_completion(Trigger::Open, cx);
    }

    /// The reads that Refresh or Retry starts: the stale schemas of the
    /// list, the failed reads, or the whole connection when it was never
    /// read. Reads in progress are left out.
    fn completion_retry_scopes(&self, popup: &CompletionPopup) -> Vec<Scope> {
        let scopes: Vec<Scope> = if !popup.read {
            vec![Scope::Connection]
        } else if !popup.completion.errors.is_empty() {
            popup
                .completion
                .errors
                .iter()
                .map(|(need, _)| need_scope(need))
                .collect()
        } else {
            popup
                .completion
                .stale
                .iter()
                .map(|schema| Scope::Schema(schema.clone()))
                .collect()
        };
        scopes
            .into_iter()
            .filter(|scope| !self.catalog.reads(popup.profile, scope))
            .collect()
    }

    /// Finds the suggestions for the cursor of the active tab again.
    pub(super) fn update_completion(&mut self, trigger: Trigger, cx: &mut Context<Self>) {
        let tab = &self.tabs[self.active];
        let tab_id = tab.saved.id;
        if trigger != Trigger::Open
            && self
                .completion
                .as_ref()
                .is_none_or(|popup| popup.tab != tab_id)
        {
            self.close_completion(cx);
            return;
        }
        let Some(profile) = tab
            .saved
            .profile
            .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
            .cloned()
        else {
            self.close_completion(cx);
            return;
        };
        let input = tab.input.read(cx);
        let text = input.value();
        let cursor = input.cursor();
        let Some(request) = completion::request(&text, cursor, profile.database_type) else {
            self.close_completion(cx);
            return;
        };
        let previous = self.completion.take().filter(|popup| popup.tab == tab_id);
        // The editor notifies also when nothing that the popup follows
        // changed, for example when the caret blinks.
        if trigger == Trigger::Edit
            && previous
                .as_ref()
                .is_some_and(|popup| popup.request == request)
        {
            self.completion = previous;
            return;
        }
        // The popup follows one name. Typing past it, or moving the cursor
        // to another name, closes it.
        if trigger == Trigger::Edit
            && previous
                .as_ref()
                .is_none_or(|popup| popup.request.start != request.start)
        {
            cx.notify();
            return;
        }
        let browses = profile.catalog.browses();
        if browses {
            self.ensure_catalog(profile.id);
        }
        let catalog = self.catalog.catalog(profile.id).filter(|_| browses);
        let loaded = catalog.is_some();
        let read = catalog.is_some_and(|catalog| catalog.fetched_at.is_some());
        let settings = crate::model::effective_catalog(&profile, &self.shared_catalogs);
        let completion = completion::complete(
            catalog,
            &request,
            profile.default_schema(),
            &settings,
            crate::catalog::now(),
            profile.database_type,
        );
        if trigger == Trigger::Edit && completion.items.is_empty() && completion.missing.is_empty()
        {
            cx.notify();
            return;
        }
        let mut requested = previous
            .as_ref()
            .map(|popup| popup.requested.clone())
            .unwrap_or_default();
        // A request by the user can read what the list needs, like Run can
        // open a session. A read that failed waits for Retry.
        for need in &completion.missing {
            if *need == Need::Schemas
                || requested.contains(need)
                || completion.errors.iter().any(|(failed, _)| failed == need)
            {
                continue;
            }
            requested.insert(need.clone());
            self.refresh_catalog(profile.id, need_scope(need), cx);
        }
        let same_list = previous.as_ref().is_some_and(|popup| {
            popup.request.prefix == request.prefix && popup.request.place == request.place
        });
        let (selected, scroll) = match previous {
            Some(popup) if same_list => (
                popup.selected.min(completion.items.len().saturating_sub(1)),
                popup.scroll,
            ),
            Some(popup) => (0, popup.scroll),
            None => (0, UniformListScrollHandle::new()),
        };
        if !same_list {
            scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        self.completion = Some(CompletionPopup {
            tab: tab_id,
            profile: profile.id,
            request,
            completion,
            browses,
            loaded,
            read,
            selected,
            scroll,
            requested,
        });
        cx.notify();
    }

    pub(super) fn close_completion(&mut self, cx: &mut Context<Self>) {
        if self.completion.take().is_some() {
            cx.notify();
        }
    }

    /// The popup of the active tab.
    fn active_completion(&self) -> Option<&CompletionPopup> {
        let tab = self.tabs.get(self.active)?.saved.id;
        self.completion.as_ref().filter(|popup| popup.tab == tab)
    }

    fn move_completion(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(popup) = self.completion.as_mut() else {
            return false;
        };
        let count = popup.completion.items.len();
        if count == 0 {
            return false;
        }
        let selected = popup.selected as isize + delta;
        popup.selected = if delta.abs() == 1 {
            selected.rem_euclid(count as isize) as usize
        } else {
            selected.clamp(0, count as isize - 1) as usize
        };
        popup
            .scroll
            .scroll_to_item(popup.selected, ScrollStrategy::Nearest);
        cx.notify();
        true
    }

    /// Inserts the suggestion at `index` in place of the name at the cursor.
    fn accept_completion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.completion.take() else {
            return;
        };
        let Some(item) = popup.completion.items.get(index) else {
            return;
        };
        let Some(tab) = self.tabs.iter().find(|tab| tab.saved.id == popup.tab) else {
            return;
        };
        let replace = popup.request.replace.clone();
        let insert = item.insert.clone();
        tab.input.update(cx, |state, cx| {
            state.set_selected_range(replace, cx);
            state.replace(insert, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Attaches the keys of the popup to the editor container. They run
    /// before the editor, and only while the popup is open.
    pub(super) fn completion_keys(
        &self,
        element: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        element
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.active_completion().is_some() && this.move_completion(-1, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.active_completion().is_some() && this.move_completion(1, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MovePageUp, _, cx| {
                if this.active_completion().is_some()
                    && this.move_completion(-(MAX_ROWS as isize), cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MovePageDown, _, cx| {
                if this.active_completion().is_some() && this.move_completion(MAX_ROWS as isize, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, action: &Enter, window, cx| {
                if Enter::is_primary(action) {
                    this.accept_selected(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                this.accept_selected(window, cx);
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.active_completion().is_some() {
                    this.close_completion(cx);
                    cx.stop_propagation();
                }
            }))
    }

    fn accept_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self
            .active_completion()
            .filter(|popup| !popup.completion.items.is_empty())
            .map(|popup| popup.selected)
        else {
            return;
        };
        self.accept_completion(selected, window, cx);
        cx.stop_propagation();
    }

    /// The popup at the cursor of the active editor, when it is open.
    pub(super) fn completion_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popup = self.active_completion()?;
        let editor = &self.tabs[self.active].input;
        if !editor.focus_handle(cx).is_focused(window) {
            return None;
        }
        let input = editor.read(cx);
        let (cursor, line_height) = input.cursor_layout()?;
        let input_bounds = input.input_bounds();
        // The cursor of the last layout, in window coordinates.
        let caret = cursor.origin + input.scroll_offset();
        if !input_bounds.contains(&caret) {
            return None;
        }
        let text = input.value();
        let typed =
            &text[popup.request.replace.start..input.cursor().max(popup.request.replace.start)];
        let names = self.completion_font(cx);
        let typed_width = text_width(window, typed, &names, self.ui_px(13.));
        let layout = self.completion_layout(popup, window, cx);
        // Names in the list start under the name in the editor.
        let name_inset = self.ui_px(ROW_CHROME - 2. - GAP + 1.);
        let mut position = point(
            caret.x - typed_width - name_inset,
            caret.y + line_height + self.ui_px(2.),
        );
        let right = input_bounds.right() - layout.width - self.ui_px(4.);
        position.x = position.x.min(right).max(input_bounds.left());
        let details = self.completion_details(popup, cx);
        Some(
            deferred(
                anchored()
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        h_flex()
                            .items_start()
                            .gap(self.ui_px(4.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.close_completion(cx);
                            }))
                            .child(self.completion_list(popup, &layout, window, cx))
                            .children(details),
                    ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    fn completion_font(&self, _: &App) -> Font {
        Font {
            weight: FontWeight::BOLD,
            ..font(self.settings.editor_font_family.clone())
        }
    }

    /// The width of the list, and which labels show. The list grows to fit
    /// the longest name, hides the labels at the right of all rows when one
    /// name does not fit beside its label, then cuts the names.
    fn completion_layout(&self, popup: &CompletionPopup, window: &mut Window, cx: &App) -> Layout {
        let names = self.completion_font(cx);
        let labels = font(cx.theme().font_family.clone());
        let name_size = self.ui_px(13.);
        let label_size = self.ui_px(12.);
        let chrome = self.ui_px(ROW_CHROME);
        let gap = self.ui_px(GAP);
        let rows: Vec<(Pixels, Pixels)> = popup
            .completion
            .items
            .iter()
            .map(|item| {
                let name = text_width(window, &shown_name(item), &names, name_size);
                let label = text_width(window, &item.detail, &labels, label_size)
                    + item.source.as_deref().map_or(px(0.), |source| {
                        text_width(window, source, &names, self.ui_px(11.)) + self.ui_px(10.) + gap
                    });
                (name, label)
            })
            .collect();
        let widest = rows
            .iter()
            .map(|(name, label)| *name + gap + *label)
            .fold(px(0.), Pixels::max);
        let width = (widest + chrome)
            .max(self.ui_px(MIN_WIDTH))
            .min(self.ui_px(MAX_WIDTH));
        let details = rows
            .iter()
            .all(|(name, label)| *name + gap + *label + chrome <= width);
        let cut = rows
            .iter()
            .map(|(name, label)| {
                let label = if details { gap + *label } else { px(0.) };
                *name + label + chrome > width + px(0.5)
            })
            .collect();
        Layout {
            width,
            details,
            cut,
        }
    }

    fn completion_body(&self, popup: &CompletionPopup) -> Body {
        if !popup.browses {
            return Body::Off;
        }
        if !popup.completion.items.is_empty() {
            return Body::Items;
        }
        if !popup.loaded || self.catalog.reads(popup.profile, &Scope::Connection) {
            return Body::Loading(vec![Text::Plain("Loading schemas…".into())]);
        }
        if !popup.read {
            if let Some((_, error)) = popup.completion.errors.first() {
                return Body::Failed {
                    title: "Could not load schemas",
                    message: vec![Text::Plain(first_line(error))],
                };
            }
            return Body::NotRead;
        }
        let Some(need) = popup.completion.missing.first() else {
            return Body::NoSuggestions;
        };
        let reading = self.catalog.reads(popup.profile, &need_scope(need));
        let error = popup
            .completion
            .errors
            .iter()
            .find(|(failed, _)| failed == need)
            .map(|(_, error)| error);
        match (need, error) {
            (Need::Columns(schema, relation), Some(error)) if !reading => Body::Failed {
                title: "Could not load columns",
                message: code_names(
                    &first_line(error),
                    &[format!("{schema}.{relation}"), relation.clone()],
                ),
            },
            (Need::Relations(schema), Some(error)) if !reading => Body::Failed {
                title: "Could not load tables",
                message: code_names(&first_line(error), std::slice::from_ref(schema)),
            },
            (Need::Columns(schema, relation), _) => Body::Loading(vec![
                Text::Plain("Loading columns of ".into()),
                Text::Code(format!("{schema}.{relation}")),
                Text::Plain("…".into()),
            ]),
            (Need::Relations(schema), _) => Body::Loading(vec![
                Text::Plain("Loading tables of ".into()),
                Text::Code(schema.clone()),
                Text::Plain("…".into()),
            ]),
            (Need::Schemas, _) => Body::Loading(vec![Text::Plain("Loading schemas…".into())]),
        }
    }

    fn completion_list(
        &self,
        popup: &CompletionPopup,
        layout: &Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let body = self.completion_body(popup);
        let shell = v_flex()
            .id("sql-completion")
            .test_support()
            .role(Role::List)
            .aria_label("Suggestions")
            .w(layout.width)
            .p(self.ui_px(4.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_size(self.ui_px(13.))
            .font_family(theme.font_family.clone())
            // Clicks in the popup stay out of the editor.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let shell = match body {
            Body::Items => shell
                .child(self.completion_rows(popup, layout, window, cx))
                .children(self.completion_footer(popup, cx)),
            Body::Loading(message) => shell.child(
                h_flex()
                    .id("sql-completion-loading")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(plain(&message))
                    .h(self.ui_px(ROW_HEIGHT))
                    .px(self.ui_px(8.))
                    .gap(self.ui_px(GAP))
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .size(self.ui_px(14.))
                            .flex_none()
                            .child(Spinner::new().xsmall().color(theme.muted_foreground)),
                    )
                    .child(self.texts(message, cx)),
            ),
            Body::Failed { title, message } => shell.child(self.completion_notice(
                title,
                Some(message),
                Some(("sql-completion-retry", "Retry")),
                true,
                cx,
            )),
            Body::NotRead => shell.child(self.completion_notice(
                "No cached tables",
                Some(vec![Text::Plain(
                    "Refresh the schemas of this connection to get suggestions.".into(),
                )]),
                Some(("sql-completion-refresh", "Refresh")),
                false,
                cx,
            )),
            Body::Off => shell.child(self.completion_notice(
                "No cached tables",
                Some(vec![Text::Plain(
                    "Turn on metadata refresh in the connection settings.".into(),
                )]),
                None,
                false,
                cx,
            )),
            Body::NoSuggestions => shell.child(
                div()
                    .id("sql-completion-empty")
                    .test_support()
                    .role(Role::Status)
                    .aria_label("No suggestions")
                    .h(self.ui_px(ROW_HEIGHT))
                    .px(self.ui_px(8.))
                    .flex()
                    .items_center()
                    .text_color(theme.muted_foreground)
                    .child("No suggestions"),
            ),
        };
        shell.into_any_element()
    }

    /// A title, one line, and an optional button: the states without a list.
    fn completion_notice(
        &self,
        title: &'static str,
        message: Option<Vec<Text>>,
        action: Option<(&'static str, &'static str)>,
        warning: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let profile = self.active_completion().map(|popup| popup.profile);
        v_flex()
            .id("sql-completion-notice")
            .test_support()
            .role(Role::Status)
            .aria_label(title)
            .p(self.ui_px(8.))
            .gap(self.ui_px(6.))
            .child(
                h_flex()
                    .gap(self.ui_px(GAP))
                    .font_weight(FontWeight::MEDIUM)
                    .when(warning, |row| {
                        row.child(
                            Icon::new(IconName::CircleAlert)
                                .size(self.ui_px(14.))
                                .text_color(theme.warning),
                        )
                    })
                    .child(title),
            )
            .when_some(message, |notice, message| {
                notice.child(
                    div()
                        .text_size(self.ui_px(12.))
                        .text_color(theme.muted_foreground)
                        .child(self.texts(message, cx)),
                )
            })
            .when_some(action.zip(profile), |notice, ((id, label), _)| {
                notice.child(
                    h_flex().child(
                        Button::new(id)
                            .outline()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(label)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_completions(&ShowCompletions, window, cx);
                            })),
                    ),
                )
            })
    }

    /// Text with names in the code font.
    fn texts(&self, parts: Vec<Text>, cx: &App) -> Div {
        let theme = cx.theme();
        let code_font = self.settings.editor_font_family.clone();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .children(parts.into_iter().map(|part| {
                match part {
                    Text::Plain(text) => div().child(text),
                    Text::Code(text) => div()
                        .font_family(code_font.clone())
                        .px(self.ui_px(4.))
                        .rounded(self.ui_px(4.))
                        .bg(theme.muted)
                        .text_color(theme.foreground)
                        .child(text),
                }
            }))
    }

    fn completion_rows(
        &self,
        popup: &CompletionPopup,
        layout: &Layout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let count = popup.completion.items.len();
        let row_height = self.ui_px(ROW_HEIGHT);
        let items: Vec<Item> = popup.completion.items.clone();
        let selected = popup.selected;
        let cut = layout.cut.clone();
        let details = layout.details;
        let qrow = cx.entity().downgrade();
        let names_font = SharedString::from(self.settings.editor_font_family.clone());
        let scale = self.settings.ui_scale;
        uniform_list("sql-completion-rows", count, move |range, _, cx| {
            range
                .map(|index| {
                    completion_row(
                        &items[index],
                        index,
                        index == selected,
                        cut[index],
                        details,
                        &names_font,
                        scale,
                        qrow.clone(),
                        cx,
                    )
                })
                .collect()
        })
        .track_scroll(&popup.scroll)
        .h(row_height * count.min(MAX_ROWS) as f32)
    }

    /// The stale footer: the schemas of the list that are older than the
    /// refresh period, and Refresh.
    fn completion_footer(
        &self,
        popup: &CompletionPopup,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme();
        let completion = &popup.completion;
        let reads = |need: &Need| self.catalog.reads(popup.profile, &need_scope(need));
        // A read that failed comes first, then a read in progress, then
        // stale schemas.
        let failed = completion
            .errors
            .iter()
            .find(|(need, _)| !reads(need))
            .cloned();
        let loading = completion
            .missing
            .iter()
            .find(|need| {
                reads(need) || (popup.requested.contains(*need) && !failed_need(completion, need))
            })
            .cloned();
        let stale = &completion.stale;
        let refreshing = stale.iter().any(|schema| {
            self.catalog
                .reads(popup.profile, &Scope::Schema(schema.clone()))
        });
        let (slot, message, action): (Slot, Vec<Text>, Option<(&'static str, String)>) =
            if let Some((need, error)) = failed {
                (
                    Slot::Alert,
                    need_text("Could not load", &need, ""),
                    Some(("Retry", first_line(&error))),
                )
            } else if let Some(need) = loading {
                (Slot::Spinner, need_text("Loading", &need, "…"), None)
            } else if refreshing {
                let message = match stale.as_slice() {
                    [schema] => vec![
                        Text::Plain("Refreshing ".into()),
                        Text::Code(schema.clone()),
                        Text::Plain("…".into()),
                    ],
                    schemas => vec![Text::Plain(format!(
                        "Refreshing {} schemas…",
                        schemas.len()
                    ))],
                };
                (Slot::Spinner, message, None)
            } else if !stale.is_empty() {
                let message = match stale.as_slice() {
                    [schema] => vec![Text::Code(schema.clone()), Text::Plain(" is stale".into())],
                    schemas => vec![Text::Plain(format!("{} schemas are stale", schemas.len()))],
                };
                (Slot::Dot, message, Some(("Refresh", stale_tooltip(stale))))
            } else {
                return None;
            };
        let label = plain(&message);
        Some(
            h_flex()
                .id("sql-completion-footer")
                .test_support()
                .role(Role::Status)
                .aria_label(label)
                .mt(self.ui_px(4.))
                .mx(self.ui_px(-4.))
                .mb(self.ui_px(-4.))
                .px(self.ui_px(12.))
                .h(self.ui_px(ROW_HEIGHT))
                .gap(self.ui_px(GAP))
                .border_t_1()
                .border_color(theme.border)
                .text_size(self.ui_px(12.))
                .text_color(theme.muted_foreground)
                .child(
                    div()
                        .size(self.ui_px(14.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .map(|cell| match slot {
                            Slot::Spinner => {
                                cell.child(Spinner::new().xsmall().color(theme.muted_foreground))
                            }
                            Slot::Dot => cell
                                .child(div().size(self.ui_px(6.)).rounded_full().bg(theme.warning)),
                            Slot::Alert => cell.child(
                                Icon::new(IconName::CircleAlert)
                                    .size(self.ui_px(14.))
                                    .text_color(theme.warning),
                            ),
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(self.texts(message, cx).flex_nowrap()),
                )
                .when_some(action, |footer, (label, tooltip)| {
                    footer.child(
                        Button::new("sql-completion-footer-action")
                            .link()
                            .xsmall()
                            .label(label)
                            .tooltip(tooltip)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_completions(&ShowCompletions, window, cx);
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// The box beside the list with the full name, type, relation, and
    /// comment of the selected column, or the comment of a table.
    fn completion_details(&self, popup: &CompletionPopup, cx: &App) -> Option<AnyElement> {
        let item = popup.completion.items.get(popup.selected)?;
        if item.kind != ItemKind::Column && item.comment.is_none() {
            return None;
        }
        let theme = cx.theme();
        let code = self.settings.editor_font_family.clone();
        Some(
            v_flex()
                .id("sql-completion-details")
                .test_support()
                .role(Role::Note)
                .aria_label(details_label(item))
                .w(self.ui_px(DETAILS_WIDTH))
                .px(self.ui_px(12.))
                .py(self.ui_px(10.))
                .gap(self.ui_px(6.))
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_lg)
                .shadow_lg()
                .text_size(self.ui_px(12.))
                .text_color(theme.muted_foreground)
                .child(
                    h_flex()
                        .items_start()
                        .justify_between()
                        .gap(self.ui_px(GAP))
                        .child(
                            div()
                                .min_w_0()
                                .font_family(code.clone())
                                .text_size(self.ui_px(13.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.popover_foreground)
                                .child(item.name.clone()),
                        )
                        .child(div().flex_none().child(item.detail.clone())),
                )
                .when_some(item.relation.as_ref(), |details, (schema, relation)| {
                    details.when(item.kind == ItemKind::Column, |details| {
                        details.child(
                            div()
                                .font_family(code.clone())
                                .child(format!("{schema}.{relation}")),
                        )
                    })
                })
                .when_some(item.comment.clone(), |details, comment| {
                    details.child(
                        div()
                            .text_size(self.ui_px(13.))
                            .text_color(theme.popover_foreground)
                            .child(comment),
                    )
                })
                .into_any_element(),
        )
    }
}

/// The details of `item` in one line, for assistive technologies.
fn details_label(item: &Item) -> String {
    let mut parts = vec![item.name.clone(), item.detail.clone()];
    if item.kind == ItemKind::Column
        && let Some((schema, relation)) = &item.relation
    {
        parts.push(format!("{schema}.{relation}"));
    }
    parts.extend(item.comment.clone());
    parts.join(", ")
}

/// One row of the list.
#[allow(clippy::too_many_arguments)]
fn completion_row(
    item: &Item,
    index: usize,
    selected: bool,
    cut: bool,
    details: bool,
    names_font: &SharedString,
    scale: f32,
    qrow: WeakEntity<Qrow>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let ui = |value: f32| px(scale * value);
    let icon = match item.kind {
        ItemKind::Schema => IconName::Database,
        ItemKind::Table => IconName::Table,
        ItemKind::View => IconName::Eye,
        ItemKind::Column => IconName::Minus,
    };
    let shown = shown_name(item);
    let qualifier_len = item
        .qualifier
        .as_ref()
        .map_or(0, |qualifier| qualifier.len() + 1);
    let mut highlights = Vec::new();
    if let Some(qualifier) = &item.qualifier {
        highlights.push((
            0..qualifier.len() + 1,
            HighlightStyle {
                color: Some(theme.muted_foreground),
                ..Default::default()
            },
        ));
    }
    for offset in &item.matches {
        let start = qualifier_len + offset;
        let end = start
            + item.name[*offset..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
        highlights.push((
            start..end,
            HighlightStyle {
                color: Some(theme.blue),
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
        ));
    }
    let id = SharedString::from(format!("sql-completion-item-{shown}"));
    let full = shown.clone();
    h_flex()
        .id(ElementId::Name(id))
        .test_support()
        .role(Role::ListItem)
        .aria_label(shown.clone())
        .aria_selected(selected)
        .h(ui(ROW_HEIGHT))
        .px(ui(8.))
        .gap(ui(GAP))
        .rounded(theme.radius)
        .cursor_pointer()
        .when(selected, |row| {
            row.bg(theme.accent).text_color(theme.accent_foreground)
        })
        .when(!selected, |row| row.hover(|row| row.bg(theme.muted)))
        .child(
            Icon::new(icon)
                .size(ui(14.))
                .flex_none()
                .text_color(theme.muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(names_font.clone())
                .child(StyledText::new(shown).with_highlights(highlights)),
        )
        .when(details, |row| {
            row.child(
                h_flex()
                    .flex_none()
                    .gap(ui(6.))
                    .text_size(ui(12.))
                    .text_color(theme.muted_foreground)
                    .when_some(item.source.clone(), |labels, source| {
                        labels.child(
                            div()
                                .px(ui(5.))
                                .rounded(ui(4.))
                                .bg(theme.muted)
                                .font_family(names_font.clone())
                                .text_size(ui(11.))
                                .text_color(theme.foreground)
                                .child(source),
                        )
                    })
                    .child(item.detail.clone()),
            )
        })
        .when(cut, |row| {
            row.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
            })
        })
        .on_click(move |_, window, cx| {
            let _ = qrow.update(cx, |this, cx| this.accept_completion(index, window, cx));
        })
        .into_any_element()
}

/// The name as the list shows it: with its qualifier, when it has one.
fn shown_name(item: &Item) -> String {
    match &item.qualifier {
        Some(qualifier) => format!("{qualifier}.{}", item.name),
        None => item.name.clone(),
    }
}

/// What the cell before the footer text shows.
#[derive(Clone, Copy)]
enum Slot {
    Spinner,
    Dot,
    Alert,
}

fn failed_need(completion: &Completion, need: &Need) -> bool {
    completion.errors.iter().any(|(failed, _)| failed == need)
}

/// "`verb` columns of `schema.table``end`", for the footer.
fn need_text(verb: &str, need: &Need, end: &str) -> Vec<Text> {
    let (what, name) = match need {
        Need::Schemas => return vec![Text::Plain(format!("{verb} schemas{end}"))],
        Need::Relations(schema) => ("tables of", schema.clone()),
        Need::Columns(schema, relation) => ("columns of", format!("{schema}.{relation}")),
    };
    let mut parts = vec![Text::Plain(format!("{verb} {what} ")), Text::Code(name)];
    if !end.is_empty() {
        parts.push(Text::Plain(end.into()));
    }
    parts
}

/// The read that gives `need`.
fn need_scope(need: &Need) -> Scope {
    match need {
        Need::Schemas => Scope::Connection,
        Need::Relations(schema) => Scope::Schema(schema.clone()),
        Need::Columns(schema, relation) => Scope::Relation(schema.clone(), relation.clone()),
    }
}

fn text_width(window: &mut Window, text: &str, font: &Font, size: Pixels) -> Pixels {
    if text.is_empty() {
        return px(0.);
    }
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::from(text.to_owned()), size, &[run], None)
        .width
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_owned()
}

/// `message` with the first of `names` that it contains in the code font.
fn code_names(message: &str, names: &[String]) -> Vec<Text> {
    for name in names {
        if let Some(start) = message.find(name.as_str()) {
            let end = start + name.len();
            return [
                Text::Plain(message[..start].to_owned()),
                Text::Code(name.clone()),
                Text::Plain(message[end..].to_owned()),
            ]
            .into_iter()
            .filter(|part| !matches!(part, Text::Plain(text) if text.is_empty()))
            .collect();
        }
    }
    vec![Text::Plain(message.to_owned())]
}

fn plain(parts: &[Text]) -> String {
    parts
        .iter()
        .map(|part| match part {
            Text::Plain(text) | Text::Code(text) => text.as_str(),
        })
        .collect()
}

fn stale_tooltip(schemas: &[String]) -> String {
    match schemas {
        [schema] => format!("Refresh {schema}"),
        schemas => format!("Refresh {}", schemas.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn an_error_shows_the_relation_name_in_code_font() {
        let parts = code_names(
            "Table or view not found: avia.flight_events",
            &["avia.flight_events".into(), "flight_events".into()],
        );
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], Text::Plain(text) if text == "Table or view not found: "));
        assert!(matches!(&parts[1], Text::Code(text) if text == "avia.flight_events"));
        let parts = code_names("Timed out", &["avia.x".into()]);
        assert!(matches!(&parts[..], [Text::Plain(text)] if text == "Timed out"));
    }
}
