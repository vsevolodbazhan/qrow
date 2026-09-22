use super::*;
use gpui_kit::component::{
    alert::Alert,
    form::{field, v_form},
    v_flex,
};
use qrow::workspaces::{self, Catalog};

#[derive(Clone, PartialEq, Eq, Action, serde::Deserialize)]
#[action(namespace = qrow, no_json)]
pub(super) struct SelectWorkspace(pub Uuid);

#[derive(Clone, PartialEq, Eq, Action, serde::Deserialize)]
#[action(namespace = qrow, no_json)]
struct UnavailableWorkspace;

fn workspace_command(
    name: impl Into<SharedString>,
    action: impl Action,
    unavailable: bool,
) -> MenuItem {
    // macOS validates actions again when opening a menu, overriding `disabled`.
    // An unhandled action keeps unavailable commands disabled during validation.
    if unavailable {
        MenuItem::action(name, UnavailableWorkspace).disabled(true)
    } else {
        MenuItem::action(name, action)
    }
}

fn workspace_menu(catalog: &Catalog, blocked: bool) -> Menu {
    Menu::new("Workspaces").items(
        catalog
            .entries()
            .iter()
            .map(|entry| {
                let current = entry.id == catalog.active().id;
                workspace_command(
                    entry.name.clone(),
                    SelectWorkspace(entry.id),
                    blocked || current,
                )
                .checked(current)
            })
            .chain([
                MenuItem::separator(),
                workspace_command("New Workspace…", NewWorkspace, blocked),
            ]),
    )
}

pub(super) struct WorkspaceForm {
    name: Entity<InputState>,
    error: Option<String>,
}
pub(super) type PendingWorkspace = mpsc::Receiver<Result<(Catalog, Workspace, Saver), String>>;

impl Qrow {
    pub(super) fn refresh_workspace_menu(&mut self, cx: &App) {
        let blocked = self.demo
            || self.saver.is_none()
            || self.dialog_open()
            || self.pending_quit.is_some()
            || self.pending_workspace.is_some()
            || self.tabs.iter().any(|tab| tab.busy);
        // Rebuild native menus only when their selection, entries, or availability changes.
        let state = (
            self.catalog.active().id,
            self.catalog.entries().len(),
            blocked,
        );
        if self.workspace_menu_state != Some(state) {
            self.workspace_menu_state = Some(state);
            set_app_menus(cx, workspace_menu(&self.catalog, blocked));
        }
    }

    pub(super) fn select_workspace(
        &mut self,
        action: &SelectWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.0 == self.catalog.active().id
            || self.demo
            || self.saver.is_none()
            || self.dialog_open()
            || window.has_active_dialog(cx)
            || self.pending_quit.is_some()
            || self.pending_workspace.is_some()
            || self.tabs.iter().any(|tab| tab.busy)
        {
            return;
        }
        // The same modal guard protects edits while the background save completes.
        // Selection is already known, so the user does not need another confirmation.
        self.open_workspaces(&OpenWorkspaces, window, cx);
        self.switch_workspace(Some(action.0), cx);
    }

    pub(super) fn open_workspaces(
        &mut self,
        _: &OpenWorkspaces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.demo || self.dialog_open() || self.pending_quit.is_some() {
            return;
        }
        self.workspace_form = Some(WorkspaceForm {
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name")),
            error: None,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let create = weak.clone();
            let content = weak.update(cx, |this, cx| this.workspace_content(cx)).ok();
            let busy = weak
                .read_with(cx, |this, _| this.pending_workspace.is_some())
                .unwrap_or(false);
            dialog
                .title("Workspaces")
                .w(super::setting_row::Rows::dialog_width(window, 32.))
                .overlay_closable(false)
                .keyboard(!busy)
                .close_button(!busy)
                .on_ok(move |_, _, cx| {
                    let _ = create.update(cx, |this, cx| this.switch_workspace(None, cx));
                    false
                })
                .children(content)
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.workspace_form = None;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    pub(super) fn new_workspace(
        &mut self,
        _: &NewWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspaces(&OpenWorkspaces, window, cx);
        if let Some(form) = &self.workspace_form {
            form.name.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    fn workspace_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.workspace_form else {
            return div().into_any_element();
        };
        let busy = self.pending_workspace.is_some();
        let blocked = busy || self.saver.is_none() || self.tabs.iter().any(|t| t.busy);
        v_flex().gap_4()
            .child(div().text_color(cx.theme().muted_foreground)
                .child("Switching saves your SQL and closes the current workspace's sessions and results."))
            .child(v_flex().id("workspace-list").max_h_48().overflow_y_scroll().gap_1()
                .children(self.catalog.entries().iter().map(|entry| {
                    let id = entry.id;
                    let active = id == self.catalog.active().id;
                    Button::new(SharedString::from(format!("workspace-{id}")))
                        .label(if active { format!("{} · Current", entry.name) } else { entry.name.clone() })
                        .accessibility_label(format!("{} workspace {}", if active { "Current" } else { "Open" }, entry.name))
                        .disabled(blocked || active)
                        .on_click(cx.listener(move |this, _, _, cx| this.switch_workspace(Some(id), cx)))
                })))
            .child(v_form().child(field().label("New workspace")
                .child(Input::new(&form.name).aria_label("Workspace name").disabled(busy))))
            .child(Button::new("create-workspace").primary().label("Create workspace")
                .disabled(blocked)
                .on_click(cx.listener(|this, _, _, cx| this.switch_workspace(None, cx))))
            .when(busy, |el| el.child(div().id("workspace-switch-status").role(Role::Status).aria_label("Saving and opening workspace…").child("Saving and opening workspace…")))
            .when(!busy && self.tabs.iter().any(|t| t.busy), |el| el.child("Wait for running queries to finish before switching workspaces."))
            .when(self.saver.is_none(), |el| el.child("Workspace saving is disabled. Resolve the load error and restart Qrow before switching."))
            .when_some(form.error.clone(), |el, error| el.child(div().id("workspace-switch-error").role(Role::Alert).aria_label(error.clone()).child(Alert::error("workspace-error", error))))
            .into_any_element()
    }

    fn switch_workspace(&mut self, target: Option<Uuid>, cx: &mut Context<Self>) {
        if self.pending_workspace.is_some()
            || self.pending_quit.is_some()
            || self.tabs.iter().any(|t| t.busy)
        {
            return;
        }
        let Some(form) = &mut self.workspace_form else {
            return;
        };
        let name = target
            .is_none()
            .then(|| form.name.read(cx).value().to_string());
        form.error = None;
        let Some(saver) = &self.saver else {
            return;
        };
        let receipt = match saver.flush(self.snapshot(cx)) {
            Ok(receipt) => receipt,
            Err(error) => {
                self.workspace_form.as_mut().unwrap().error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let path = storage::workspace_path();
        let root = path.parent().expect("workspace directory").to_owned();
        let wake = self.wake.clone();
        let (tx, rx) = mpsc::channel();
        self.pending_workspace = Some(rx);
        self.dirty = None;
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<_> {
                receipt
                    .recv()
                    .map_err(|_| anyhow::anyhow!("Workspace saver stopped"))?
                    .map_err(anyhow::Error::msg)?;
                let save_wake = wake.clone();
                workspaces::open(&root, target, name.as_deref(), move || {
                    let _ = save_wake.try_send(());
                })
            })()
            .map_err(|e| format!("Could not change workspace: {e:#}"));
            let _ = tx.send(result);
            let _ = wake.try_send(());
        });
        let _ = self.wake.try_send(());
        cx.notify();
    }

    pub(super) fn poll_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let result = self
            .pending_workspace
            .as_ref()
            .and_then(|rx| match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Workspace operation stopped. Try again.".into()))
                }
            });
        let Some(result) = result else {
            return false;
        };
        self.pending_workspace = None;
        match result {
            Err(error) => {
                if let Some(form) = &mut self.workspace_form {
                    form.error = Some(error);
                }
            }
            Ok((catalog, mut workspace, saver)) => {
                let mut message = None;
                let unavailable_font = prepare_workspace(&mut workspace, &self.fonts, &mut message);
                let old_scale = self.settings.ui_scale;
                for tab in &self.tabs {
                    if let Some(worker) = &tab.worker {
                        worker.shutdown();
                    }
                }
                if let Some(mut previous) = self.saver.take() {
                    let _ = previous.stop();
                }
                self.catalog = catalog;
                self.saver = Some(saver);
                self.settings = workspace.settings;
                self.profiles = workspace.profiles;
                self.active_tabs = workspace.active_tabs;
                self.active = workspace.active_tab;
                self.tabs.clear();
                for saved in workspace.tabs {
                    let tab = self.make_tab(saved, window, cx);
                    self.tabs.push(tab);
                }
                self.message = message;
                self.dirty = unavailable_font.then(Instant::now);
                self.sidebar_width = self.sidebar_width / old_scale * self.settings.ui_scale;
                self.editor_height = self.editor_height / old_scale * self.settings.ui_scale;
                self.resize = None;
                self.menu = None;
                self.workspace_form = None;
                apply_ui_theme(&self.settings, window, cx);
                window.close_dialog(cx);
                self.tabs[self.active]
                    .input
                    .update(cx, |input, cx| input.focus(window, cx));
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Catalog, MenuItem, UnavailableWorkspace, workspace_menu};

    #[test]
    fn native_menu_marks_current_workspace_and_disables_unavailable_commands() {
        let catalog = Catalog::default();
        let menu = workspace_menu(&catalog, false);
        assert_eq!(menu.name.as_ref(), "Workspaces");
        let MenuItem::Action {
            name,
            action,
            checked,
            disabled,
            ..
        } = &menu.items[0]
        else {
            panic!("workspace action expected")
        };
        assert_eq!(name.as_ref(), "Default");
        assert!(*checked && *disabled);
        assert!(action.partial_eq(&UnavailableWorkspace));
        assert!(!menu.items[2].is_disabled());
        let blocked = workspace_menu(&catalog, true);
        assert!(blocked.items[0].is_disabled());
        assert!(blocked.items[2].is_disabled());
    }
}
