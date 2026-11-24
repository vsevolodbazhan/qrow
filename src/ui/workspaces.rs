//! Multi-workspace management: switching workspaces in the single main window,
//! creating, renaming, duplicating, revealing, and deleting named workspaces.

use super::setting_row::Rows;
use super::*;
use crate::model::{MAX_WORKSPACE_NAME, validate_workspace_name};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    Icon,
    alert::Alert,
    form::{field, v_form},
    h_flex,
    scroll::ScrollableElement as _,
    tag::Tag,
    v_flex,
};

const NAME_DIALOG_REMS: f32 = 28.;
const MANAGER_DIALOG_REMS: f32 = 38.;
const MANAGER_MAX_HEIGHT_REMS: f32 = 30.;

/// The Activity source label for workspace management failures.
const WORKSPACES_SOURCE: &str = "Workspaces";

pub(super) struct WorkspaceNameForm {
    /// `None` creates a new workspace; `Some(id)` renames an existing one.
    pub(super) target: Option<Uuid>,
    /// Reopen the Manage Workspaces dialog after closing this form.
    pub(super) reopen_manager: bool,
    pub(super) name: Entity<InputState>,
    pub(super) error: Option<String>,
}

/// What happens after the user confirms stopping active work before a switch.
#[derive(Clone, Copy)]
enum SwitchAction {
    Switch(Uuid),
    SwitchAndDelete { target: Uuid, delete: Uuid },
}

impl Qrow {
    /// Updates the native macOS Workspace menu (and cached menu state for
    /// shortcut rebuilds) to reflect the current workspace list.
    pub(super) fn sync_workspace_menus(&self, cx: &mut App) {
        let (workspaces, active) = match &self.store {
            Some(store) => (
                store
                    .workspaces()
                    .iter()
                    .map(|entry| (entry.id, entry.name.clone()))
                    .collect(),
                Some(store.active_workspace_id()),
            ),
            None => (Vec::new(), None),
        };
        shortcuts::set_workspace_menu(cx, workspaces, active);
        set_menus(cx, self.settings.assistant.enabled);
    }

    pub fn active_workspace_name(&self) -> Option<&str> {
        self.store
            .as_ref()
            .map(|store| store.active_workspace_entry().name.as_str())
    }

    pub(super) fn switch_to_workspace_action(
        &mut self,
        action: &SwitchToWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        self.request_workspace_switch(SwitchAction::Switch(action.0), window, cx);
    }

    pub(super) fn new_workspace_action(
        &mut self,
        _: &NewWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.is_none() || self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        self.open_workspace_name_dialog(None, false, window, cx);
    }

    pub(super) fn manage_workspaces_action(
        &mut self,
        _: &ManageWorkspaces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.is_none() || self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        self.open_manage_workspaces_dialog(window, cx);
    }

    pub(super) fn reveal_workspace_folder_action(
        &mut self,
        _: &RevealWorkspaceFolder,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(store) = &self.store {
            cx.reveal_path(&store.workspace_dir(store.active_workspace_id()));
        }
    }

    fn refresh_workspace_summaries(&mut self) {
        self.workspace_summaries.clear();
        let Some(store) = &self.store else {
            return;
        };
        let active_id = store.active_workspace_id();
        for entry in store.workspaces() {
            let counts = if entry.id == active_id {
                (self.profiles.len(), self.tabs.len())
            } else {
                storage::load(&store.workspace_path(entry.id))
                    .map(|ws| (ws.profiles.len(), ws.tabs.len()))
                    .unwrap_or((0, 0))
            };
            self.workspace_summaries.insert(entry.id, counts);
        }
    }

    fn switch_active_work_description(&self, cx: &App) -> Option<&'static str> {
        let assistant = self.assistant_working();
        let query = self.tabs.iter().any(|tab| tab.busy);
        let export = cx.global::<crate::export::Jobs>().active_count() > 0;
        let refresh = self
            .profiles
            .iter()
            .any(|profile| self.catalog.is_refreshing(profile.id));
        if export {
            return Some(if assistant || query || refresh {
                "Exports and other work are still running. Switch workspaces now to stop them?"
            } else {
                "An export is still running. Switch workspaces now to stop it?"
            });
        }
        match (assistant, query, refresh) {
            (true, true, _) => Some(
                "An assistant turn and a query are still running. Switch workspaces now to stop both?",
            ),
            (true, false, _) => {
                Some("The assistant is still working. Switch workspaces now to stop the turn?")
            }
            (false, true, _) => Some("A query is still running. Switch workspaces now to stop it?"),
            (false, false, true) => {
                Some("A metadata refresh is still running. Switch workspaces now to stop it?")
            }
            (false, false, false) => None,
        }
    }

    fn request_workspace_switch(
        &mut self,
        action: SwitchAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let target = match action {
            SwitchAction::Switch(id) => id,
            SwitchAction::SwitchAndDelete { target, .. } => target,
        };
        if target == store.active_workspace_id() && matches!(action, SwitchAction::Switch(_)) {
            return;
        }
        if store.workspace_entry(target).is_none() {
            return;
        }
        if self.form.as_ref().is_some_and(|form| form.saving.is_some()) {
            self.message = Some(
                "Wait for the connection to finish saving, then switch workspaces again.".into(),
            );
            cx.notify();
            return;
        }
        if let Some(description) = self.switch_active_work_description(cx) {
            self.confirm_switch_while_working(action, description, window, cx);
            return;
        }
        self.perform_workspace_switch(action, window, cx);
    }

    fn confirm_switch_while_working(
        &mut self,
        action: SwitchAction,
        description: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_switch_warning_open {
            return;
        }
        self.workspace_switch_warning_open = true;
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let keep = weak.clone();
            let proceed = weak.clone();
            let dismiss = weak.clone();
            alert
                .title("Work is still running")
                .description(description)
                .width(px(560.))
                .on_close(move |_, _, cx| {
                    let dismiss = dismiss.clone();
                    cx.defer(move |cx| {
                        let _ = dismiss.update(cx, |this, cx| {
                            this.workspace_switch_warning_open = false;
                            cx.notify();
                        });
                    });
                })
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("keep-working-switch")
                                .primary()
                                .label("Keep working")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = keep.update(cx, |this, cx| {
                                        this.workspace_switch_warning_open = false;
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Button::new("switch-anyway")
                                .label("Switch anyway")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = proceed.update(cx, |this, cx| {
                                        this.workspace_switch_warning_open = false;
                                        this.perform_workspace_switch(action, window, cx);
                                    });
                                }),
                        ),
                )
        });
        cx.notify();
    }

    fn perform_workspace_switch(
        &mut self,
        action: SwitchAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (target_id, delete_after) = match action {
            SwitchAction::Switch(id) => (id, None),
            SwitchAction::SwitchAndDelete { target, delete } => (target, Some(delete)),
        };

        // 1. Flush the current workspace and settings BEFORE releasing locks so
        //    a save failure keeps the user safely on the current workspace.
        let current_snapshot = self.snapshot(cx);
        if let Some(saver) = &self.saver {
            let flush = saver.flush(current_snapshot).and_then(|rx| {
                rx.recv()
                    .map_err(|_| {
                        anyhow::anyhow!("Workspace saver stopped before confirming the save")
                    })?
                    .map_err(anyhow::Error::msg)
            });
            if let Err(error) = flush {
                self.report_failure(
                    toast::Failure {
                        id: "workspace-switch-error",
                        title: "Could not save current workspace",
                        file: None,
                        log: crate::activity::APPLICATION,
                        source: WORKSPACES_SOURCE,
                        error: format!("{error:#}"),
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        if let Some(settings_saver) = &self.settings_saver {
            let settings = self.settings.clone();
            let flush = settings_saver.flush(settings.clone()).and_then(|rx| {
                rx.recv()
                    .map_err(|_| {
                        anyhow::anyhow!("Settings saver stopped before confirming the save")
                    })?
                    .map_err(anyhow::Error::msg)
            });
            if let Err(error) = flush {
                self.last_saved_settings = None;
                self.report_failure(
                    toast::Failure {
                        id: "workspace-switch-error",
                        title: "Could not save settings",
                        file: None,
                        log: crate::activity::APPLICATION,
                        source: WORKSPACES_SOURCE,
                        error: format!("{error:#}"),
                    },
                    window,
                    cx,
                );
                return;
            }
            self.last_saved_settings = Some(settings);
        }

        // 2. Stop live work in the outgoing workspace.
        let previous_id = self
            .store
            .as_ref()
            .map(storage::DataStore::active_workspace_id);
        cx.global::<crate::export::Jobs>()
            .cancel_current_and_wait(Duration::from_secs(2));
        self.sign_in_ui.cancel_all();
        self.assistant_state.save_transcripts = None;
        self.save_transcripts(None);
        if let Some(store) = &self.assistant_state.store {
            let _ = store.flush(Duration::from_secs(2));
        }
        self.assistant_state.shutdown();
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.shutdown();
            }
        }
        self.catalog.shutdown();
        let deadline = Instant::now() + Duration::from_secs(1);
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.wait_for_shutdown(deadline.saturating_duration_since(Instant::now()));
            }
        }
        self.catalog.wait_for_shutdown(deadline);

        // Release the outgoing workspace's lock.
        if let Some(mut saver) = self.saver.take() {
            let _ = saver.stop();
        }

        // 3. Open the target workspace. If opening fails, reopen the previous
        //    workspace so the user stays in a valid writable state.
        let wake = self.wake.clone();
        let open_result = self.store.as_ref().map(|store| {
            store.open_workspace_saver(target_id, move || {
                let _ = wake.try_send(());
            })
        });
        let (mut workspace, new_saver) = match open_result {
            Some(Ok(opened)) => opened,
            Some(Err(error)) => {
                // Attempt to reclaim the previous workspace lock.
                if let (Some(store), Some(prev_id)) = (&self.store, previous_id) {
                    let wake = self.wake.clone();
                    if let Ok((_, restored_saver)) =
                        store.open_workspace_saver(prev_id, move || {
                            let _ = wake.try_send(());
                        })
                    {
                        self.saver = Some(restored_saver);
                    }
                }
                self.report_failure(
                    toast::Failure {
                        id: "workspace-switch-error",
                        title: "Could not open workspace",
                        file: None,
                        log: crate::activity::APPLICATION,
                        source: WORKSPACES_SOURCE,
                        error: format!("{error:#}"),
                    },
                    window,
                    cx,
                );
                cx.notify();
                return;
            }
            None => return,
        };

        if let Some(store) = &mut self.store
            && let Err(error) = store.set_active_workspace(target_id)
        {
            self.report_failure(
                toast::Failure {
                    id: "workspace-switch-error",
                    title: "Could not update active workspace",
                    file: None,
                    log: crate::activity::APPLICATION,
                    source: WORKSPACES_SOURCE,
                    error: format!("{error:#}"),
                },
                window,
                cx,
            );
        }

        // 4. Optional post-switch deletion (when deleting the active workspace).
        if let Some(delete_id) = delete_after {
            self.finish_workspace_deletion(delete_id, window, cx);
        }

        // 5. Load the target workspace into the window while keeping global
        //    settings, updates, and SettingsWindow intact.
        let local_dir = self
            .store
            .as_ref()
            .map(storage::DataStore::active_local_directory);
        workspace.normalize();
        self.message = new_saver.newer.as_ref().map(storage::NewerData::notice);
        self.saver = Some(new_saver);
        self.dirty = None;
        self.profiles = workspace.profiles;
        self.shared_catalogs = workspace.shared_catalogs;
        self.sign_ins = workspace.sign_ins;
        self.active_tabs = workspace.active_tabs;
        self.assistant = workspace.assistant;
        self.exports.clear();
        self.menu = None;
        self.completion = None;
        self.completion_reads.clear();
        self.completion_statements = None;
        self.form = None;
        self.tab_form = None;
        self.result_form = None;
        self.dbt_details = None;
        self.dbt_details_origin = None;

        self.oidc.configure(&self.sign_ins, &self.profiles);
        self.external_auth.configure(&self.profiles);
        self.sign_in_ui = sign_in_view::SignInState::new(self.browser.clone());

        self.tabs.clear();
        for saved_tab in workspace.tabs {
            let tab = self.make_tab(saved_tab, window, cx);
            self.tabs.push(tab);
        }
        if self.tabs.is_empty() {
            let tab = self.make_tab(
                SavedTab::new(1, self.profiles.first().map(|p| p.id)),
                window,
                cx,
            );
            self.tabs.push(tab);
        }
        self.active = workspace.active_tab.min(self.tabs.len() - 1);

        let mut catalog = catalog_tree::CatalogTree::new(local_dir.clone(), window, cx);
        catalog.scope = workspace.search_scope;
        self._catalog_subscriptions = [
            cx.subscribe(
                &catalog.state,
                |this, _, event: &gpui_kit::base::TreeEvent, cx| {
                    this.catalog_expansion_changed(event, cx)
                },
            ),
            cx.subscribe(&catalog.search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.catalog_search_changed(cx);
                }
            }),
        ];
        self.catalog = catalog;
        self.sync_catalog_keys();

        let dbt_dir = local_dir
            .as_ref()
            .map(|local| storage::dbt_index_directory(local));
        self.dbt = dbt::DbtProjects::new(dbt_dir, self.wake.clone());
        self.sync_dbt();

        self.activity
            .update(cx, |view, cx| view.reset_for_workspace(window, cx));

        let mut assistant_state = assistant_view::AssistantState::new(cx);
        assistant_state.store = assistant_view::transcript_store(local_dir.as_deref());
        assistant_state.composer_target = self
            .tabs
            .get(self.active)
            .map(|tab| assistant_view::ComposerTarget::Tab(tab.saved.id));
        self.assistant_state = assistant_state;

        self.rebuild_catalog_tree(cx);
        self.sync_workspace_menus(cx);
        self.tabs[self.active]
            .input
            .update(cx, |editor, cx| editor.focus(window, cx));
        cx.notify();
    }

    fn finish_workspace_deletion(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = &mut self.store else {
            return;
        };
        let credentials = self.credentials.clone();
        let tokens = self.tokens.clone();
        match store.delete_workspace(id, credentials.as_ref(), tokens.as_ref()) {
            Ok(warnings) => {
                for warning in warnings {
                    let entry = crate::activity::ActivityEntry::new(Severity::Error, warning)
                        .with_source(WORKSPACES_SOURCE);
                    self.record_activity(crate::activity::APPLICATION, entry, cx);
                }
                self.sync_workspace_menus(cx);
            }
            Err(error) => {
                self.report_failure(
                    toast::Failure {
                        id: "workspace-delete-error",
                        title: "Could not delete workspace",
                        file: None,
                        log: crate::activity::APPLICATION,
                        source: WORKSPACES_SOURCE,
                        error: format!("{error:#}"),
                    },
                    window,
                    cx,
                );
            }
        }
    }

    /// Opens the New Workspace or Rename Workspace dialog.
    pub(super) fn open_workspace_name_dialog(
        &mut self,
        target: Option<Uuid>,
        reopen_manager: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = target
            .and_then(|id| self.store.as_ref()?.workspace_entry(id))
            .map(|entry| entry.name.clone())
            .unwrap_or_default();
        let placeholder = if target.is_some() {
            "Workspace name"
        } else {
            "Analytics"
        };
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        let input_focus = name.clone();
        self.workspace_name_form = Some(WorkspaceNameForm {
            target,
            reopen_manager,
            name,
            error: None,
        });
        let title = if target.is_some() {
            "Rename Workspace"
        } else {
            "New Workspace"
        };
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let save = weak.clone();
            let close = weak.clone();
            let content = weak
                .update(cx, |this, cx| this.workspace_name_content(cx))
                .ok();
            let footer = weak
                .update(cx, |this, cx| this.workspace_name_footer(cx))
                .ok();
            dialog
                .title(title)
                .w(Rows::dialog_width(window, NAME_DIALOG_REMS))
                .overlay_closable(false)
                .on_ok(move |_, window, cx| {
                    let _ = save.update(cx, |this, cx| this.submit_workspace_name(window, cx));
                    false
                })
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, window, cx| {
                    let close = close.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = close.update(cx, |this, cx| {
                            let reopen = this
                                .workspace_name_form
                                .take()
                                .is_some_and(|form| form.reopen_manager);
                            if reopen {
                                this.reopen_manage_workspaces_deferred(window, cx);
                            }
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

    fn workspace_name_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.workspace_name_form else {
            return div().into_any_element();
        };
        v_flex()
            .id("workspace-name-form")
            .key_context("WorkspaceNameDialog")
            .on_action(cx.listener(|this, _: &SubmitWorkspaceName, window, cx| {
                this.submit_workspace_name(window, cx);
            }))
            .w_full()
            .gap_2()
            .child(
                v_form().w_full().child(
                    field()
                        .label("Name")
                        .description(format!(
                            "Unique name, {MAX_WORKSPACE_NAME} characters or fewer."
                        ))
                        .child(
                            Input::new(&form.name)
                                .id("workspace-name-input")
                                .focus_ring(false)
                                .w_full()
                                .aria_label("Name"),
                        ),
                ),
            )
            .when_some(form.error.clone(), |content, error| {
                content.child(
                    div()
                        .id("workspace-name-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .child(Alert::error("workspace-name-error-alert", error)),
                )
            })
            .into_any_element()
    }

    fn workspace_name_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.workspace_name_form else {
            return div().into_any_element();
        };
        let is_rename = form.target.is_some();
        let submit_label = if is_rename { "Rename" } else { "Create" };
        let tooltip_label = if is_rename {
            "Rename workspace"
        } else {
            "Create workspace"
        };
        v_flex()
            .w_full()
            .gap_2()
            .key_context("WorkspaceNameDialog")
            .on_action(cx.listener(|this, _: &SubmitWorkspaceName, window, cx| {
                this.submit_workspace_name(window, cx);
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-workspace-name")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let reopen = this
                                    .workspace_name_form
                                    .take()
                                    .is_some_and(|form| form.reopen_manager);
                                window.close_dialog(cx);
                                if reopen {
                                    this.reopen_manage_workspaces_deferred(window, cx);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("submit-workspace-name")
                            .primary()
                            .label(submit_label)
                            .map(|mut button| {
                                button.interactivity().tooltip(
                                    StatusTooltip::new(tooltip_label, "").for_action(
                                        &SubmitWorkspaceName,
                                        Some("WorkspaceNameDialog"),
                                    ),
                                );
                                button
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_workspace_name(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn submit_workspace_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.workspace_name_form else {
            return;
        };
        let raw_name = form.name.read(cx).value().to_string();
        let target = form.target;
        let reopen_manager = form.reopen_manager;
        let validated = match validate_workspace_name(&raw_name) {
            Ok(name) => name,
            Err(error) => {
                if let Some(form) = &mut self.workspace_name_form {
                    form.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };

        let result = match (self.store.as_mut(), target) {
            (Some(store), Some(id)) => store.rename_workspace(id, &validated).map(|_| None),
            (Some(store), None) => store
                .create_workspace(&validated)
                .map(|entry| Some(entry.id)),
            (None, _) => return,
        };

        match result {
            Ok(created_id) => {
                self.workspace_name_form = None;
                self.sync_workspace_menus(cx);
                window.close_dialog(cx);
                if let Some(id) = created_id {
                    if !reopen_manager {
                        self.request_workspace_switch(SwitchAction::Switch(id), window, cx);
                    } else {
                        self.reopen_manage_workspaces_deferred(window, cx);
                    }
                } else if reopen_manager {
                    self.reopen_manage_workspaces_deferred(window, cx);
                }
                cx.notify();
            }
            Err(error) => {
                if let Some(form) = &mut self.workspace_name_form {
                    form.error = Some(error.to_string());
                }
                cx.notify();
            }
        }
    }

    fn reopen_manage_workspaces_deferred(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |this, window, cx| {
            this.open_manage_workspaces_dialog(window, cx);
        });
    }

    /// Opens the Manage Workspaces dialog.
    pub(super) fn open_manage_workspaces_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.is_none() {
            return;
        }
        self.refresh_workspace_summaries();
        self.workspace_manager_open = true;
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let content = weak
                .update(cx, |this, cx| this.manage_workspaces_content(window, cx))
                .ok();
            let footer = weak
                .update(cx, |this, cx| this.manage_workspaces_footer(cx))
                .ok();
            dialog
                .title("Manage Workspaces")
                .w(Rows::dialog_width(window, MANAGER_DIALOG_REMS))
                .overlay_closable(true)
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let close = close.clone();
                    cx.defer(move |cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.workspace_manager_open = false;
                            cx.notify();
                        });
                    });
                })
        });
        cx.notify();
    }

    fn manage_workspaces_content(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(store) = &self.store else {
            return div().into_any_element();
        };
        let active_id = store.active_workspace_id();
        let can_delete = store.workspaces().len() > 1;
        let max_height = (window.rem_size() * MANAGER_MAX_HEIGHT_REMS)
            .min(window.viewport_size().height - window.rem_size() * 12.)
            .max(px(160.));
        let entries: Vec<_> = store.workspaces().to_vec();

        div()
            .id("manage-workspaces-list")
            .test_support()
            .w_full()
            .child(
                v_flex()
                    .w_full()
                    .max_h(max_height)
                    .overflow_y_scrollbar()
                    .gap_1()
                    .children(entries.into_iter().map(|entry| {
                        let id = entry.id;
                        let name = entry.name;
                        let is_active = id == active_id;
                        let (connections, tabs) =
                            self.workspace_summaries.get(&id).copied().unwrap_or((0, 0));
                        let summary = format!(
                            "{connections} {}, {tabs} {}",
                            if connections == 1 {
                                "connection"
                            } else {
                                "connections"
                            },
                            if tabs == 1 { "tab" } else { "tabs" }
                        );
                        let row_id = SharedString::from(format!("workspace-row-{id}"));
                        let rename_name = name.clone();
                        let delete_name = name.clone();
                        let dup_name = name.clone();
                        let reveal_name = name.clone();
                        let switch_name = name.clone();

                        h_flex()
                            .id(row_id)
                            .test_support()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(if is_active {
                                cx.theme().primary.opacity(0.45)
                            } else {
                                cx.theme().border
                            })
                            .bg(if is_active {
                                cx.theme().secondary.opacity(0.6)
                            } else {
                                cx.theme().background
                            })
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .min_w_0()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(name),
                                            )
                                            .when(is_active, |row| {
                                                row.child(Tag::secondary().small().child("Active"))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(summary),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap_1()
                                    .when(!is_active, |actions| {
                                        actions.child(
                                            Button::new(SharedString::from(format!(
                                                "workspace-switch-{id}"
                                            )))
                                            .small()
                                            .label("Switch")
                                            .accessibility_label(format!("Switch to {switch_name}"))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.workspace_manager_open = false;
                                                window.close_dialog(cx);
                                                this.request_workspace_switch(
                                                    SwitchAction::Switch(id),
                                                    window,
                                                    cx,
                                                );
                                            })),
                                        )
                                    })
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "workspace-rename-{id}"
                                        )))
                                        .ghost()
                                        .small()
                                        .icon(Icon::new(AssetIconName::Pencil).small())
                                        .accessibility_label(format!("Rename {rename_name}"))
                                        .tooltip("Rename workspace")
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.workspace_manager_open = false;
                                                window.close_dialog(cx);
                                                cx.defer_in(window, move |this, window, cx| {
                                                    this.open_workspace_name_dialog(
                                                        Some(id),
                                                        true,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "workspace-duplicate-{id}"
                                        )))
                                        .ghost()
                                        .small()
                                        .icon(Icon::new(AssetIconName::Copy).small())
                                        .accessibility_label(format!("Duplicate {dup_name}"))
                                        .tooltip("Duplicate workspace")
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.duplicate_workspace_entry(id, window, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "workspace-reveal-{id}"
                                        )))
                                        .ghost()
                                        .small()
                                        .icon(Icon::new(AssetIconName::FolderOpen).small())
                                        .accessibility_label(format!(
                                            "Reveal folder for {reveal_name}"
                                        ))
                                        .tooltip("Reveal workspace folder")
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                if let Some(store) = &this.store {
                                                    cx.reveal_path(&store.workspace_dir(id));
                                                }
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "workspace-delete-{id}"
                                        )))
                                        .ghost()
                                        .small()
                                        .disabled(!can_delete)
                                        .icon(Icon::new(AssetIconName::Trash).small())
                                        .accessibility_label(format!("Delete {delete_name}"))
                                        .tooltip(if can_delete {
                                            "Delete workspace"
                                        } else {
                                            "Cannot delete the only workspace"
                                        })
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.workspace_manager_open = false;
                                                window.close_dialog(cx);
                                                let name = delete_name.clone();
                                                cx.defer_in(window, move |this, window, cx| {
                                                    this.confirm_delete_workspace(
                                                        id, name, window, cx,
                                                    );
                                                });
                                            }),
                                        ),
                                    ),
                            )
                    })),
            )
            .into_any_element()
    }

    fn manage_workspaces_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .w_full()
            .gap_2()
            .child(
                Button::new("manage-workspaces-new")
                    .label("New workspace…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.workspace_manager_open = false;
                        window.close_dialog(cx);
                        cx.defer_in(window, |this, window, cx| {
                            this.open_workspace_name_dialog(None, true, window, cx);
                        });
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("manage-workspaces-close")
                    .primary()
                    .label("Close")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.workspace_manager_open = false;
                        window.close_dialog(cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn duplicate_workspace_entry(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let is_active = self
            .store
            .as_ref()
            .is_some_and(|store| store.active_workspace_id() == id);
        let live_snapshot = is_active.then(|| self.snapshot(cx));
        let result = self
            .store
            .as_mut()
            .map(|store| store.duplicate_workspace(id, live_snapshot.as_ref(), None));
        match result {
            Some(Ok(_)) => {
                self.refresh_workspace_summaries();
                self.sync_workspace_menus(cx);
                cx.notify();
            }
            Some(Err(error)) => {
                self.report_failure(
                    toast::Failure {
                        id: "workspace-duplicate-error",
                        title: "Could not duplicate workspace",
                        file: None,
                        log: crate::activity::APPLICATION,
                        source: WORKSPACES_SOURCE,
                        error: format!("{error:#}"),
                    },
                    window,
                    cx,
                );
            }
            None => {}
        }
    }

    fn confirm_delete_workspace(
        &mut self,
        id: Uuid,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let cancel = weak.clone();
            let confirm = weak.clone();
            alert
                .title(format!("Delete \"{name}\"?"))
                .description(
                    "All connections, query tabs, saved passwords, and sign-ins in this workspace will be permanently deleted.",
                )
                .width(px(480.))
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("cancel-delete-workspace")
                                .label("Cancel")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = cancel.update(cx, |this, cx| {
                                        this.reopen_manage_workspaces_deferred(window, cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("confirm-delete-workspace")
                                .label("Delete")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = confirm.update(cx, |this, cx| {
                                        this.delete_workspace_confirmed(id, window, cx);
                                    });
                                }),
                        ),
                )
        });
        cx.notify();
    }

    fn delete_workspace_confirmed(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        if store.workspaces().len() <= 1 {
            return;
        }
        if store.active_workspace_id() == id {
            let Some(next) = store
                .workspaces()
                .iter()
                .find(|entry| entry.id != id)
                .map(|entry| entry.id)
            else {
                return;
            };
            self.request_workspace_switch(
                SwitchAction::SwitchAndDelete {
                    target: next,
                    delete: id,
                },
                window,
                cx,
            );
        } else {
            self.finish_workspace_deletion(id, window, cx);
            self.reopen_manage_workspaces_deferred(window, cx);
        }
    }
}
