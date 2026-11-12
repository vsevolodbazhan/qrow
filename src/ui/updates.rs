//! Update checks of release builds, their toasts, and Restart to update.
//!
//! A release build checks once a day while **Check Automatically** is on.
//! A copy that installs updates itself downloads and verifies the new
//! version in the background, then offers a restart. Other copies show how
//! to update them. The About dialog shows the state and checks on request.

use super::toast::{self, NotificationType};
use super::*;
use crate::update::{self, Download, Manager, Setup, Updater, Version};
use gpui_kit::component::notification::Notification;

/// The first automatic check waits until the window has settled.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
/// How often the timer looks at the time of the last check.
const TICK: Duration = Duration::from_secs(60 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The update state of the window.
#[derive(Default)]
pub(super) struct Updates {
    setup: Option<Arc<Setup>>,
    updater: Option<Arc<Updater>>,
    pub(super) status: Status,
    last_check: Option<Instant>,
    /// The newest version that a toast announced, so it shows once.
    announced: Option<Version>,
    /// Set while Qrow quits to start the installed version.
    pub(super) restarting: bool,
}

#[derive(Clone, Default)]
pub(super) enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading(Version),
    Ready(Arc<Download>),
    Installing(Version),
    /// The new version is in place and starts at the next launch.
    Installed(Version),
    /// A newer version that another program or the user installs.
    Available(Version),
    Failed(String),
}

impl Updates {
    pub(super) fn new(setup: Option<Arc<Setup>>, trust: &crate::tls::Trust) -> Self {
        let updater = setup.as_ref().and_then(|setup| {
            Updater::new(setup.source.clone(), trust)
                .map_err(|error| eprintln!("Update checks are off: {error:#}"))
                .ok()
                .map(Arc::new)
        });
        Self {
            setup: updater.is_some().then_some(setup).flatten(),
            updater,
            ..Self::default()
        }
    }

    /// True when this copy can check for updates.
    pub(super) fn enabled(&self) -> bool {
        self.setup.is_some()
    }

    fn busy(&self) -> bool {
        matches!(
            self.status,
            Status::Checking | Status::Downloading(_) | Status::Installing(_)
        )
    }

    /// The line that the About dialog shows, or `None` before any check.
    pub(super) fn summary(&self) -> Option<String> {
        Some(match &self.status {
            Status::Idle => return None,
            Status::Checking => "Checking for updates…".into(),
            Status::UpToDate => "Qrow is up to date.".into(),
            Status::Downloading(version) => format!("Downloading Qrow {version}…"),
            Status::Ready(download) => format!("Qrow {} is ready to install.", download.version),
            Status::Installing(version) => format!("Installing Qrow {version}…"),
            Status::Installed(version) => format!("Qrow {version} starts after a restart."),
            Status::Available(version) => format!("Qrow {version} is available."),
            Status::Failed(error) => format!("Could not update: {error}"),
        })
    }

    /// How to install an available version on this copy.
    pub(super) fn instruction(&self) -> Option<String> {
        let Status::Available(_) = self.status else {
            return None;
        };
        let install = &self.setup.as_ref()?.install;
        Some(match install {
            update::Install::Managed(Manager::Homebrew { cask }) => {
                format!("Run brew upgrade --cask {cask} to update.")
            }
            update::Install::Managed(Manager::Scoop { app }) => {
                format!("Run scoop update {app} to update.")
            }
            update::Install::Managed(Manager::Flatpak) => "Update it with Flatpak.".into(),
            update::Install::Managed(Manager::Snap) => "Update it with Snap.".into(),
            _ => "Download it from GitHub Releases.".into(),
        })
    }

    /// The command that updates this copy, for a Copy command action.
    fn command(&self) -> Option<String> {
        match &self.setup.as_ref()?.install {
            update::Install::Managed(Manager::Homebrew { cask }) => {
                Some(format!("brew upgrade --cask {cask}"))
            }
            update::Install::Managed(Manager::Scoop { app }) => Some(format!("scoop update {app}")),
            _ => None,
        }
    }

    pub(super) fn can_restart(&self) -> bool {
        matches!(self.status, Status::Ready(_) | Status::Installed(_))
    }

    pub(super) fn checking(&self) -> bool {
        self.busy()
    }
}

impl Qrow {
    /// Starts the daily check. The timer runs for the life of the window and
    /// reads the setting at each tick, so the switch applies at once.
    pub(super) fn start_update_checks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.updates.enabled() {
            return;
        }
        cx.spawn_in(window, async move |weak, cx| {
            cx.background_executor().timer(FIRST_CHECK_DELAY).await;
            loop {
                let checked = weak.update_in(cx, |this, window, cx| {
                    let due = this
                        .updates
                        .last_check
                        .is_none_or(|last| last.elapsed() >= CHECK_INTERVAL);
                    if this.settings.check_for_updates && due {
                        this.check_for_updates(false, window, cx);
                    }
                });
                if checked.is_err() {
                    break;
                }
                cx.background_executor().timer(TICK).await;
            }
        })
        .detach();
    }

    /// Looks for a newer release. A manual check reports its result in
    /// About; an automatic check only shows a toast when there is news.
    pub(super) fn check_for_updates(
        &mut self,
        manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(setup), Some(updater)) =
            (self.updates.setup.clone(), self.updates.updater.clone())
        else {
            return;
        };
        // A found update stays until the restart; a later check would only
        // download it again.
        if self.updates.busy() || (!manual && self.updates.can_restart()) {
            return;
        }
        if manual && self.updates.can_restart() {
            cx.notify();
            return;
        }
        self.updates.status = Status::Checking;
        self.updates.last_check = Some(Instant::now());
        cx.notify();
        let current = setup.current.clone();
        let check = cx
            .background_executor()
            .spawn(async move { updater.check(&current) });
        cx.spawn_in(window, async move |weak, cx| {
            let result = check.await;
            let _ = weak.update_in(cx, |this, window, cx| {
                this.finish_update_check(result, manual, window, cx)
            });
        })
        .detach();
    }

    fn finish_update_check(
        &mut self,
        result: anyhow::Result<Option<update::Candidate>>,
        manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(setup), Some(updater)) =
            (self.updates.setup.clone(), self.updates.updater.clone())
        else {
            return;
        };
        match result {
            Ok(None) => self.updates.status = Status::UpToDate,
            Ok(Some(candidate)) if setup.installs_updates() => {
                self.updates.status = Status::Downloading(candidate.version.clone());
                let download = cx.background_executor().spawn(async move {
                    updater.download(&candidate, &setup.install, &setup.arch, &setup.downloads)
                });
                cx.spawn_in(window, async move |weak, cx| {
                    let result = download.await;
                    let _ = weak.update_in(cx, |this, window, cx| {
                        this.finish_update_download(result, window, cx)
                    });
                })
                .detach();
            }
            Ok(Some(candidate)) => {
                let version = candidate.version;
                self.updates.status = Status::Available(version.clone());
                self.announce_update(&version, &setup, window, cx);
            }
            Err(error) => {
                // An automatic check fails quietly, for example offline.
                eprintln!("Update check failed: {error:#}");
                self.updates.status = if manual {
                    Status::Failed(format!("{error:#}"))
                } else {
                    Status::Idle
                };
            }
        }
        cx.notify();
    }

    fn finish_update_download(
        &mut self,
        result: anyhow::Result<Download>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(download) => {
                let version = download.version.clone();
                self.updates.status = Status::Ready(Arc::new(download));
                if self.updates.announced.as_ref() != Some(&version) {
                    self.updates.announced = Some(version.clone());
                    window.push_notification(ready_toast(&version, cx.weak_entity()), cx);
                }
            }
            Err(error) => {
                eprintln!("Update download failed: {error:#}");
                self.updates.status = Status::Failed(format!("{error:#}"));
            }
        }
        cx.notify();
    }

    fn announce_update(
        &mut self,
        version: &Version,
        setup: &Setup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.updates.announced.as_ref() == Some(version) {
            return;
        }
        self.updates.announced = Some(version.clone());
        let detail = self.updates.instruction().map(SharedString::from);
        let toast = toast::notice(
            NotificationType::Info,
            "update-available",
            format!("Qrow {version} is available"),
            detail,
        );
        let toast = match self.updates.command() {
            Some(command) => toast.action(move |_, _, _| {
                let command = command.clone();
                Button::new("update-copy-command")
                    .label("Copy command")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(command.clone()))
                    })
            }),
            None => {
                let page = setup.release_page(version);
                toast.action(move |_, _, _| {
                    let page = page.clone();
                    Button::new("update-open-release")
                        .label("Open release")
                        .on_click(move |_, _, cx| cx.open_url(&page))
                })
            }
        };
        // An action keeps a toast open; update toasts hide like the others.
        window.push_notification(toast.autohide(true), cx);
    }

    /// Installs a downloaded update and quits to start it.
    pub(super) fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(setup) = self.updates.setup.clone() else {
            return;
        };
        match self.updates.status.clone() {
            Status::Installed(_) => self.quit_to_restart(window, cx),
            Status::Ready(download) => {
                let version = download.version.clone();
                self.updates.status = Status::Installing(version.clone());
                cx.notify();
                let install = setup.install.clone();
                let task = cx
                    .background_executor()
                    .spawn(async move { update::apply(&download, &install) });
                cx.spawn_in(window, async move |weak, cx| {
                    let result = task.await;
                    let _ = weak.update_in(cx, |this, window, cx| match result {
                        Ok(()) => {
                            this.updates.status = Status::Installed(version);
                            this.quit_to_restart(window, cx);
                        }
                        Err(error) => {
                            let error = format!("{error:#}");
                            this.updates.status = Status::Failed(error.clone());
                            window.push_notification(
                                toast::error(
                                    "update-failed",
                                    "Could not install the update",
                                    error,
                                ),
                                cx,
                            );
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            _ => {}
        }
    }

    fn quit_to_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.updates.restarting = true;
        self.request_quit(window, cx);
        // Quit can stop for running work or a failed save. The new version
        // then starts at the next launch.
        if !self.quit_confirmed && self.pending_quit.is_none() && !self.quit_warning_open {
            self.updates.restarting = false;
        }
        cx.notify();
    }

    /// Ends the application: restarts into an installed update, or quits.
    pub(super) fn exit(&mut self, cx: &mut Context<Self>) {
        if self.updates.restarting {
            if let Some(path) = self.updates.setup.as_ref().and_then(|s| s.restart_path()) {
                cx.set_restart_path(path);
            }
            cx.restart();
        } else {
            cx.quit();
        }
    }
}

/// The toast of a downloaded update, with **Restart to update**.
fn ready_toast(version: &Version, owner: WeakEntity<Qrow>) -> Notification {
    toast::notice(
        NotificationType::Info,
        "update-ready",
        format!("Qrow {version} is ready"),
        Some("Restart to start the new version.".into()),
    )
    .action(move |_, _, _| {
        let owner = owner.clone();
        Button::new("update-restart")
            .primary()
            .label("Restart to update")
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |this, cx| this.restart_to_update(window, cx));
            })
    })
    .autohide(true)
}
