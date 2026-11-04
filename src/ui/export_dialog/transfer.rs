//! Captured transfer settings.
use super::*;

impl ExportDialog {
    pub(super) fn transfer_profile(&self) -> Option<&Profile> {
        if self.scope.executes() {
            self.run.as_ref().map(|intent| &intent.profile)
        } else {
            self.profile.as_ref()
        }
    }
}
