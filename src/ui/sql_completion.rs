//! The autocomplete popup of the SQL editor. Tab after a name opens it with the schemas, tables, views, and columns that fit the
//! cursor, from the cached metadata of the tab's connection. [`crate::completion`] finds the
//! suggestions; this module owns the popup, its keys, and the reads that it
//! asks the catalog worker for.

use super::Qrow;
use super::dbt::{DbtLookup, summary};
use crate::catalog::Scope;
use crate::completion::{self, Completion, Item, ItemKind, Need, Request};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, MoveDown, MovePageDown, MovePageUp, MoveUp,
};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewState, TextViewStyle};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::HashSet;
use uuid::Uuid;

/// The most rows that the list shows without scrolling.
const MAX_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 26.;
const MIN_WIDTH: f32 = 320.;
const MAX_WIDTH: f32 = 440.;
const DETAILS_WIDTH: f32 = 260.;
/// The longest description in the details box, in characters: about four
/// lines. The box shows the start of the first paragraph.
const DESCRIPTION_CHARS: usize = 140;
/// The tallest description in the details box, in pixels, for Markdown with
/// content of any height, like an image.
const DESCRIPTION_HEIGHT: f32 = 120.;
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
    /// The parsed Markdown of the description in the details box.
    description: RefCell<Option<Description>>,
}

/// The Markdown state of a description, kept while its text stays the same.
struct Description {
    text: String,
    state: Entity<TextViewState>,
    _parsed: Subscription,
}

impl CompletionPopup {
    /// The query tab of the popup.
    pub(super) fn tab(&self) -> Uuid {
        self.tab
    }
}

/// A state of the popup without a list.
struct Notice {
    title: &'static str,
    message: Vec<Text>,
    action: Option<(&'static str, &'static str)>,
    /// Whether the title has the warning icon.
    warning: bool,
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
    /// Refresh or Retry of the open popup, or the popup when it is closed.
    fn refresh_completion(&mut self, cx: &mut Context<Self>) {
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
        let description = previous.as_ref().and_then(|popup| popup.description.take());
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
            description: RefCell::new(description),
        });
        cx.notify();
    }

    /// Loads the metadata cache of the tab's connection, so that the first
    /// Tab finds names at once. Reads only the file on this computer.
    pub(super) fn preload_completion(&mut self, tab: Uuid) {
        let profile = self
            .tabs
            .iter()
            .find(|candidate| candidate.saved.id == tab)
            .and_then(|tab| tab.saved.profile)
            .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
            .filter(|profile| profile.catalog.browses())
            .map(|profile| profile.id);
        if let Some(profile) = profile
            && self.catalog.catalog(profile).is_none()
        {
            self.ensure_catalog(profile);
        }
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
                this.tab_completion(window, cx);
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.active_completion().is_some() {
                    this.close_completion(cx);
                    cx.stop_propagation();
                }
            }))
    }

    /// Tab: inserts the selected name, runs Refresh or Retry of an open
    /// list without names, or opens the list after a name. Elsewhere, and
    /// when no name fits, Tab indents as before.
    fn tab_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.editor_autocomplete {
            return;
        }
        if let Some(popup) = self.active_completion() {
            if !popup.completion.items.is_empty() {
                self.accept_selected(window, cx);
            } else if !self.completion_retry_scopes(popup).is_empty() {
                self.refresh_completion(cx);
                cx.stop_propagation();
            }
            return;
        }
        let tab = &self.tabs[self.active];
        let input = tab.input.read(cx);
        if !input.selected_range().is_empty() {
            return;
        }
        let Some(profile) = tab
            .saved
            .profile
            .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
        else {
            return;
        };
        let after_name = completion::request(&input.value(), input.cursor(), profile.database_type)
            .is_some_and(|request| {
                !request.prefix.is_empty()
                    || matches!(
                        request.place,
                        completion::Place::Qualified { .. } | completion::Place::Relation
                    )
            });
        if !after_name || !profile.catalog.browses() {
            return;
        }
        self.update_completion(Trigger::Open, cx);
        // Tab opens only a list with names, or with a read for them,
        // including the first load of the cache and the Refresh of a
        // connection that Qrow never read.
        let useful = self.active_completion().is_some_and(|popup| {
            !popup.completion.items.is_empty()
                || !popup.loaded
                || !popup.read
                || (popup.read && !popup.completion.missing.is_empty())
        });
        if useful {
            cx.stop_propagation();
        } else {
            self.close_completion(cx);
        }
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
        // The border, the padding of the popup and of a row, the icon, and
        // the gap before the name.
        let name_inset = self.ui_px(1. + 4. + 8. + 14. + GAP);
        let mut position = point(
            caret.x - typed_width - name_inset,
            caret.y + line_height + self.ui_px(2.),
        );
        let margin = px(8.);
        let window_right = window.viewport_size().width - margin;
        let right = input_bounds.right().min(window_right) - layout.width;
        position.x = position.x.min(right).max(input_bounds.left());
        let details = self.completion_details(popup, &layout, window, cx);
        // The box goes below the list when it does not fit at its right,
        // so the list does not move away from the typed name.
        let beside = position.x + layout.width + self.ui_px(4. + DETAILS_WIDTH) <= window_right;
        Some(
            deferred(
                anchored()
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .flex()
                            .when(beside, |group| group.flex_row())
                            .when(!beside, |group| group.flex_col())
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
                .children(self.completion_footer(popup, layout, window, cx)),
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
                    .child({
                        // The border and the padding of the popup and of
                        // the row, the spinner, and its gap.
                        let room = layout.width - px(2.) - self.ui_px(8. + 16. + 14. + GAP);
                        self.text_line("sql-completion-loading-text", message, room, window, cx)
                    }),
            ),
            Body::Failed { title, message } => shell.child(self.completion_notice(
                Notice {
                    title,
                    message,
                    action: Some(("sql-completion-retry", "Retry")),
                    warning: true,
                },
                layout,
                window,
                cx,
            )),
            Body::NotRead => shell.child(self.completion_notice(
                Notice {
                    title: "No cached metadata",
                    message: vec![Text::Plain(
                        "Refresh the schemas of this connection to get suggestions.".into(),
                    )],
                    action: Some(("sql-completion-refresh", "Refresh")),
                    warning: false,
                },
                layout,
                window,
                cx,
            )),
            Body::Off => shell.child(self.completion_notice(
                Notice {
                    title: "No cached metadata",
                    message: vec![Text::Plain(
                        "Turn on metadata refresh in the connection settings.".into(),
                    )],
                    action: None,
                    warning: false,
                },
                layout,
                window,
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

    /// A title, a message, and an optional button: the states without a
    /// list.
    fn completion_notice(
        &self,
        notice: Notice,
        layout: &Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let Notice {
            title,
            message,
            action,
            warning,
        } = notice;
        // The border and the padding of the popup and of the notice.
        let room = layout.width - px(2.) - self.ui_px(8. + 16.);
        let message = self.texts(message, room, window, cx);
        let theme = cx.theme();
        let profile = self.active_completion().map(|popup| popup.profile);
        v_flex()
            .id("sql-completion-notice")
            .test_support()
            .role(Role::Status)
            .aria_label(title)
            .p(self.ui_px(8.))
            // One line height for the title and the message, like toasts,
            // so that the space above and below the message is the same.
            .line_height(rems(1.25))
            .child(
                h_flex()
                    .id("sql-completion-title")
                    .test_support()
                    .gap(self.ui_px(GAP))
                    .font_weight(FontWeight::MEDIUM)
                    .when(warning, |row| {
                        row.child(
                            Icon::new(IconName::TriangleAlert)
                                .size(self.ui_px(14.))
                                .text_color(theme.warning),
                        )
                    })
                    .child(title),
            )
            .child(
                div()
                    .id("sql-completion-message")
                    .test_support()
                    .text_size(self.ui_px(12.))
                    .text_color(theme.muted_foreground)
                    .child(message),
            )
            .when_some(action.zip(profile), |notice, ((id, label), _)| {
                notice.child(
                    h_flex().mt(self.ui_px(8.)).child(
                        Button::new(id).small().label(label).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.refresh_completion(cx);
                            },
                        )),
                    ),
                )
            })
    }

    /// Text with names in the code font, at 12 px, in lines of `room`.
    /// Plain text wraps between its words. A name wider than `room` is cut
    /// at the end and shows its full text in a tooltip.
    fn texts(&self, parts: Vec<Text>, room: Pixels, window: &mut Window, cx: &App) -> Div {
        let code_font = font(self.settings.editor_font_family.clone());
        let mut items: Vec<AnyElement> = Vec::new();
        for (index, part) in parts.into_iter().enumerate() {
            match part {
                // Each word with its space is an item of the row, so the
                // row wraps between words.
                Text::Plain(text) => items.extend(text.split_inclusive(' ').map(|word| {
                    div()
                        .whitespace_nowrap()
                        .child(word.to_owned())
                        .into_any_element()
                })),
                Text::Code(text) => {
                    let cut = self.code_width(window, &text, &code_font) > room + px(0.5);
                    items.push(
                        self.code_chip(text.clone(), cx)
                            .id(("sql-completion-name", index))
                            .max_w_full()
                            .truncate()
                            .when(cut, |chip| {
                                chip.tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(text.clone())
                                        .build(window, cx)
                                })
                            })
                            .into_any_element(),
                    );
                }
            }
        }
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .children(items)
    }

    /// Text with names in the code font, at 12 px, on one line of `room`.
    /// Text wider than `room` is cut and shows its full text in a tooltip.
    fn text_line(
        &self,
        id: &'static str,
        parts: Vec<Text>,
        room: Pixels,
        window: &mut Window,
        cx: &App,
    ) -> AnyElement {
        let ui_font = font(cx.theme().font_family.clone());
        let code_font = font(self.settings.editor_font_family.clone());
        let width = parts.iter().fold(px(0.), |width, part| {
            width
                + match part {
                    Text::Plain(text) => text_width(window, text, &ui_font, self.ui_px(12.)),
                    Text::Code(text) => self.code_width(window, text, &code_font),
                }
        });
        let full = plain(&parts);
        let cut = width > room + px(0.5);
        div()
            .id(id)
            .test_support()
            // The size that `width` measures, whatever the parent sets.
            .text_size(self.ui_px(12.))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .flex()
            .items_center()
            .children(parts.into_iter().map(|part| match part {
                Text::Plain(text) => div().flex_none().child(text),
                Text::Code(text) => self.code_chip(text, cx).min_w_0().truncate(),
            }))
            .when(cut, |line| {
                line.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                })
            })
            .into_any_element()
    }

    /// A name in the code font, on a muted background.
    fn code_chip(&self, text: String, cx: &App) -> Div {
        let theme = cx.theme();
        div()
            .font_family(self.settings.editor_font_family.clone())
            .px(self.ui_px(4.))
            .rounded(self.ui_px(4.))
            .bg(theme.muted)
            .text_color(theme.foreground)
            .child(text)
    }

    /// The width of a name chip: the name and its padding.
    fn code_width(&self, window: &mut Window, text: &str, code_font: &Font) -> Pixels {
        text_width(window, text, code_font, self.ui_px(12.)) + self.ui_px(8.)
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
        .w_full()
        .h(row_height * count.min(MAX_ROWS) as f32)
    }

    /// The stale footer: the schemas of the list that are older than the
    /// refresh period, and Refresh.
    fn completion_footer(
        &self,
        popup: &CompletionPopup,
        layout: &Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
        // The footer spans the popup inside its border. Its padding, the
        // slot and its gap, and the action with its gap leave the room of
        // the message.
        let action_width = action.as_ref().map_or(px(0.), |(label, _)| {
            let ui_font = font(cx.theme().font_family.clone());
            text_width(window, label, &ui_font, window.rem_size() * 0.75) + self.ui_px(GAP)
        });
        let room = layout.width - px(2.) - self.ui_px(24. + 14. + GAP) - action_width;
        let line = self.text_line("sql-completion-footer-text", message, room, window, cx);
        let theme = cx.theme();
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
                                Icon::new(IconName::TriangleAlert)
                                    .size(self.ui_px(14.))
                                    .text_color(theme.warning),
                            ),
                        }),
                )
                .child(line)
                .when_some(action, |footer, (label, tooltip)| {
                    footer.child(
                        Button::new("sql-completion-footer-action")
                            .link()
                            .xsmall()
                            .label(label)
                            .tooltip(tooltip)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_completion(cx);
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// The box beside the list with what the selected row does not show:
    /// the description, and the name or the type when the row cuts or
    /// hides them. Without such text there is no box.
    fn completion_details(
        &self,
        popup: &CompletionPopup,
        layout: &Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let item = popup.completion.items.get(popup.selected)?;
        let dbt = self.dbt_lookups().remove(&popup.profile);
        let details = DetailsBox::of(item, layout, popup.selected, dbt.as_deref())?;
        let description = details
            .description
            .as_deref()
            .map(|text| description_state(popup, text, cx));
        let theme = cx.theme();
        let code = self.settings.editor_font_family.clone();
        // The text width inside the box: its width less padding and border.
        let inner = self.ui_px(DETAILS_WIDTH - 2. * 12. - 2.);
        let name_lines = details.name.as_deref().map(|name| {
            let font = Font {
                weight: FontWeight::BOLD,
                ..font(code.clone())
            };
            part_lines(name, inner, |text| {
                text_width(window, text, &font, self.ui_px(13.))
            })
        });
        let detail_lines = details.detail.as_deref().map(|detail| {
            let font = font(theme.font_family.clone());
            part_lines(detail, inner, |text| {
                text_width(window, text, &font, self.ui_px(12.))
            })
        });
        Some(
            v_flex()
                .id("sql-completion-details")
                .test_support()
                .role(Role::Note)
                .aria_label(details.label())
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
                .children(name_lines.map(|lines| {
                    v_flex()
                        .w_full()
                        .font_family(code.clone())
                        .text_size(self.ui_px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.popover_foreground)
                        .children(lines.into_iter().map(|line| div().child(line)))
                }))
                .children(detail_lines.map(|lines| {
                    v_flex()
                        .w_full()
                        .children(lines.into_iter().map(|line| div().child(line)))
                }))
                .children(description.map(|state| {
                    div()
                        .id("sql-completion-description")
                        .test_support()
                        .w_full()
                        .max_h(self.ui_px(DESCRIPTION_HEIGHT))
                        .overflow_hidden()
                        .text_size(self.ui_px(13.))
                        .text_color(theme.popover_foreground)
                        .child(
                            TextView::new(&state)
                                .style(TextViewStyle::default().paragraph_gap(rems(0.)))
                                .selectable(false),
                        )
                }))
                .into_any_element(),
        )
    }
}

/// `text` in lines that fit `width`. A line ends only after `_`, `.`, `,`,
/// `<`, or `:`, so long names and types break between their parts. A part
/// wider than `width` breaks between its characters.
fn part_lines(text: &str, width: Pixels, mut measure: impl FnMut(&str) -> Pixels) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, c) in text.char_indices() {
        if matches!(c, '_' | '.' | ',' | '<' | ':') {
            let end = index + c.len_utf8();
            parts.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for part in parts {
        let joined = format!("{line}{part}");
        if line.is_empty() || measure(&joined) <= width {
            line = joined;
        } else {
            lines.push(std::mem::take(&mut line));
            line = part.to_owned();
        }
        // A part that alone is wider than the box breaks between characters.
        while measure(&line) > width && line.chars().count() > 1 {
            let mut cut = line.len();
            while cut > 0 && measure(&line[..cut]) > width {
                cut = line[..cut].char_indices().last().map_or(0, |(i, _)| i);
            }
            let cut = cut.max(line.chars().next().map_or(1, char::len_utf8));
            lines.push(line[..cut].to_owned());
            line = line[cut..].to_owned();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The Markdown state of `text`, the description of the selected row. The
/// popup keeps it while the text stays the same, so the text parses once.
fn description_state(
    popup: &CompletionPopup,
    text: &str,
    cx: &mut Context<Qrow>,
) -> Entity<TextViewState> {
    let mut description = popup.description.borrow_mut();
    if let Some(current) = description.as_ref()
        && current.text == text
    {
        return current.state.clone();
    }
    let state = cx.new(|cx| TextViewState::markdown(text, cx));
    // The parse ends after the first frame.
    let parsed = cx.observe(&state, |_, _, cx| cx.notify());
    *description = Some(Description {
        text: text.to_owned(),
        state: state.clone(),
        _parsed: parsed,
    });
    state
}

/// What the details box adds to the selected row.
struct DetailsBox {
    /// The full name, when the row cuts it.
    name: Option<String>,
    /// The kind or the type, when the rows hide them.
    detail: Option<String>,
    /// The start of the dbt description, or of the comment when there is no
    /// dbt description, as Markdown. A dbt description is usually the more
    /// complete text.
    description: Option<String>,
}

impl DetailsBox {
    fn of(item: &Item, layout: &Layout, index: usize, dbt: Option<&DbtLookup>) -> Option<Self> {
        let dbt_description = item.relation.as_ref().and_then(|(schema, relation)| {
            let column = (item.kind == ItemKind::Column).then_some(item.name.as_str());
            dbt?.description(schema, relation, column)
        });
        let description = dbt_description.or_else(|| {
            item.comment
                .as_deref()
                .map(str::trim)
                .filter(|comment| !comment.is_empty())
        });
        let details = Self {
            name: layout
                .cut
                .get(index)
                .copied()
                .unwrap_or(false)
                .then(|| shown_name(item)),
            detail: (!layout.details).then(|| item.detail.clone()),
            description: description.map(|text| summary(text, DESCRIPTION_CHARS).0),
        };
        (details.name.is_some() || details.detail.is_some() || details.description.is_some())
            .then_some(details)
    }

    /// The text of the box in one line, for assistive technologies.
    fn label(&self) -> String {
        [&self.name, &self.detail, &self.description]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }
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
        .w_full()
        .h(ui(ROW_HEIGHT))
        .px(ui(8.))
        .gap(ui(GAP))
        .rounded(theme.radius)
        .cursor_pointer()
        .when(selected, |row| row.bg(theme.tokens.list_active))
        .when(!selected, |row| {
            row.hover(|row| row.bg(theme.tokens.list_hover))
        })
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
        // The details box shows the full name of the selected row.
        .when(cut && !selected, |row| {
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

/// "`verb` columns of `table``end`", for the footer.
fn need_text(verb: &str, need: &Need, end: &str) -> Vec<Text> {
    let (what, name) = match need {
        Need::Schemas => return vec![Text::Plain(format!("{verb} schemas{end}"))],
        Need::Relations(schema) => ("tables of", schema.clone()),
        // The table alone: the statement names its schema, and a long
        // schema name would push the table out of the footer.
        Need::Columns(_, relation) => ("columns of", relation.clone()),
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
    fn long_names_and_types_wrap_between_their_parts() {
        // One pixel for each character.
        let measure = |text: &str| px(text.chars().count() as f32);
        assert_eq!(
            part_lines("map<string,struct<a:timestamp>>", px(12.), measure),
            ["map<string,", "struct<a:", "timestamp>>"]
        );
        assert_eq!(part_lines("short", px(12.), measure), ["short"]);
        // A part wider than the box breaks between its characters.
        assert_eq!(
            part_lines("abcdefghij_k", px(4.), measure),
            ["abcd", "efgh", "ij_k"]
        );
    }

    #[::core::prelude::v1::test]
    fn the_details_box_shows_the_start_of_a_long_description() {
        let layout = Layout {
            width: px(320.),
            details: true,
            cut: vec![false],
        };
        let item = |comment: &str| Item {
            kind: ItemKind::Column,
            name: "account_id".into(),
            qualifier: None,
            insert: "account_id".into(),
            matches: Vec::new(),
            source: None,
            detail: "string".into(),
            relation: Some(("billing".into(), "accounts".into())),
            comment: Some(comment.into()),
        };
        let long = format!(
            "The account that pays, from `billing.accounts`. {}\n\nOne row for each account.",
            "It has the plan and the invoices of the account. ".repeat(5)
        );
        let details = DetailsBox::of(&item(&long), &layout, 0, None).unwrap();
        let description = details.description.unwrap();
        assert!(
            description.chars().count() <= DESCRIPTION_CHARS + 1,
            "{description}"
        );
        assert!(description.starts_with("The account that pays, from `billing.accounts`. It has"));
        assert!(description.ends_with('…'), "{description}");
        // A short description shows in full, and a blank one shows nothing.
        let short = "The account ID that the billing system uses. Join to billing.accounts on account_id to get the plan and the invoices of the account.";
        let details = DetailsBox::of(&item(short), &layout, 0, None).unwrap();
        assert_eq!(details.description.as_deref(), Some(short));
        assert!(DetailsBox::of(&item("  "), &layout, 0, None).is_none());
    }

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
