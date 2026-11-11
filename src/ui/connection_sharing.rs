//! Copy settings and Paste connection, through the clipboard. Passwords stay
//! in the secret store of this computer.
use super::*;
use crate::model::{SignIn, sharing::place_connection, shown_catalog};

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
        let sign_in = profile
            .authentication
            .sign_in()
            .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id));
        cx.write_to_clipboard(ClipboardItem::new_string(
            profile.to_shared_text(&catalog, sign_in),
        ));
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
                let (profile, sign_in) = place_connection(shared, &self.profiles, &self.sign_ins);
                let pasted = PastedConnection { sign_in };
                self.open_profile_editor(profile, true, Some(pasted), window, cx);
            }
            Err(error) => window.push_notification(
                toast::error(
                    "paste-connection-toast",
                    "Could not paste connection",
                    error.to_string(),
                ),
                cx,
            ),
        }
    }
}
