use gpui::{
    App, IntoElement, ParentElement as _, SharedString, StyleRefinement, Styled, Window,
    prelude::FluentBuilder as _,
};

use crate::{
    ActiveTheme, StyledExt,
    group_box::{GroupBox, GroupBoxVariants},
    label::Label,
    setting::{RenderOptions, SettingItem},
    v_flex,
};

/// A setting group that can contain multiple setting items.
#[derive(Clone)]
pub struct SettingGroup {
    style: StyleRefinement,

    pub(super) title: Option<SharedString>,
    pub(super) description: Option<SharedString>,
    pub(super) items: Vec<SettingItem>,
}

impl Styled for SettingGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl SettingGroup {
    /// Create a new setting group.
    pub fn new() -> Self {
        Self {
            style: StyleRefinement::default(),
            title: None,
            description: None,
            items: Vec::new(),
        }
    }

    /// Set the label of the setting group, default is None.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the description of the setting group, default is None.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a setting item to the group.
    pub fn item(mut self, item: SettingItem) -> Self {
        self.items.push(item);
        self
    }

    /// Add multiple setting items to the group.
    pub fn items<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = SettingItem>,
    {
        self.items.extend(items);
        self
    }

    /// Return true if any of the setting items in the group match the given query.
    pub(super) fn is_match(&self, query: &str, cx: &App) -> bool {
        self.items.iter().any(|item| item.is_match(query, cx))
    }

    pub(super) fn is_resettable(&self, query: &str, cx: &App) -> bool {
        self.items
            .iter()
            .any(|item| item.is_match(query, cx) && item.is_resettable(cx))
    }

    pub(crate) fn render(
        self,
        query: &str,
        options: &RenderOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        GroupBox::new()
            .id(SharedString::from(format!("group-{}", options.group_ix())))
            .with_variant(options.group_variant())
            .when_some(self.title.clone(), |this, title| {
                this.title(v_flex().gap_1().child(title).when_some(
                    self.description.clone(),
                    |this, description| {
                        this.child(
                            Label::new(description)
                                .text_sm()
                                .text_color(cx.theme().muted_foreground),
                        )
                    },
                ))
            })
            .gap_4()
            .when(options.item_dividers(), |this| {
                this.content_style(StyleRefinement::default().p_0().gap_0())
            })
            .children(
                self.items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.is_match(&query, cx))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .enumerate()
                    .map(|(shown_ix, (item_ix, item))| {
                        let item = item
                            .clone()
                            .render_item(&options.with_item_ix(item_ix), window, cx);
                        if options.item_dividers() {
                            // The divider runs the full width of the group;
                            // only the content keeps the padding.
                            item.px_4()
                                .py_2p5()
                                .when(shown_ix > 0, |item| {
                                    item.border_t_1().border_color(cx.theme().border)
                                })
                        } else {
                            item
                        }
                    }),
            )
            .refine_style(&self.style)
    }

    pub(crate) fn reset(&self, query: &str, window: &mut Window, cx: &mut App) {
        for item in &self.items {
            if item.is_match(query, cx) {
                item.reset(window, cx);
            }
        }
    }
}
