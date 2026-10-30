use std::rc::Rc;

use crate::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui::{
    Action, AnyElement, App, AppContext, Context, DismissEvent, Empty, Entity, EventEmitter,
    Half as _, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement, Pixels, Point,
    Render, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled, StyledText,
    Subscription, WeakEntity, Window, deferred, div, prelude::FluentBuilder, px, relative,
};
use gpui_base::{TestSupportExt as _, input::CompletionNote};
use lsp_types::CompletionItem;

const MAX_MENU_HEIGHT: Pixels = px(240.);
const POPOVER_GAP: Pixels = px(4.);

use crate::{
    ActiveTheme, Disableable as _, IndexPath, Selectable, actions, h_flex,
    input::{
        self, EditorState,
        popovers::{editor_popover, render_markdown},
    },
    label::Label,
    list::{List, ListDelegate, ListEvent, ListState},
};

struct ContextMenuDelegate {
    query: SharedString,
    menu: WeakEntity<CompletionMenu>,
    items: Vec<Rc<CompletionItem>>,
    selected_ix: usize,
}

impl ContextMenuDelegate {
    fn set_items(&mut self, items: Vec<CompletionItem>) {
        let selected = self.selected_item().cloned();
        self.items = items.into_iter().map(Rc::new).collect();
        self.selected_ix = selected
            .and_then(|old| {
                self.items.iter().position(|item| {
                    item.label == old.label
                        && item.text_edit == old.text_edit
                        && item.insert_text == old.insert_text
                })
            })
            .unwrap_or(0);
    }

    fn selected_item(&self) -> Option<&Rc<CompletionItem>> {
        self.items.get(self.selected_ix)
    }
}

#[derive(IntoElement)]
struct CompletionMenuItem {
    ix: usize,
    item: Rc<CompletionItem>,
    children: Vec<AnyElement>,
    selected: bool,
    highlight_prefix: SharedString,
}

impl CompletionMenuItem {
    fn new(ix: usize, item: Rc<CompletionItem>) -> Self {
        Self {
            ix,
            item,
            children: vec![],
            selected: false,
            highlight_prefix: "".into(),
        }
    }

    fn highlight_prefix(mut self, s: impl Into<SharedString>) -> Self {
        self.highlight_prefix = s.into();
        self
    }
}
impl Selectable for CompletionMenuItem {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for CompletionMenuItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}
impl RenderOnce for CompletionMenuItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let item = self.item;

        let deprecated = item.deprecated.unwrap_or(false);
        let mut query = item
            .filter_text
            .as_deref()
            .unwrap_or(&self.highlight_prefix)
            .chars()
            .flat_map(char::to_lowercase)
            .peekable();
        let mut ranges = Vec::new();
        for (offset, ch) in item.label.char_indices() {
            if query
                .peek()
                .is_some_and(|wanted| ch.to_lowercase().any(|actual| actual == *wanted))
            {
                query.next();
                ranges.push(offset..offset + ch.len_utf8());
            }
        }
        if query.peek().is_some() {
            ranges.clear();
        }
        let highlights = ranges
            .into_iter()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        color: Some(cx.theme().blue),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();

        h_flex()
            .id(self.ix)
            .test_support()
            .role(gpui::Role::MenuItem)
            .aria_label(item.label.clone())
            .gap_2()
            .p_1()
            .text_xs()
            .line_height(relative(1.))
            .rounded(cx.theme().radius.half())
            .when(item.deprecated.unwrap_or(false), |this| this.line_through())
            .hover(|this| this.bg(cx.theme().accent.opacity(0.8)))
            .when(self.selected, |this| {
                this.bg(cx.theme().tokens.accent)
                    .text_color(cx.theme().accent_foreground)
            })
            .child(div().child(StyledText::new(item.label.clone()).with_highlights(highlights)))
            .when(item.detail.is_some(), |this| {
                this.child(
                    Label::new(item.detail.as_deref().unwrap_or("").to_string())
                        .text_color(cx.theme().muted_foreground)
                        .when(deprecated, |this| this.line_through())
                        .italic(),
                )
            })
            .children(self.children)
    }
}

impl EventEmitter<DismissEvent> for ContextMenuDelegate {}

impl ListDelegate for ContextMenuDelegate {
    type Item = CompletionMenuItem;

    fn items_count(&self, _: usize, _: &gpui::App) -> usize {
        self.items.len()
    }

    fn render_item(
        &mut self,
        ix: crate::IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let item = self.items.get(ix.row)?;
        Some(CompletionMenuItem::new(ix.row, item.clone()).highlight_prefix(self.query.clone()))
    }

    fn set_selected_index(
        &mut self,
        ix: Option<crate::IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected_ix = ix.map(|i| i.row).unwrap_or(0);
        cx.notify();
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(item) = self.selected_item() else {
            return;
        };

        let _ = self.menu.update(cx, |this, cx| {
            this.select_item(&item, window, cx);
        });
    }
}

/// A context menu for code completions and code actions.
pub struct CompletionMenu {
    offset: usize,
    editor: WeakEntity<EditorState>,
    list: Entity<ListState<ContextMenuDelegate>>,
    open: bool,
    note: Option<CompletionNote>,
    anchor: Option<(SharedString, usize, u64)>,

    /// The offset of the first character that triggered the completion.
    pub(crate) trigger_start_offset: Option<usize>,
    query: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl CompletionMenu {
    /// Creates a new `CompletionMenu` with the given offset and completion items.
    ///
    /// NOTE: This element should not call from EditorState::new, unless that will stack overflow.
    pub(crate) fn new(
        editor: Entity<EditorState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let view = cx.weak_entity();
            let menu = ContextMenuDelegate {
                query: SharedString::default(),
                menu: view,
                items: vec![],
                selected_ix: 0,
            };

            let list = cx.new(|cx| ListState::new(menu, window, cx));

            let _subscriptions =
                vec![
                    cx.subscribe(&list, |this: &mut Self, _, ev: &ListEvent, cx| {
                        match ev {
                            ListEvent::Confirm(_) => {
                                this.hide(cx);
                            }
                            _ => {}
                        }
                        cx.notify();
                    }),
                ];

            Self {
                offset: 0,
                editor: editor.downgrade(),
                list,
                open: false,
                note: None,
                anchor: None,
                trigger_start_offset: None,
                query: SharedString::default(),
                _subscriptions,
            }
        })
    }

    fn select_item(&mut self, item: &CompletionItem, window: &mut Window, cx: &mut Context<Self>) {
        let item = item.clone();
        let range = self.trigger_start_offset.unwrap_or(self.offset)..self.offset;

        let editor = self.editor.clone();
        let anchor = self.anchor.clone();

        cx.spawn_in(window, async move |_, cx| {
            editor.update_in(cx, |editor, window, cx| {
                if anchor.as_ref().is_none_or(|(text, cursor, revision)| {
                    editor.value() != *text
                        || editor.cursor() != *cursor
                        || editor.completion_menu_state().request_revision() != *revision
                }) {
                    return;
                }
                editor.insert_completion(&item, range, window, cx);
            })
        })
        .detach();

        self.hide(cx);
    }

    pub(crate) fn handle_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.open {
            return false;
        }

        if input::Enter::is_primary(&*action) {
            self.on_action_enter(window, cx);
        } else if action.partial_eq(&input::Escape) {
            self.on_action_escape(window, cx);
        } else if action.partial_eq(&input::MoveUp) {
            self.on_action_up(window, cx);
        } else if action.partial_eq(&input::MoveDown) {
            self.on_action_down(window, cx);
        } else {
            return false;
        }

        cx.stop_propagation();
        true
    }

    fn on_action_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.list.read(cx).delegate().selected_item().cloned() else {
            return;
        };
        self.select_item(&item, window, cx);
    }

    fn on_action_escape(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.hide(cx);
    }

    fn on_action_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.list.read(cx).delegate().items.is_empty() {
            return;
        }
        self.list.update(cx, |this, cx| {
            this.on_action_select_prev(&actions::SelectUp, window, cx)
        });
    }

    fn on_action_down(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.list.read(cx).delegate().items.is_empty() {
            return;
        }
        self.list.update(cx, |this, cx| {
            this.on_action_select_next(&actions::SelectDown, window, cx)
        });
    }

    /// Hide the completion menu and reset the trigger start offset.
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        let revision = self.anchor.as_ref().map(|(_, _, revision)| *revision);
        self.open = false;
        self.anchor = None;
        self.trigger_start_offset = None;
        let editor = self.editor.clone();
        if let Some(revision) = revision {
            cx.spawn(async move |_, cx| {
                let _ = editor.update(cx, |editor, cx| {
                    if editor.completion_menu_state().request_revision() == revision {
                        editor.dismiss_completion_overlay(cx);
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Sets the trigger start offset if it is not already set.
    pub(crate) fn update_query(&mut self, start_offset: usize, query: impl Into<SharedString>) {
        if self.trigger_start_offset.is_none() {
            self.trigger_start_offset = Some(start_offset);
        }
        self.query = query.into();
    }

    pub(crate) fn show(
        &mut self,
        offset: usize,
        items: impl Into<Vec<CompletionItem>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = items.into();
        self.offset = offset;
        self.anchor = self.editor.upgrade().map(|editor| {
            let editor = editor.read(cx);
            (
                editor.value(),
                offset,
                editor.completion_menu_state().request_revision(),
            )
        });
        self.open = true;
        self.list.update(cx, |this, cx| {
            let longest_ix = items
                .iter()
                .enumerate()
                .max_by_key(|(_, item)| {
                    item.label.len() + item.detail.as_ref().map(|d| d.len()).unwrap_or(0)
                })
                .map(|(ix, _)| ix)
                .unwrap_or(0);

            this.delegate_mut().query = self.query.clone();
            this.delegate_mut().set_items(items);
            let selected = (!this.delegate().items.is_empty())
                .then(|| IndexPath::new(this.delegate().selected_ix));
            this.set_selected_index(selected, window, cx);
            this.set_item_to_measure_index(IndexPath::new(longest_ix), window, cx);
        });

        cx.notify();
    }

    pub(crate) fn set_note(&mut self, note: Option<CompletionNote>) {
        self.note = note;
    }

    fn render_note(&self, has_items: bool, cx: &App) -> Option<AnyElement> {
        let note = self.note.clone()?;
        let editor = self.editor.clone();
        Some(
            h_flex()
                .id("completion-note")
                .test_support()
                .role(gpui::Role::Status)
                .aria_label(note.text().clone())
                .gap_2()
                .px_2()
                .py_1()
                .text_color(cx.theme().muted_foreground)
                .when(has_items, |this| {
                    this.border_t_1().border_color(cx.theme().border)
                })
                .child(div().flex_1().min_w_0().child(note.text().clone()))
                .when_some(note.action_label().cloned(), |this, label| {
                    this.child(
                        Button::new("completion-note-action")
                            .link()
                            .xsmall()
                            .label(label.clone())
                            .accessibility_label(label)
                            .disabled(note.is_loading())
                            .on_click(move |_, window, cx| {
                                note.activate(window, cx);
                                if let Some(editor) = editor.upgrade() {
                                    editor.update(cx, |editor, cx| editor.focus(window, cx));
                                }
                            }),
                    )
                })
                .into_any_element(),
        )
    }

    fn origin(&self, cx: &App) -> Option<Point<Pixels>> {
        let editor = self.editor.upgrade()?;
        let editor = editor.read(cx);
        let Some((cursor_bounds, line_height)) = editor.cursor_layout() else {
            return None;
        };
        let cursor_origin = cursor_bounds.origin;

        let scroll_origin = editor.scroll_offset();

        Some(
            scroll_origin + cursor_origin - editor.input_bounds().origin
                + Point::new(-px(4.), line_height + px(4.)),
        )
    }
}

impl Render for CompletionMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }

        let has_items = !self.list.read(cx).delegate().items.is_empty();
        if !has_items && self.note.is_none() {
            self.open = false;
            return Empty.into_any_element();
        }

        let Some(pos) = self.origin(cx) else {
            return Empty.into_any_element();
        };

        let selected_documentation = self
            .list
            .read(cx)
            .delegate()
            .selected_item()
            .and_then(|item| item.documentation.clone());

        let Some(editor) = self.editor.upgrade() else {
            return Empty.into_any_element();
        };
        let configured_max = editor.read(cx).lsp().completion_menu.max_width;
        let abs_pos = editor.read(cx).input_bounds().origin + pos;
        let max_width = configured_max
            .min(window.bounds().size.width - abs_pos.x)
            .max(px(120.));
        let vertical_layout =
            abs_pos.x + configured_max + POPOVER_GAP + configured_max + POPOVER_GAP
                > window.bounds().size.width;

        deferred(
            div()
                .absolute()
                .left(pos.x)
                .top(pos.y)
                .flex()
                .flex_row()
                .gap(POPOVER_GAP)
                .items_start()
                .when(vertical_layout, |this| this.flex_col())
                .child(
                    editor_popover("completion-menu", cx)
                        .test_support()
                        .role(gpui::Role::Menu)
                        .aria_label("SQL completions")
                        .max_w(max_width)
                        .min_w(px(120.))
                        .child(
                            v_flex()
                                .when(has_items, |this| {
                                    this.child(List::new(&self.list).max_h(MAX_MENU_HEIGHT))
                                })
                                .children(self.render_note(has_items, cx)),
                        ),
                )
                .when_some(selected_documentation, |this, documentation| {
                    let doc = match documentation {
                        lsp_types::Documentation::String(s) => s.clone(),
                        lsp_types::Documentation::MarkupContent(mc) => mc.value.clone(),
                    };
                    this.child(
                        div()
                            .id("completion-documentation")
                            .test_support()
                            .role(gpui::Role::Label)
                            .aria_label(doc.clone())
                            .child(
                                editor_popover("completion-documentation", cx)
                                    .w(max_width)
                                    .max_h(MAX_MENU_HEIGHT)
                                    .overflow_y_scrollbar()
                                    .px_2()
                                    .child(render_markdown("doc", doc, window, cx)),
                            ),
                    )
                })
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.hide(cx);
                })),
        )
        .into_any_element()
    }
}
