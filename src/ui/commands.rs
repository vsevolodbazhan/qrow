//! Commands without a default shortcut. The user can give each one a
//! shortcut in Settings. Each one does what its button or menu item does, on
//! the active query tab and its shown result. A command whose target is out
//! of view, under a dialog, Activity, or the maximized assistant, does
//! nothing, because a shortcut acts on what the window shows.

use super::*;
use crate::catalog::Scope;
use catalog_tree::Node;

impl Qrow {
    /// True when the query tab, its editor, and its results are in view.
    fn workspace_in_view(&self, cx: &App) -> bool {
        !self.dialog_open()
            && !self.assistant_covers_workspace(cx)
            && !self.activity.read(cx).is_open()
    }

    pub(super) fn cancel_query(&mut self, _: &CancelQuery, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel(cx);
    }

    pub(super) fn export_results(
        &mut self,
        _: &ExportResults,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_in_view(cx) {
            return;
        }
        let tab = &self.tabs[self.active];
        let result = tab.shown_result();
        // Like the Export button: a cursor drain reads only the live result.
        let draining = tab.live == Some(result.id)
            && (tab.download.is_some() || tab.cursor == crate::worker::Cursor::Draining);
        if !draining {
            self.open_export(window, cx);
        }
    }

    pub(super) fn rename_active_tab(
        &mut self,
        _: &RenameTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_in_view(cx) {
            return;
        }
        let tab = self.tabs[self.active].saved.id;
        self.open_tab_rename(tab, window, cx);
    }

    pub(super) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.step_tab(1, window, cx);
    }

    pub(super) fn previous_tab(
        &mut self,
        _: &PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_tab(-1, window, cx);
    }

    /// Selects the query tab `step` places away in the tab strip, and wraps
    /// at its ends.
    fn step_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_in_view(cx) {
            return;
        }
        let visible = self.visible_tab_indices();
        let Some(position) = visible.iter().position(|index| *index == self.active) else {
            return;
        };
        let next = (position as isize + step).rem_euclid(visible.len() as isize) as usize;
        if visible[next] != self.active {
            self.activate(visible[next], window, cx);
        }
    }

    pub(super) fn pin_result(
        &mut self,
        _: &PinResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_in_view(cx) {
            return;
        }
        let tab = &self.tabs[self.active];
        let (tab_id, result_id) = (tab.saved.id, tab.shown_result().id);
        self.toggle_result_pin(tab_id, result_id, window, cx);
    }

    pub(super) fn close_shown_result(
        &mut self,
        _: &CloseResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_in_view(cx) {
            return;
        }
        let tab = &self.tabs[self.active];
        let (tab_id, result_id) = (tab.saved.id, tab.shown_result().id);
        self.close_result(tab_id, result_id, window, cx);
    }

    pub(super) fn next_result_tab(
        &mut self,
        _: &NextResultTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_result_tab(1, cx);
    }

    pub(super) fn previous_result_tab(
        &mut self,
        _: &PreviousResultTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_result_tab(-1, cx);
    }

    /// Shows the result tab `step` places away, like a click on it, and
    /// wraps at the ends of the result tabs.
    fn step_result_tab(&mut self, step: isize, cx: &mut Context<Self>) {
        if !self.workspace_in_view(cx) {
            return;
        }
        self.restore_results(cx);
        let tab = &mut self.tabs[self.active];
        let current = tab.shown_result().id;
        let Some(position) = tab.results.iter().position(|result| result.id == current) else {
            return;
        };
        let count = tab.results.len() as isize;
        let next = (position as isize + step).rem_euclid(count) as usize;
        tab.active_result = tab.results[next].id;
        tab.panel.user_select(Panel::Results);
        cx.notify();
    }

    pub(super) fn copy_shown_query(
        &mut self,
        _: &CopyQuery,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_in_view(cx) {
            return;
        }
        let tab = &self.tabs[self.active];
        let (tab_id, result_id) = (tab.saved.id, tab.shown_result().id);
        self.copy_result_query(tab_id, result_id, cx);
    }

    /// Refreshes the selected schema or table of the connection tree, like
    /// Refresh metadata in its menu. Without such a selection, it refreshes
    /// the connection of the active tab.
    pub(super) fn refresh_metadata(
        &mut self,
        _: &RefreshMetadata,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() {
            return;
        }
        let selected = self
            .catalog
            .state
            .read(cx)
            .selected_item()
            .and_then(|item| self.catalog.node(&item.id))
            .and_then(|node| match node {
                Node::Schema { profile, name, .. } => Some((*profile, Scope::Schema(name.clone()))),
                Node::Relation {
                    profile,
                    schema,
                    name,
                    ..
                } => Some((*profile, Scope::Relation(schema.clone(), name.clone()))),
                _ => None,
            });
        let target = selected.or_else(|| {
            self.active_profile()
                .map(|profile| (profile, Scope::Connection))
        });
        if let Some((profile, scope)) = target {
            self.refresh_catalog(profile, scope, cx);
        }
    }

    pub(super) fn new_conversation(
        &mut self,
        _: &NewConversation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.assistant.enabled || self.dialog_open() {
            return;
        }
        if !self.assistant_state.open {
            self.toggle_assistant(window, cx);
        }
        self.create_assistant_conversation(window, cx);
    }

    pub(super) fn focus_editor(
        &mut self,
        _: &FocusEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_in_view(cx) {
            return;
        }
        self.tabs[self.active]
            .input
            .update(cx, |editor, cx| editor.focus(window, cx));
    }

    pub(super) fn focus_results(
        &mut self,
        _: &FocusResults,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_in_view(cx) {
            return;
        }
        self.restore_results(cx);
        let tab = &mut self.tabs[self.active];
        tab.panel.user_select(Panel::Results);
        let focus = tab.shown_result().table.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    /// Shows the connection tree if it is out of view, then focuses it.
    pub(super) fn focus_catalog(
        &mut self,
        _: &FocusCatalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || self.activity.read(cx).is_open() {
            return;
        }
        if !self.sidebar
            || self.sidebar_panel != SidebarPanel::Connections
            || self.assistant_covers_workspace(cx)
        {
            self.show_sidebar_panel(SidebarPanel::Connections, window, cx);
        }
        let focus = self.catalog.state.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn check_for_updates_now(
        &mut self,
        _: &CheckForUpdates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.check_for_updates(true, window, cx);
    }
}
