//! Multi-workspace management: switching workspaces in the single main window,
//! creating, renaming, duplicating, revealing, and deleting named workspaces.

use super::setting_row::Rows;
use super::*;
use crate::model::{
    MAX_WORKSPACE_NAME, validate_workspace_name,
    workspace_export::{ExportOptions, MAX_EXPORTED_WORKSPACE_BYTES},
};
use gpui_kit::Anchor;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    Icon,
    alert::Alert,
    checkbox::Checkbox,
    form::{field, v_form},
    h_flex,
    menu::DropdownMenu as _,
    scroll::ScrollableElement as _,
    tag::Tag,
    v_flex,
};
use std::io::Read as _;

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

pub(super) struct ExportWorkspaceForm {
    pub(super) target: Uuid,
    pub(super) name: String,
    pub(super) options: ExportOptions,
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

    pub(super) fn import_workspace_action(
        &mut self,
        _: &ImportWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.is_none() || self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        self.prompt_import_workspace(false, window, cx);
    }

    pub(super) fn export_workspace_action(
        &mut self,
        _: &ExportWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let Some(id) = self
            .store
            .as_ref()
            .map(storage::DataStore::active_workspace_id)
        else {
            return;
        };
        self.open_export_workspace_dialog(id, window, cx);
    }

    pub(super) fn backup_workspace_action(
        &mut self,
        _: &BackupWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let Some(id) = self
            .store
            .as_ref()
            .map(storage::DataStore::active_workspace_id)
        else {
            return;
        };
        self.prompt_backup_workspace(id, window, cx);
    }

    fn safe_workspace_filename(name: &str, suffix: &str) -> String {
        let cleaned: String = name
            .trim()
            .chars()
            .map(|ch| {
                if ch.is_control()
                    || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                {
                    '_'
                } else {
                    ch
                }
            })
            .collect();
        let base = cleaned.trim();
        let base = if base.is_empty() { "workspace" } else { base };
        format!("{base}{suffix}")
    }

    /// Opens the Export Workspace review dialog so the user can choose which
    /// potentially sensitive plain-text fields to include before saving.
    fn open_export_workspace_dialog(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(entry) = store.workspace_entry(id) else {
            return;
        };
        self.export_workspace_form = Some(ExportWorkspaceForm {
            target: id,
            name: entry.name.clone(),
            options: ExportOptions::default(),
        });
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let content = weak
                .update(cx, |this, cx| this.export_workspace_content(cx))
                .ok();
            let footer = weak
                .update(cx, |this, cx| this.export_workspace_footer(cx))
                .ok();
            dialog
                .title("Export Workspace")
                .w(Rows::dialog_width(window, NAME_DIALOG_REMS))
                .overlay_closable(true)
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, window, cx| {
                    let close = close.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.export_workspace_form = None;
                            if this.workspace_manager_open {
                                this.workspace_manager_focus.focus(window, cx);
                            }
                            cx.notify();
                        });
                    });
                })
        });
        cx.notify();
    }

    fn export_workspace_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.export_workspace_form else {
            return div().into_any_element();
        };
        let options = form.options;
        let summary = format!(
            "Export \"{}\" as a portable .qrow-workspace.json file. Passwords, sign-in tokens, account identities, dbt manifest paths, custom keep-alive SQL, settings, and assistant transcripts are never exported.",
            form.name
        );
        v_flex()
            .id("export-workspace-dialog")
            .test_support()
            .w_full()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(summary),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Checkbox::new("export-workspace-include-sql")
                                    .small()
                                    .label("Include saved SQL")
                                    .checked(options.include_sql)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if let Some(form) = &mut this.export_workspace_form {
                                            form.options.include_sql = *checked;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .pl_5()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Saved SQL is exported as plain text."),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Checkbox::new("export-workspace-include-parameters")
                                    .small()
                                    .label("Include session parameters")
                                    .checked(options.include_parameters)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if let Some(form) = &mut this.export_workspace_form {
                                            form.options.include_parameters = *checked;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .pl_5()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Session parameters can contain secrets."),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Checkbox::new("export-workspace-include-notes")
                                    .small()
                                    .label("Include assistant notes")
                                    .checked(options.include_assistant_notes)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if let Some(form) = &mut this.export_workspace_form {
                                            form.options.include_assistant_notes = *checked;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .pl_5()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Assistant notes are exported as plain text."),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn export_workspace_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .w_full()
            .justify_end()
            .gap_2()
            .child(
                Button::new("export-workspace-cancel")
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.export_workspace_form = None;
                        window.close_dialog(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("export-workspace-confirm")
                    .primary()
                    .label("Export…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let Some(form) = this.export_workspace_form.take() else {
                            return;
                        };
                        window.close_dialog(cx);
                        this.prompt_export_workspace(form.target, form.options, window, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn prompt_export_workspace(
        &mut self,
        id: Uuid,
        options: ExportOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(entry) = store.workspace_entry(id) else {
            return;
        };
        let name = entry.name.clone();
        let filename = Self::safe_workspace_filename(&name, ".qrow-workspace.json");
        let directory = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
        let prompt = cx.prompt_for_new_path(&directory, Some(&filename));
        cx.spawn_in(window, async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                error => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.report_failure(
                            toast::Failure {
                                id: "workspace-export-error",
                                title: "Could not open the save panel",
                                file: None,
                                log: crate::activity::APPLICATION,
                                source: WORKSPACES_SOURCE,
                                error: format!("{error:?}"),
                            },
                            window,
                            cx,
                        );
                    });
                    return;
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.export_workspace_to_path_with_options(id, path, options, window, cx);
            });
        })
        .detach();
    }

    /// Exports workspace `id` as a portable `.qrow-workspace.json` bundle to
    /// `path` using safe default [`ExportOptions`].
    pub fn export_workspace_to_path(
        &mut self,
        id: Uuid,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_workspace_to_path_with_options(id, path, ExportOptions::default(), window, cx);
    }

    /// Exports workspace `id` as a portable `.qrow-workspace.json` bundle to
    /// `path` with explicit `options`.
    pub fn export_workspace_to_path_with_options(
        &mut self,
        id: Uuid,
        path: std::path::PathBuf,
        options: ExportOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(entry) = store.workspace_entry(id) else {
            return;
        };
        let name = entry.name.clone();
        let live_snapshot = (store.active_workspace_id() == id).then(|| self.snapshot(cx));
        let source_path = store.workspace_path(id);
        let write_path = path.clone();
        let export_name = name.clone();
        let result = cx.background_spawn(async move {
            let workspace = match live_snapshot {
                Some(ws) => ws,
                None => storage::load_existing(&source_path)?,
            };
            let json = crate::model::workspace_export::export_workspace_with_options(
                &export_name,
                &workspace,
                options,
            )?;
            if let Some(parent) = write_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            storage::write_atomically(&write_path, json.as_bytes())
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = result.await;
            let _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(()) => {
                    window.push_notification(
                        toast::saved_file(
                            toast::SavedFileIds {
                                notice: "workspace-exported-toast",
                                title: "workspace-exported-title",
                                filename: "workspace-exported-filename",
                                reveal: "workspace-exported-reveal",
                            },
                            format!("Exported workspace \"{name}\""),
                            path,
                        ),
                        cx,
                    );
                }
                Err(error) => {
                    this.report_failure(
                        toast::Failure {
                            id: "workspace-export-error",
                            title: "Could not export workspace",
                            file: Some(toast::file_name(&path)),
                            log: crate::activity::APPLICATION,
                            source: WORKSPACES_SOURCE,
                            error: format!("{error:#}"),
                        },
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn prompt_backup_workspace(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(entry) = store.workspace_entry(id) else {
            return;
        };
        let name = entry.name.clone();
        let filename = Self::safe_workspace_filename(&name, "-backup.json");
        let directory = store.backups_dir();
        let _ = std::fs::create_dir_all(&directory);
        let prompt = cx.prompt_for_new_path(&directory, Some(&filename));
        cx.spawn_in(window, async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                error => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.report_failure(
                            toast::Failure {
                                id: "workspace-backup-error",
                                title: "Could not open the save panel",
                                file: None,
                                log: crate::activity::APPLICATION,
                                source: WORKSPACES_SOURCE,
                                error: format!("{error:?}"),
                            },
                            window,
                            cx,
                        );
                    });
                    return;
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.backup_workspace_to_path(id, path, window, cx);
            });
        })
        .detach();
    }

    /// Writes a full JSON backup of workspace `id` to `path`, preserving any
    /// unknown fields and newer-version metadata.
    pub fn backup_workspace_to_path(
        &mut self,
        id: Uuid,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(entry) = store.workspace_entry(id) else {
            return;
        };
        let name = entry.name.clone();
        let is_active = store.active_workspace_id() == id;
        let source_path = store.workspace_path(id);
        let write_path = path.clone();
        let active_receipt = if is_active {
            let snapshot = self.snapshot(cx);
            self.saver
                .as_ref()
                .map(|saver| saver.backup(snapshot.clone(), write_path.clone()))
                .transpose()
        } else {
            Ok(None)
        };
        let active_receipt = match active_receipt {
            Ok(receipt) => receipt,
            Err(error) => {
                self.report_failure(
                    toast::Failure {
                        id: "workspace-backup-error",
                        title: "Could not back up workspace",
                        file: Some(toast::file_name(&path)),
                        log: crate::activity::APPLICATION,
                        source: WORKSPACES_SOURCE,
                        error: format!("{error:#}"),
                    },
                    window,
                    cx,
                );
                return;
            }
        };
        let fallback_snapshot = (is_active && active_receipt.is_none()).then(|| self.snapshot(cx));
        let result = cx.background_spawn(async move {
            if let Some(receipt) = active_receipt {
                receipt
                    .recv()
                    .map_err(|_| anyhow::anyhow!("Workspace saver stopped before backup finished"))?
                    .map_err(anyhow::Error::msg)
            } else if let Some(snapshot) = fallback_snapshot {
                storage::store::write_workspace_backup(&snapshot, &write_path)
            } else {
                storage::store::backup_workspace_file(&source_path, &write_path)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = result.await;
            let _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(()) => {
                    window.push_notification(
                        toast::saved_file(
                            toast::SavedFileIds {
                                notice: "workspace-backup-toast",
                                title: "workspace-backup-title",
                                filename: "workspace-backup-filename",
                                reveal: "workspace-backup-reveal",
                            },
                            format!("Backed up workspace \"{name}\""),
                            path,
                        ),
                        cx,
                    );
                }
                Err(error) => {
                    this.report_failure(
                        toast::Failure {
                            id: "workspace-backup-error",
                            title: "Could not back up workspace",
                            file: Some(toast::file_name(&path)),
                            log: crate::activity::APPLICATION,
                            source: WORKSPACES_SOURCE,
                            error: format!("{error:#}"),
                        },
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn prompt_import_workspace(
        &mut self,
        keep_manager_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match prompt.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return,
                error => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.report_failure(
                            toast::Failure {
                                id: "workspace-import-error",
                                title: "Could not open the file panel",
                                file: None,
                                log: crate::activity::APPLICATION,
                                source: WORKSPACES_SOURCE,
                                error: format!("{error:?}"),
                            },
                            window,
                            cx,
                        );
                    });
                    return;
                }
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.import_workspace_from_path(path, keep_manager_open, window, cx);
            });
        })
        .detach();
    }

    /// Reads a portable `.qrow-workspace.json` file from `path`, registers it
    /// as a new workspace, and either switches into it or refreshes the open
    /// Manage Workspaces dialog.
    pub fn import_workspace_from_path(
        &mut self,
        path: std::path::PathBuf,
        keep_manager_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(data_dir) = self.store.as_ref().map(|store| store.data_dir().to_owned()) else {
            return;
        };
        let cleanup_data_dir = data_dir.clone();
        let read_path = path.clone();
        let staged = cx.background_spawn(async move {
            let file = std::fs::File::open(&read_path)?;
            let mut bytes = Vec::new();
            file.take((MAX_EXPORTED_WORKSPACE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= MAX_EXPORTED_WORKSPACE_BYTES,
                "The workspace file is too large to import."
            );
            let text = String::from_utf8(bytes)
                .map_err(|_| anyhow::anyhow!("The selected file is not valid UTF-8 text."))?;
            let imported = crate::model::workspace_export::import_workspace(&text)?;
            let counts = (
                imported.workspace.profiles.len(),
                imported.workspace.tabs.len(),
            );
            let id =
                storage::store::prepare_imported_workspace_file(&data_dir, &imported.workspace)?;
            Ok::<_, anyhow::Error>((id, imported.name, counts))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = staged.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let (id, name, counts) = match result {
                    Ok(staged) => staged,
                    Err(error) => {
                        this.report_failure(
                            toast::Failure {
                                id: "workspace-import-error",
                                title: "Could not import workspace",
                                file: Some(toast::file_name(&path)),
                                log: crate::activity::APPLICATION,
                                source: WORKSPACES_SOURCE,
                                error: format!("{error:#}"),
                            },
                            window,
                            cx,
                        );
                        return;
                    }
                };
                let (entry, sync_error) = match this
                    .store
                    .as_mut()
                    .map(|store| store.commit_imported_workspace(id, &name))
                {
                    Some(Ok(outcome)) => outcome,
                    Some(Err(error)) => {
                        this.report_failure(
                            toast::Failure {
                                id: "workspace-import-error",
                                title: "Could not import workspace",
                                file: Some(toast::file_name(&path)),
                                log: crate::activity::APPLICATION,
                                source: WORKSPACES_SOURCE,
                                error: format!("{error:#}"),
                            },
                            window,
                            cx,
                        );
                        return;
                    }
                    None => {
                        let _ = std::fs::remove_dir_all(storage::workspace_directory(
                            &cleanup_data_dir,
                            id,
                        ));
                        return;
                    }
                };
                if let Some(error) = sync_error {
                    let activity_entry = crate::activity::ActivityEntry::new(
                        Severity::Error,
                        format!(
                            "Imported workspace \"{}\", but syncing registry directory failed: {error:#}",
                            entry.name
                        ),
                    )
                    .with_source(WORKSPACES_SOURCE);
                    this.record_activity(crate::activity::APPLICATION, activity_entry, cx);
                }
                this.workspace_summaries.insert(entry.id, counts);
                this.sync_workspace_menus(cx);
                if keep_manager_open && this.workspace_manager_open {
                    this.select_workspace_in_manager(entry.id);
                    cx.notify();
                } else {
                    this.request_workspace_switch(SwitchAction::Switch(entry.id), window, cx);
                }
            });
        })
        .detach();
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

    fn switch_active_work_description(&self, cx: &App) -> Option<String> {
        let mut active: Vec<&'static str> = Vec::new();
        if cx.global::<crate::export::Jobs>().active_count() > 0 {
            active.push("an export");
        }
        if self.assistant_working() {
            active.push("an assistant turn");
        }
        if self.tabs.iter().any(|tab| tab.busy) {
            active.push("a query");
        }
        if self.sign_ins_working() {
            active.push("a sign-in");
        }
        if self
            .profiles
            .iter()
            .any(|profile| self.catalog.is_refreshing(profile.id))
        {
            active.push("a metadata refresh");
        }
        match active.as_slice() {
            [] => None,
            [one] => {
                let mut first = one.chars();
                let capitalized = match first.next() {
                    Some(c) => format!("{}{}", c.to_uppercase(), first.as_str()),
                    None => String::new(),
                };
                Some(format!(
                    "{capitalized} is still running. Switch workspaces now to stop it?"
                ))
            }
            [first, second] => {
                let mut chars = first.chars();
                let capitalized = match chars.next() {
                    Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
                    None => String::new(),
                };
                Some(format!(
                    "{capitalized} and {second} are still running. Switch workspaces now to stop them?"
                ))
            }
            many => {
                let last = many.last().unwrap();
                let head = many[..many.len() - 1].join(", ");
                let mut chars = head.chars();
                let capitalized = match chars.next() {
                    Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
                    None => String::new(),
                };
                Some(format!(
                    "{capitalized}, and {last} are still running. Switch workspaces now to stop them?"
                ))
            }
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
        description: String,
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
                .description(description.clone())
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

        // 2. Open and validate the target workspace BEFORE shutting down any
        //    outgoing workers or releasing the outgoing lock. If the target
        //    cannot be opened (missing, corrupt, locked, or newer version),
        //    the current workspace stays completely intact and usable.
        let wake = self.wake.clone();
        let open_result = self.store.as_ref().map(|store| {
            store.open_workspace_saver(target_id, move || {
                let _ = wake.try_send(());
            })
        });
        let (workspace, mut new_saver) = match open_result {
            Some(Ok(opened)) => opened,
            Some(Err(error)) => {
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

        // 3. Cancel active exports and wait for writers to finish before
        //    committing the switch. Invalidate any in-flight clipboard copy so
        //    it cannot write to the clipboard or open a dialog after switching.
        export_dialog::invalidate_copies(cx);
        if !cx
            .global::<crate::export::Jobs>()
            .cancel_current_and_wait(Duration::from_secs(2))
        {
            let _ = new_saver.stop();
            self.report_failure(
                toast::Failure {
                    id: "workspace-switch-error",
                    title: "Could not switch workspace",
                    file: None,
                    log: crate::activity::APPLICATION,
                    source: WORKSPACES_SOURCE,
                    error: "An export did not stop in time. Wait for it to finish, then try again."
                        .into(),
                },
                window,
                cx,
            );
            cx.notify();
            return;
        }

        // 4. Commit the active workspace in `workspaces.json` before tearing
        //    down the outgoing workspace or deleting anything. If the registry
        //    was not updated on disk, abort cleanly with the current workspace
        //    still intact.
        if let Some(store) = &mut self.store
            && let Err(error) = store.set_active_workspace(target_id)
        {
            if store.active_workspace_id() != target_id {
                let _ = new_saver.stop();
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
                cx.notify();
                return;
            }
            let entry = crate::activity::ActivityEntry::new(
                Severity::Error,
                format!(
                    "Active workspace updated, but syncing registry directory failed: {error:#}"
                ),
            )
            .with_source(WORKSPACES_SOURCE);
            self.record_activity(crate::activity::APPLICATION, entry, cx);
        }

        // 5. Stop and quiesce ALL outgoing workers and background writers
        //    (exports, sign-ins, OIDC token writes, dbt index writes,
        //    transcripts, query sessions, catalog workers, and workspace lock)
        //    BEFORE deleting any workspace credentials or directories.
        self.sign_in_ui.cancel_all();
        self.assistant_state.save_transcripts = None;
        self.save_transcripts(None);
        if let Some(store) = self.assistant_state.store.take() {
            let _ = store.flush(Duration::from_secs(2));
        }
        self.assistant_state.shutdown();
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.shutdown();
            }
        }
        self.catalog.shutdown();
        self.dbt.shutdown();
        let deadline = Instant::now() + Duration::from_secs(1);
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.wait_for_shutdown(deadline.saturating_duration_since(Instant::now()));
            }
        }
        self.catalog.wait_for_shutdown(deadline);
        self.oidc
            .configure(&workspace.sign_ins, &workspace.profiles);
        self.oidc.sync_token_writes();
        self.external_auth.configure(&workspace.profiles);

        // Release the outgoing workspace's lock.
        if let Some(mut saver) = self.saver.take() {
            let _ = saver.stop();
        }

        // 6. Optional post-switch deletion (when deleting the active workspace).
        //    All outgoing writers are now quiesced so deleted credentials and
        //    local directories cannot be recreated.
        if let Some(delete_id) = delete_after {
            self.finish_workspace_deletion(delete_id, window, cx);
        }

        // 7. Load the target workspace into the window while keeping global
        //    settings, updates, and SettingsWindow intact, and clear any
        //    workspace-specific overlays (dbt details sheet, Assistant draft,
        //    Activity panel).
        let local_dir = self
            .store
            .as_ref()
            .map(storage::DataStore::active_local_directory);
        if self.dbt_details.is_some() {
            window.close_sheet(cx);
            self.dbt_details = None;
            self.dbt_details_origin = None;
        }
        self.message = new_saver.newer.as_ref().map(storage::NewerData::notice);
        self.saver = Some(new_saver);
        self.dirty = None;
        self.apply_workspace_layout(workspace.layout, window);
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
        self.workspace_manager_selected = None;

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
        self.assistant_pane
            .update(cx, |pane, cx| pane.reset_for_workspace(window, cx));

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
                        self.workspace_manager_selected = Some(id);
                        self.reopen_manage_workspaces_deferred(window, cx);
                    }
                } else if reopen_manager {
                    self.workspace_manager_selected = target;
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

    fn manage_workspaces_selected_id(&self) -> Option<Uuid> {
        let store = self.store.as_ref()?;
        self.workspace_manager_selected
            .filter(|id| store.workspace_entry(*id).is_some())
            .or_else(|| Some(store.active_workspace_id()))
    }

    fn select_workspace_in_manager(&mut self, id: Uuid) {
        self.workspace_manager_selected = Some(id);
        if let Some(index) = self
            .store
            .as_ref()
            .and_then(|store| store.workspaces().iter().position(|entry| entry.id == id))
        {
            self.workspace_manager_scroll.scroll_to_item(index);
        }
    }

    fn step_workspace_manager_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(store) = &self.store else {
            return;
        };
        let entries = store.workspaces();
        if entries.is_empty() {
            return;
        }
        let current = self
            .manage_workspaces_selected_id()
            .and_then(|id| entries.iter().position(|entry| entry.id == id))
            .unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(entries.len() - 1)
        };
        let next_id = entries[next].id;
        self.select_workspace_in_manager(next_id);
        cx.notify();
    }

    fn switch_from_manager(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_id) = self
            .store
            .as_ref()
            .map(storage::DataStore::active_workspace_id)
        else {
            return;
        };
        if id == active_id {
            return;
        }
        self.workspace_manager_open = false;
        self.workspace_manager_selected = None;
        window.close_dialog(cx);
        self.request_workspace_switch(SwitchAction::Switch(id), window, cx);
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
        if let Some(selected) = self.manage_workspaces_selected_id() {
            self.select_workspace_in_manager(selected);
        }
        self.workspace_manager_open = true;
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let confirm = weak.clone();
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
                .on_ok(move |_, window, cx| {
                    confirm
                        .update(cx, |this, cx| {
                            let Some(selected) = this.manage_workspaces_selected_id() else {
                                return false;
                            };
                            let active_id = this
                                .store
                                .as_ref()
                                .map(storage::DataStore::active_workspace_id);
                            if Some(selected) == active_id {
                                return false;
                            }
                            this.switch_from_manager(selected, window, cx);
                            false
                        })
                        .unwrap_or(false)
                })
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let close = close.clone();
                    cx.defer(move |cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.workspace_manager_open = false;
                            if this.workspace_name_form.is_none() {
                                this.workspace_manager_selected = None;
                            }
                            cx.notify();
                        });
                    });
                })
        });
        cx.defer_in(window, |this, window, cx| {
            if this.workspace_manager_open {
                this.workspace_manager_focus.focus(window, cx);
            }
        });
        cx.notify();
    }

    fn manage_workspaces_content(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(store) = &self.store else {
            return div().into_any_element();
        };
        let active_id = store.active_workspace_id();
        let selected_id = self.manage_workspaces_selected_id();
        let can_delete = store.workspaces().len() > 1;
        let max_height = (window.rem_size() * MANAGER_MAX_HEIGHT_REMS)
            .min(window.viewport_size().height - window.rem_size() * 12.)
            .max(px(160.));
        let entries: Vec<_> = store.workspaces().to_vec();
        let weak = cx.weak_entity();
        let selected_bg = if cx.theme().is_dark() {
            cx.theme().primary.opacity(0.10)
        } else {
            cx.theme().primary.opacity(0.06)
        };

        div()
            .id("manage-workspaces-list")
            .test_support()
            .track_focus(&self.workspace_manager_focus)
            .role(Role::List)
            .aria_label("Workspaces")
            .w_full()
            .pt(px(5.5))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let m = &event.keystroke.modifiers;
                if m.control || m.alt || m.platform || m.function || m.shift {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "up" => {
                        this.step_workspace_manager_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        this.step_workspace_manager_selection(1, cx);
                        cx.stop_propagation();
                    }
                    "home" => {
                        if let Some(first) = this
                            .store
                            .as_ref()
                            .and_then(|store| store.workspaces().first())
                            .map(|entry| entry.id)
                        {
                            this.select_workspace_in_manager(first);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }
                    "end" => {
                        if let Some(last) = this
                            .store
                            .as_ref()
                            .and_then(|store| store.workspaces().last())
                            .map(|entry| entry.id)
                        {
                            this.select_workspace_in_manager(last);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }
                    "enter" => {
                        if let Some(selected) = this.manage_workspaces_selected_id()
                            && this
                                .store
                                .as_ref()
                                .is_some_and(|store| store.active_workspace_id() != selected)
                        {
                            this.switch_from_manager(selected, window, cx);
                            cx.stop_propagation();
                        }
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(
                        v_flex()
                            .id("manage-workspaces-scroll")
                            .w_full()
                            .max_h(max_height)
                            .overflow_y_scroll()
                            .track_scroll(&self.workspace_manager_scroll)
                            .gap_1p5()
                            .children(entries.into_iter().map(|entry| {
                        let id = entry.id;
                        let name = entry.name;
                        let is_active = id == active_id;
                        let is_selected = selected_id == Some(id);
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
                        let actions_name = name.clone();
                        let delete_name = name.clone();
                        let row_a11y = if is_active {
                            format!("{name}, active, {summary}")
                        } else {
                            format!("{name}, {summary}")
                        };
                        let menu_weak = weak.clone();

                        h_flex()
                            .id(row_id)
                            .test_support()
                            .role(Role::ListItem)
                            .aria_selected(is_selected)
                            .aria_label(row_a11y)
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .cursor_pointer()
                            .border_color(if is_selected {
                                cx.theme().primary.opacity(0.65)
                            } else {
                                cx.theme().border
                            })
                            .bg(if is_selected {
                                selected_bg
                            } else {
                                cx.theme().background
                            })
                            .when(!is_selected, |row| {
                                row.hover(|style| style.bg(cx.theme().secondary.opacity(0.45)))
                            })
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                if event.click_count() >= 2 && !is_active {
                                    this.switch_from_manager(id, window, cx);
                                } else {
                                    this.select_workspace_in_manager(id);
                                    this.workspace_manager_focus.focus(window, cx);
                                    cx.notify();
                                }
                            }))
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
                                                    .id(SharedString::from(format!(
                                                        "workspace-name-{id}"
                                                    )))
                                                    .min_w_0()
                                                    .truncate()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .tooltip({
                                                        let full_name = name.clone();
                                                        move |window, cx| {
                                                            gpui_kit::component::tooltip::Tooltip::new(
                                                                full_name.clone(),
                                                            )
                                                            .build(window, cx)
                                                        }
                                                    })
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
                                div()
                                    .id(SharedString::from(format!("workspace-actions-wrap-{id}")))
                                    .flex_shrink_0()
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "workspace-actions-{id}"
                                        )))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Ellipsis)
                                        .accessibility_label(format!(
                                            "Actions for {actions_name}"
                                        ))
                                        .tooltip("Workspace actions")
                                        .dropdown_menu_with_anchor(
                                            Anchor::TopRight,
                                            move |menu, _, _| {
                                                let rename_weak = menu_weak.clone();
                                                let duplicate_weak = menu_weak.clone();
                                                let export_weak = menu_weak.clone();
                                                let reveal_weak = menu_weak.clone();
                                                let delete_weak = menu_weak.clone();
                                                let delete_name = delete_name.clone();
                                                menu.item(
                                                    PopupMenuItem::new("Rename…")
                                                        .icon(Icon::new(AssetIconName::Pencil).small())
                                                        .on_click(move |_, window, cx| {
                                                            let _ = rename_weak.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.workspace_manager_open =
                                                                        false;
                                                                    window.close_dialog(cx);
                                                                    cx.defer_in(
                                                                        window,
                                                                        move |this, window, cx| {
                                                                            this.open_workspace_name_dialog(
                                                                                Some(id),
                                                                                true,
                                                                                window,
                                                                                cx,
                                                                            );
                                                                        },
                                                                    );
                                                                },
                                                            );
                                                        }),
                                                )
                                                .item(
                                                    PopupMenuItem::new("Duplicate")
                                                        .icon(Icon::new(AssetIconName::Copy).small())
                                                        .on_click(move |_, window, cx| {
                                                            let _ = duplicate_weak.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.duplicate_workspace_entry(
                                                                        id, window, cx,
                                                                    );
                                                                },
                                                            );
                                                        }),
                                                )
                                                .item(
                                                    PopupMenuItem::new("Export…")
                                                        .icon(
                                                            Icon::new(AssetIconName::Download)
                                                                .small(),
                                                        )
                                                        .on_click(move |_, window, cx| {
                                                            let export_weak = export_weak.clone();
                                                            window.defer(cx, move |window, cx| {
                                                                let _ = export_weak.update(
                                                                    cx,
                                                                    |this, cx| {
                                                                        this.open_export_workspace_dialog(
                                                                            id, window, cx,
                                                                        );
                                                                    },
                                                                );
                                                            });
                                                        }),
                                                )
                                                .item(
                                                    PopupMenuItem::new("Reveal folder")
                                                        .icon(
                                                            Icon::new(AssetIconName::FolderOpen)
                                                                .small(),
                                                        )
                                                        .on_click(move |_, _, cx| {
                                                            let _ = reveal_weak.update(
                                                                cx,
                                                                |this, cx| {
                                                                    if let Some(store) = &this.store
                                                                    {
                                                                        cx.reveal_path(
                                                                            &store.workspace_dir(id),
                                                                        );
                                                                    }
                                                                },
                                                            );
                                                        }),
                                                )
                                                .separator()
                                                .item(
                                                    PopupMenuItem::new("Delete…")
                                                        .icon(
                                                            Icon::new(AssetIconName::Trash).small(),
                                                        )
                                                        .disabled(!can_delete)
                                                        .when(!can_delete, |item| {
                                                            item.tooltip(
                                                                "Cannot delete the only workspace",
                                                            )
                                                        })
                                                        .on_click(move |_, window, cx| {
                                                            let delete_weak = delete_weak.clone();
                                                            let name = delete_name.clone();
                                                            window.defer(cx, move |window, cx| {
                                                                let _ = delete_weak.update(
                                                                    cx,
                                                                    |this, cx| {
                                                                        this.confirm_delete_workspace(
                                                                            id, name, window, cx,
                                                                        );
                                                                    },
                                                                );
                                                            });
                                                        }),
                                                )
                                            },
                                        ),
                                    ),
                            )
                    })),
                    )
                    .vertical_scrollbar(&self.workspace_manager_scroll),
            )
            .into_any_element()
    }

    fn manage_workspaces_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let active_id = self
            .store
            .as_ref()
            .map(storage::DataStore::active_workspace_id);
        let selected_id = self.manage_workspaces_selected_id();
        let switch_target = selected_id.filter(|id| Some(*id) != active_id);
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
            .child(
                Button::new("manage-workspaces-import")
                    .label("Import…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.prompt_import_workspace(true, window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("manage-workspaces-close")
                    .label("Close")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.workspace_manager_open = false;
                        this.workspace_manager_selected = None;
                        window.close_dialog(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("manage-workspaces-switch")
                    .primary()
                    .label("Switch")
                    .disabled(switch_target.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(target) = switch_target {
                            this.switch_from_manager(target, window, cx);
                        }
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
            Some(Ok(entry)) => {
                self.select_workspace_in_manager(entry.id);
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
        // Stack the confirmation alert on top of Manage Workspaces so Cancel,
        // Escape, and Close Dialog return directly to the manager with its
        // scroll position and focus intact.
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            let close = weak.clone();
            alert
                .title(format!("Delete \"{name}\"?"))
                .description(
                    "All connections, query tabs, saved passwords, and sign-ins in this workspace will be permanently deleted.",
                )
                .width(px(480.))
                .on_close(move |_, window, cx| {
                    let close = close.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = close.update(cx, |this, cx| {
                            if this.workspace_manager_open {
                                this.workspace_manager_focus.focus(window, cx);
                            }
                        });
                    });
                })
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("cancel-delete-workspace")
                                .label("Cancel")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
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
            if self.workspace_manager_open {
                self.workspace_manager_open = false;
                self.workspace_manager_selected = None;
                window.close_dialog(cx);
            }
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
            self.refresh_workspace_summaries();
            cx.notify();
        }
    }
}
