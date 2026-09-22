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

fn selection_menu(catalog: &Catalog, blocked: bool) -> Menu {
    Menu::new("Select Workspace")
        .disabled(blocked || catalog.entries().is_empty())
        .items(catalog.entries().iter().map(|entry| {
            let current = catalog.active().is_some_and(|active| entry.id == active.id);
            workspace_command(
                entry.name.clone(),
                SelectWorkspace(entry.id),
                blocked || current,
            )
            .checked(current)
        }))
}

fn workspace_menu(catalog: &Catalog, blocked: bool) -> Menu {
    Menu::new("Workspaces").items([
        workspace_command("New Workspace…", NewWorkspace, blocked),
        MenuItem::submenu(selection_menu(catalog, blocked)),
        workspace_command(
            "Rename Workspace…",
            RenameWorkspace,
            blocked || catalog.active().is_none(),
        ),
    ])
}

#[derive(Clone, Copy)]
enum Operation {
    Create,
    Rename(Uuid),
    Select(Uuid),
}

pub(super) struct WorkspaceForm {
    name: Entity<InputState>,
    operation: Operation,
    error: Option<String>,
}
pub(super) enum WorkspaceResult {
    Opened(Catalog, Box<Workspace>, Saver),
    Renamed(Catalog),
}
pub(super) type PendingWorkspace = mpsc::Receiver<Result<WorkspaceResult, String>>;

impl Qrow {
    fn workspace_commands_blocked(&self) -> bool {
        self.demo
            || self.saver.is_none() && self.message.is_some()
            || self.dialog_open()
            || self.pending_quit.is_some()
            || self.pending_workspace.is_some()
            || self.tabs.iter().any(|tab| tab.busy)
    }

    pub(super) fn refresh_workspace_menu(&mut self, cx: &App) {
        let blocked = self.workspace_commands_blocked();
        let state = (
            self.catalog.active().map(|e| e.id),
            self.catalog.entries().to_vec(),
            blocked,
        );
        if self.workspace_menu_state.as_ref() != Some(&state) {
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
        if self.workspace_commands_blocked()
            || window.has_active_dialog(cx)
            || self
                .catalog
                .active()
                .is_some_and(|entry| entry.id == action.0)
        {
            return;
        }
        self.workspace_dialog(Operation::Select(action.0), window, cx);
        self.submit_workspace(cx);
    }

    pub(super) fn open_workspaces(
        &mut self,
        _: &OpenWorkspaces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_commands_blocked() {
            return;
        }
        let mut menu = gpui_kit::component::native_menu::NativeMenu::new();
        for entry in self.catalog.entries() {
            menu = menu.menu_with_check(
                entry.name.clone(),
                self.catalog
                    .active()
                    .is_some_and(|active| active.id == entry.id),
                Box::new(SelectWorkspace(entry.id)),
            );
        }
        menu.show(window.mouse_position(), window, cx);
    }

    pub(super) fn new_workspace(
        &mut self,
        _: &NewWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_commands_blocked() || window.has_active_dialog(cx) {
            return;
        }
        self.workspace_dialog(Operation::Create, window, cx);
    }

    pub(super) fn rename_workspace(
        &mut self,
        _: &RenameWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_commands_blocked() || window.has_active_dialog(cx) {
            return;
        }
        if let Some(entry) = self.catalog.active() {
            self.workspace_dialog(Operation::Rename(entry.id), window, cx);
        }
    }

    fn workspace_dialog(
        &mut self,
        operation: Operation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
        if matches!(operation, Operation::Rename(_))
            && let Some(entry) = self.catalog.active()
        {
            name.update(cx, |input, cx| {
                input.set_value(entry.name.clone(), window, cx)
            });
        }
        self.workspace_form = Some(WorkspaceForm {
            name: name.clone(),
            operation,
            error: None,
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let submit = weak.clone();
            let content = weak.update(cx, |this, cx| this.workspace_content(cx)).ok();
            let busy = weak
                .read_with(cx, |this, _| this.pending_workspace.is_some())
                .unwrap_or(false);
            dialog
                .title(match operation {
                    Operation::Create => "New Workspace",
                    Operation::Rename(_) => "Rename Workspace",
                    Operation::Select(_) => "Select Workspace",
                })
                .w(super::setting_row::Rows::dialog_width(window, 28.))
                .overlay_closable(false)
                .keyboard(!busy)
                .close_button(!busy)
                .on_ok(move |_, _, cx| {
                    let _ = submit.update(cx, |this, cx| this.submit_workspace(cx));
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
        if !matches!(operation, Operation::Select(_)) {
            name.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn workspace_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.workspace_form else {
            return div().into_any_element();
        };
        let busy = self.pending_workspace.is_some();
        v_flex()
            .gap_4()
            .when(!matches!(form.operation, Operation::Select(_)), |el| {
                el.child(
                    v_form().child(
                        field().label("Workspace name").child(
                            Input::new(&form.name)
                                .aria_label("Workspace name")
                                .disabled(busy),
                        ),
                    ),
                )
            })
            .when(!busy, |el| {
                el.child(
                    Button::new("submit-workspace")
                        .primary()
                        .label(match form.operation {
                            Operation::Create => "Create workspace",
                            Operation::Rename(_) => "Rename workspace",
                            Operation::Select(_) => "Retry",
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.submit_workspace(cx))),
                )
            })
            .when(busy, |el| {
                el.child(
                    div()
                        .id("workspace-operation-status")
                        .role(Role::Status)
                        .aria_label("Saving workspace…")
                        .child("Saving workspace…"),
                )
            })
            .when_some(form.error.clone(), |el, error| {
                el.child(
                    div()
                        .id("workspace-switch-error")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .child(Alert::error("workspace-error", error)),
                )
            })
            .into_any_element()
    }

    fn submit_workspace(&mut self, cx: &mut Context<Self>) {
        if self.pending_workspace.is_some()
            || self.pending_quit.is_some()
            || self.tabs.iter().any(|t| t.busy)
        {
            return;
        }
        let Some(form) = &mut self.workspace_form else {
            return;
        };
        let operation = form.operation;
        let name = form.name.read(cx).value().to_string();
        form.error = None;
        let receipt = if matches!(operation, Operation::Rename(_)) {
            None
        } else {
            match self
                .saver
                .as_ref()
                .map(|saver| saver.flush(self.snapshot(cx)))
                .transpose()
            {
                Ok(receipt) => receipt,
                Err(error) => {
                    self.workspace_form.as_mut().unwrap().error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            }
        };
        let path = storage::workspace_path();
        let root = path.parent().expect("workspace directory").to_owned();
        let wake = self.wake.clone();
        let (tx, rx) = mpsc::channel();
        self.pending_workspace = Some(rx);
        if !matches!(operation, Operation::Rename(_)) {
            self.dirty = None;
        }
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<_> {
                if let Some(receipt) = receipt {
                    receipt
                        .recv()
                        .map_err(|_| anyhow::anyhow!("Workspace saver stopped"))?
                        .map_err(anyhow::Error::msg)?;
                }
                if let Operation::Rename(id) = operation {
                    return workspaces::rename(&root, id, &name).map(WorkspaceResult::Renamed);
                }
                let save_wake = wake.clone();
                let target = if let Operation::Select(id) = operation {
                    Some(id)
                } else {
                    None
                };
                let name = matches!(operation, Operation::Create).then_some(name.as_str());
                workspaces::open(&root, target, name, move || {
                    let _ = save_wake.try_send(());
                })
                .map(|(catalog, workspace, saver)| {
                    WorkspaceResult::Opened(catalog, Box::new(workspace), saver)
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
            Ok(WorkspaceResult::Renamed(catalog)) => {
                self.catalog = catalog;
                self.workspace_form = None;
                window.close_dialog(cx);
            }
            Ok(WorkspaceResult::Opened(catalog, workspace, saver)) => {
                let mut workspace = *workspace;
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
    use super::{Catalog, MenuItem, UnavailableWorkspace, Uuid, workspace_menu};

    #[test]
    fn native_menu_separates_creation_selection_and_rename() {
        let empty = workspace_menu(&Catalog::default(), false);
        assert!(!empty.items[0].is_disabled());
        assert!(empty.items[1].is_disabled());
        assert!(empty.items[2].is_disabled());
        let id = Uuid::new_v4();
        let catalog: Catalog = serde_json::from_value(serde_json::json!({
            "version": 1, "active": id, "entries": [{"id": id, "name": "Analytics"}]
        }))
        .unwrap();
        let menu = workspace_menu(&catalog, false);
        assert!(!menu.items[0].is_disabled());
        assert!(!menu.items[2].is_disabled());
        let MenuItem::Submenu(selection) = &menu.items[1] else {
            panic!("native submenu expected")
        };
        assert_eq!(selection.name.as_ref(), "Select Workspace");
        let MenuItem::Action {
            checked,
            disabled,
            action,
            ..
        } = &selection.items[0]
        else {
            panic!("workspace action expected")
        };
        assert!(*checked && *disabled);
        assert!(action.partial_eq(&UnavailableWorkspace));
        let blocked = workspace_menu(&catalog, true);
        assert!(blocked.items.iter().all(MenuItem::is_disabled));
    }
}
