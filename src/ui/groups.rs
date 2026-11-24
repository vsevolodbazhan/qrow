//! Connection groups and semantic environment badges.

use super::setting_row::Rows;
use super::*;
use crate::model::{ConnectionEnvironment, ConnectionGroup, MAX_GROUP_NAME, validate_group_name};
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    alert::Alert,
    form::{field, v_form},
    h_flex,
    tag::Tag,
    v_flex,
};

const GROUP_DIALOG_REMS: f32 = 26.;

pub(super) struct GroupNameForm {
    /// `None` creates a new group; `Some(id)` renames an existing group.
    pub(super) target: Option<Uuid>,
    /// Optional connection to move into the newly created group.
    pub(super) assign_profile: Option<Uuid>,
    pub(super) name: Entity<InputState>,
    pub(super) error: Option<String>,
}

/// Renders a compact semantic badge for a connection's environment using only
/// GPUI Kit theme tokens (never hardcoded hex colors).
pub(super) fn environment_badge(
    id: impl Into<ElementId>,
    environment: ConnectionEnvironment,
    compact: bool,
) -> impl IntoElement {
    let label = if compact {
        environment.short_label()
    } else {
        environment.label()
    };
    let tag = match environment {
        ConnectionEnvironment::Production => Tag::danger(),
        ConnectionEnvironment::Staging => Tag::warning(),
        ConnectionEnvironment::Development => Tag::info(),
        ConnectionEnvironment::Local => Tag::secondary(),
    };
    div()
        .id(id)
        .test_support()
        .flex_shrink_0()
        .role(Role::Status)
        .aria_label(format!("Environment: {}", environment.label()))
        .child(tag.xsmall().outline().child(label))
}

impl Qrow {
    /// Opens the New Group or Rename Group dialog.
    pub(super) fn open_group_name_dialog(
        &mut self,
        target: Option<Uuid>,
        assign_profile: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let initial = target
            .and_then(|id| self.groups.iter().find(|group| group.id == id))
            .map(|group| group.name.clone())
            .unwrap_or_default();
        let placeholder = if target.is_some() {
            "Group name"
        } else {
            "Production"
        };
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        let input_focus = name.clone();
        self.group_name_form = Some(GroupNameForm {
            target,
            assign_profile,
            name,
            error: None,
        });
        let title = if target.is_some() {
            "Rename Group"
        } else {
            "New Group"
        };
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let save = weak.clone();
            let close = weak.clone();
            let content = weak.update(cx, |this, cx| this.group_name_content(cx)).ok();
            let footer = weak.update(cx, |this, cx| this.group_name_footer(cx)).ok();
            dialog
                .title(title)
                .w(Rows::dialog_width(window, GROUP_DIALOG_REMS))
                .overlay_closable(false)
                .on_ok(move |_, window, cx| {
                    let _ = save.update(cx, |this, cx| this.submit_group_name(window, cx));
                    false
                })
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let close = close.clone();
                    cx.defer(move |cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.group_name_form = None;
                            cx.notify();
                        });
                    });
                })
        });
        cx.defer_in(window, move |_, window, cx| {
            input_focus.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        });
        cx.notify();
    }

    fn group_name_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.group_name_form else {
            return div().into_any_element();
        };
        v_flex()
            .id("group-name-form")
            .key_context("GroupNameDialog")
            .on_action(cx.listener(|this, _: &SubmitGroupName, window, cx| {
                this.submit_group_name(window, cx);
            }))
            .w_full()
            .gap_2()
            .child(
                v_form().w_full().child(
                    field()
                        .label("Name")
                        .description(format!(
                            "Unique name, {MAX_GROUP_NAME} characters or fewer."
                        ))
                        .child(
                            Input::new(&form.name)
                                .id("group-name-input")
                                .focus_ring(false)
                                .w_full()
                                .aria_label("Name"),
                        ),
                ),
            )
            .when_some(form.error.clone(), |content, error| {
                content.child(
                    div()
                        .id("group-name-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .child(Alert::error("group-name-error-alert", error)),
                )
            })
            .into_any_element()
    }

    fn group_name_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.group_name_form else {
            return div().into_any_element();
        };
        let is_rename = form.target.is_some();
        let submit_label = if is_rename { "Rename" } else { "Create" };
        let tooltip_label = if is_rename {
            "Rename group"
        } else {
            "Create group"
        };
        v_flex()
            .w_full()
            .gap_2()
            .key_context("GroupNameDialog")
            .on_action(cx.listener(|this, _: &SubmitGroupName, window, cx| {
                this.submit_group_name(window, cx);
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-group-name")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.group_name_form = None;
                                window.close_dialog(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("submit-group-name")
                            .primary()
                            .label(submit_label)
                            .map(|mut button| {
                                button.interactivity().tooltip(
                                    StatusTooltip::new(tooltip_label, "")
                                        .for_action(&SubmitGroupName, Some("GroupNameDialog")),
                                );
                                button
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_group_name(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn submit_group_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.group_name_form else {
            return;
        };
        let raw_name = form.name.read(cx).value().to_string();
        let target = form.target;
        let assign_profile = form.assign_profile;
        let validated = match validate_group_name(&raw_name) {
            Ok(name) => name,
            Err(error) => {
                if let Some(form) = &mut self.group_name_form {
                    form.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };
        let duplicate = self.groups.iter().any(|group| {
            target != Some(group.id) && group.name.trim().to_lowercase() == validated.to_lowercase()
        });
        if duplicate {
            if let Some(form) = &mut self.group_name_form {
                form.error = Some(format!("A group named \"{validated}\" already exists."));
            }
            cx.notify();
            return;
        }
        match target {
            Some(id) => {
                if let Some(group) = self.groups.iter_mut().find(|group| group.id == id) {
                    group.name = validated;
                }
            }
            None => {
                let group = ConnectionGroup::new(validated);
                let new_id = group.id;
                self.groups.push(group);
                if let Some(profile_id) = assign_profile
                    && let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id)
                {
                    profile.group_id = Some(new_id);
                }
            }
        }
        self.group_name_form = None;
        window.close_dialog(cx);
        self.rebuild_catalog_tree(cx);
        self.changed(cx);
    }

    /// Assigns `profile_id` to `group_id` (or removes it from any group when
    /// `group_id` is `None`).
    pub(super) fn set_connection_group(
        &mut self,
        profile_id: Uuid,
        group_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        if group_id.is_some_and(|id| !self.groups.iter().any(|group| group.id == id)) {
            return;
        }
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == profile_id) else {
            return;
        };
        if profile.group_id == group_id {
            return;
        }
        profile.group_id = group_id;
        // Expand the target group so the moved connection stays visible.
        if let Some(id) = group_id
            && let Some(group) = self.groups.iter_mut().find(|g| g.id == id)
        {
            group.collapsed = false;
        }
        self.rebuild_catalog_tree(cx);
        self.changed(cx);
    }

    /// Deletes a connection group without deleting any of its connections.
    /// Member connections become ungrouped.
    pub(super) fn delete_group(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let before = self.groups.len();
        self.groups.retain(|group| group.id != id);
        if self.groups.len() == before {
            return;
        }
        for profile in &mut self.profiles {
            if profile.group_id == Some(id) {
                profile.group_id = None;
            }
        }
        self.rebuild_catalog_tree(cx);
        self.changed(cx);
    }

    /// Opens the context menu for a connection group row in the sidebar.
    pub(super) fn open_group_menu(
        &mut self,
        id: Uuid,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.groups.iter().any(|group| group.id == id) {
            return;
        }
        let new_connection = cx.listener(move |this, _: &ClickEvent, window, cx| {
            let profile = Profile {
                group_id: Some(id),
                ..Profile::default()
            };
            this.edit_profile(profile, true, window, cx);
        });
        let rename = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_group_name_dialog(Some(id), None, window, cx);
        });
        let delete = cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.delete_group(id, cx);
        });
        let tree = self.catalog.state.read(cx).focus_handle(cx);
        self.open_context_menu_from(
            position,
            Some(tree),
            move |menu, _, _| {
                menu.item(menu_section("Group"))
                    .item(PopupMenuItem::new("New connection in group").on_click(new_connection))
                    .item(PopupMenuItem::new("Rename group…").on_click(rename))
                    .separator()
                    .item(PopupMenuItem::new("Delete group").on_click(delete))
            },
            window,
            cx,
        );
    }
}
