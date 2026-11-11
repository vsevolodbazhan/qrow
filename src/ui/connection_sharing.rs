//! Copy settings and Paste connection, through the clipboard, and Import
//! connections and Export connections, through a file. Passwords stay in the
//! secret store of this computer.
use super::*;
use crate::model::{
    SignIn,
    sharing::{
        MAX_CONNECTIONS_FILE_BYTES, MAX_FILE_CONNECTIONS, PlacedConnections,
        SharedConnectionSettings, connections_file_text, place_connections, read_connections_file,
    },
    shown_catalog,
};
use gpui_kit::component::notification::NotificationType;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// The name that Export connections suggests.
const EXPORT_FILENAME: &str = "qrow-connections.json";

/// A connection that Paste connection opened in Connection Settings.
pub(super) struct PastedConnection {
    /// A sign-in that the workspace does not have. Save adds it when the
    /// connection still uses it.
    pub(super) sign_in: Option<SignIn>,
}

impl Qrow {
    /// The sign-ins that Connection Settings offers: the saved ones and the
    /// sign-in of a pasted connection.
    pub(super) fn offered_sign_ins(&self, pasted: Option<&SignIn>) -> Vec<SignIn> {
        self.sign_ins.iter().chain(pasted).cloned().collect()
    }

    /// Puts the settings of a connection on the clipboard, without its
    /// password, so that another user can paste them.
    pub(super) fn copy_connection_settings(&self, id: Uuid, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.iter().find(|profile| profile.id == id) else {
            return;
        };
        let catalog = shown_catalog(profile, &self.shared_catalogs);
        cx.write_to_clipboard(ClipboardItem::new_string(
            profile.to_shared_text(&catalog, self.connection_sign_in(profile)),
        ));
    }

    fn connection_sign_in(&self, profile: &Profile) -> Option<&SignIn> {
        let id = profile.authentication.sign_in()?;
        self.sign_ins.iter().find(|sign_in| sign_in.id == id)
    }

    /// Opens New Connection with the connection settings on the clipboard,
    /// so that the user can examine them and enter a password before Save
    /// adds the connection.
    pub(super) fn paste_connection(
        &mut self,
        _: &PasteConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        match Profile::from_shared_text(&text) {
            Ok(shared) => {
                let PlacedConnections { profiles, sign_ins } =
                    place_connections(vec![shared], &self.profiles, &self.sign_ins);
                let Some(profile) = profiles.into_iter().next() else {
                    return;
                };
                let pasted = PastedConnection {
                    sign_in: sign_ins.into_iter().next(),
                };
                self.open_profile_editor(profile, true, Some(pasted), window, cx);
            }
            Err(error) => {
                error_alert(
                    "paste-connection-error",
                    "Cannot paste the connection",
                    error.to_string(),
                    window,
                    cx,
                );
                cx.notify();
            }
        }
    }

    /// Writes all connections to a file that the user chooses.
    pub(super) fn export_connections(
        &mut self,
        _: &ExportConnections,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        if self.profiles.is_empty() {
            window.push_notification(
                toast::error(
                    "connections-file-toast",
                    "Could not export connections",
                    "Add a connection first.",
                ),
                cx,
            );
            return;
        }
        let count = self.profiles.len();
        let text = connections_file_text(self.profiles.iter().map(|profile| {
            (
                profile,
                shown_catalog(profile, &self.shared_catalogs),
                self.connection_sign_in(profile),
            )
        }));
        // Import connections must be able to read the file.
        if count > MAX_FILE_CONNECTIONS || text.len() as u64 > MAX_CONNECTIONS_FILE_BYTES {
            window.push_notification(
                toast::error(
                    "connections-file-toast",
                    "Could not export connections",
                    format!(
                        "A file can have {MAX_FILE_CONNECTIONS} connections and {} MB or less.",
                        MAX_CONNECTIONS_FILE_BYTES / 1024 / 1024
                    ),
                ),
                cx,
            );
            return;
        }
        let directory = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
        let prompt = cx.prompt_for_new_path(&directory, Some(EXPORT_FILENAME));
        cx.spawn_in(window, async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                error => {
                    let _ = this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            toast::error(
                                "connections-file-toast",
                                "Could not open the save panel",
                                format!("{error:?}"),
                            ),
                            cx,
                        )
                    });
                    return;
                }
            };
            let result = cx
                .background_spawn(async move { save_file(&path, &text).map(|()| path) })
                .await;
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(path) => window.push_notification(exported_notification(count, path), cx),
                Err(error) => window.push_notification(
                    toast::error(
                        "connections-file-toast",
                        "Could not export connections",
                        error.to_string(),
                    ),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Adds the connections of a file that the user chooses. The connections
    /// have no passwords.
    pub(super) fn import_connections(
        &mut self,
        _: &ImportConnections,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => path,
                    None => return,
                },
                Ok(Ok(None)) => return,
                error => {
                    let _ = this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            toast::error(
                                "connections-file-toast",
                                "Could not open the file panel",
                                format!("{error:?}"),
                            ),
                            cx,
                        );
                    });
                    return;
                }
            };
            let Ok((credentials, demo)) =
                this.read_with(cx, |this, _| (this.credentials.clone(), this.demo))
            else {
                return;
            };
            // The passwords are ready before the connections show.
            let read = cx
                .background_spawn(async move {
                    let connections =
                        read_file(&path).and_then(|text| read_connections_file(&text))?;
                    if !demo {
                        save_empty_passwords(credentials.as_ref(), &connections)?;
                    }
                    anyhow::Ok(connections)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match read {
                Ok(connections) => {
                    let placed = place_connections(connections, &this.profiles, &this.sign_ins);
                    this.add_imported_connections(placed, window, cx);
                }
                Err(error) => window.push_notification(
                    toast::error(
                        "connections-file-toast",
                        "Could not import connections",
                        error.to_string(),
                    ),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Adds imported connections and their new sign-ins after the existing
    /// ones.
    fn add_imported_connections(
        &mut self,
        placed: PlacedConnections,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(first) = placed.profiles.first().map(|profile| profile.id) else {
            return;
        };
        let count = placed.profiles.len();
        let had_profiles = !self.profiles.is_empty();
        let passwords = placed
            .profiles
            .iter()
            .any(|profile| profile.authentication == Authentication::Password);
        let ids: Vec<Uuid> = placed.profiles.iter().map(|profile| profile.id).collect();
        let added: Vec<String> = placed
            .sign_ins
            .iter()
            .map(|sign_in| format!("\"{}\" ({})", sign_in.name, sign_in.issuer))
            .collect();
        self.sign_ins.extend(placed.sign_ins);
        self.profiles.extend(placed.profiles);
        // The first connection takes the query tabs that had no connection.
        // Each other connection gets a query tab, like a new connection.
        if !had_profiles {
            for tab in &mut self.tabs {
                tab.saved.profile = Some(first);
            }
        }
        for id in ids {
            if !self.tabs.iter().any(|tab| tab.saved.profile == Some(id)) {
                let tab = self.make_tab(SavedTab::new(1, Some(id)), window, cx);
                self.tabs.push(tab);
            }
        }
        if !had_profiles {
            self.activate(self.active, window, cx);
        }
        self.sync_catalogs(None, cx);
        self.changed(cx);
        let title = if count == 1 {
            "Imported 1 connection".to_owned()
        } else {
            format!("Imported {count} connections")
        };
        let mut lines = Vec::new();
        if passwords {
            lines.push("Edit each connection to enter its password.".to_owned());
        }
        if !added.is_empty() {
            lines.push(format!("Added sign-ins: {}.", added.join(", ")));
        }
        let detail = !lines.is_empty();
        let detail_text = lines.join(" ");
        window.push_notification(
            toast::notice(
                NotificationType::Success,
                "connections-imported-toast",
                title,
                detail.then_some(detail_text.into()),
            ),
            cx,
        );
    }
}

/// Like New Connection, a Trino connection with password authentication
/// keeps an empty password until the user enters one.
fn save_empty_passwords(
    credentials: &dyn crate::storage::Credentials,
    connections: &[SharedConnectionSettings],
) -> anyhow::Result<()> {
    for SharedConnectionSettings { profile, .. } in connections {
        if profile.database_type == crate::model::DatabaseType::Trino
            && profile.authentication == Authentication::Password
        {
            credentials
                .set_password(profile.id, "")
                .map_err(|error| anyhow::anyhow!("{}: {error:#}", profile.name))?;
        }
    }
    Ok(())
}

/// An alert with `message` and an OK button. `id` names the message, and
/// `close-{id}` the button.
pub(super) fn error_alert(
    id: &'static str,
    title: &'static str,
    message: String,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .width(px(360.))
            .title(title)
            .description(
                div()
                    .id(id)
                    .test_support()
                    .role(Role::Label)
                    .aria_label(message.clone())
                    .child(message.clone()),
            )
            .footer(
                DialogFooter::new().justify_end().child(
                    Button::new(SharedString::from(format!("close-{id}")))
                        .primary()
                        .label("OK")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    });
}

fn read_file(path: &Path) -> anyhow::Result<String> {
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(MAX_CONNECTIONS_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| anyhow::anyhow!("The file is not a text file."))?;
    anyhow::ensure!(
        text.len() as u64 <= MAX_CONNECTIONS_FILE_BYTES,
        "The file is larger than {} MB.",
        MAX_CONNECTIONS_FILE_BYTES / 1024 / 1024
    );
    Ok(text)
}

fn save_file(path: &Path, text: &str) -> std::io::Result<()> {
    crate::export::save(path, &AtomicBool::new(false), |out| {
        out.write_all(text.as_bytes())
    })
}

fn exported_notification(
    count: usize,
    path: PathBuf,
) -> gpui_kit::component::notification::Notification {
    toast::saved_file(
        toast::SavedFileIds {
            notice: "connections-exported-notice",
            title: "connections-exported-title",
            filename: "connections-exported-filename",
            reveal: "reveal-connections-file",
        },
        if count == 1 {
            "Exported 1 connection without its password".to_owned()
        } else {
            format!("Exported {count} connections without passwords")
        },
        path,
    )
    .autohide(true)
}
