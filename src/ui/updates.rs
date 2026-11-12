//! Update checks of release builds, their toasts, and Restart to update.
//!
//! A release build checks once a day while **Check Automatically** is on.
//! A copy that installs updates itself downloads and verifies the new
//! version in the background, then offers a restart. Other copies show how
//! to update them. The Updates button of the status bar shows the state and
//! checks on request.

use super::status_dot::DotStatus;
use super::status_tooltip::StatusTooltip;
use super::toast::{self, NotificationType};
use super::*;
use crate::logs::Severity;
use crate::update::{self, Download, Manager, Setup, Updater, Version};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::Icon;
use gpui_kit::component::notification::Notification;

/// The first automatic check waits until the window has settled.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
/// How often the timer looks at the time of the last check, so that a
/// change of the settings applies soon.
const TICK: Duration = Duration::from_secs(60);
/// Ticks that a failed check waits before the next try: an hour.
const RETRY_TICKS: u32 = 60;
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The update state of the window.
#[derive(Default)]
pub(super) struct Updates {
    setup: Option<Arc<Setup>>,
    updater: Option<Arc<Updater>>,
    pub(super) status: Status,
    /// Wall-clock time, so that time asleep counts toward the next check.
    last_check: Option<std::time::SystemTime>,
    /// The newest version that a toast announced, so it shows once.
    announced: Option<Version>,
    /// Ticks to skip before the retry of a failed check.
    retry_after: u32,
    /// The last check or download failed; the next due tick tries again.
    failed_since_check: bool,
    /// The other channel that a switch waits for, after a check found no
    /// release of it that is newer than the running build.
    waiting_for: Option<update::Channel>,
    /// Set while Qrow quits to start the installed version.
    pub(super) restarting: bool,
    /// What to do at the exit after an install: restart or run an installer.
    finish: Option<update::Finish>,
    /// The channel of the check that runs or ran last.
    checked_channel: Option<update::Channel>,
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
        let setup = updater.is_some().then_some(setup).flatten();
        // Downloads of earlier runs, like a Windows installer that already
        // ran, are not needed any more.
        if let Some(setup) = &setup {
            let _ = std::fs::remove_dir_all(&setup.downloads);
        }
        Self {
            setup,
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

    /// The state after the last check, or `None` before any check.
    pub(super) fn summary(&self) -> Option<String> {
        Some(match &self.status {
            Status::Idle => return None,
            Status::Checking => "Checking for updates…".into(),
            Status::UpToDate => match (&self.setup, self.waiting_for) {
                (Some(_), Some(channel)) => format!(
                    "Switches to {} with the next {} release.",
                    channel_name(channel),
                    channel_name(channel).to_lowercase()
                ),
                _ => "Qrow is up to date.".into(),
            },
            Status::Downloading(version) => format!("Downloading Qrow {version}…"),
            Status::Ready(download) => format!("Qrow {} is ready to install.", download.version),
            Status::Installing(version) => format!("Installing Qrow {version}…"),
            Status::Installed(version) => format!("Qrow {version} starts after a restart."),
            Status::Available(version) => format!("Qrow {version} is available."),
            Status::Failed(failure) => failure.clone(),
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

    /// The dot of the status bar button: news that needs the user.
    pub(super) fn dot(&self) -> Option<DotStatus> {
        match self.status {
            Status::Idle | Status::UpToDate => None,
            Status::Checking | Status::Downloading(_) | Status::Installing(_) => {
                Some(DotStatus::Working)
            }
            Status::Available(_) => Some(DotStatus::Attention),
            Status::Ready(_) | Status::Installed(_) => Some(DotStatus::Ready),
            Status::Failed(_) => Some(DotStatus::Error),
        }
    }

    /// The status line of the tooltip of the Updates button.
    pub(super) fn status_line(&self) -> String {
        self.summary().unwrap_or_else(|| match &self.setup {
            Some(setup) => format!("Qrow {} is installed.", setup.current),
            None => String::new(),
        })
    }
}

pub(super) fn channel_name(channel: update::Channel) -> &'static str {
    match channel {
        update::Channel::Stable => "Stable",
        update::Channel::Nightly => "Nightly",
    }
}

impl Qrow {
    /// The channel that updates follow. Only a copy that installs updates
    /// itself can switch; a package manager decides the channel of others.
    pub(super) fn update_channel(&self) -> update::Channel {
        let Some(setup) = &self.updates.setup else {
            return update::Channel::Stable;
        };
        match self.settings.update_channel {
            Some(channel) if setup.installs_updates() => channel,
            _ => setup.current.channel(),
        }
    }

    /// Follows `channel` from the next check, which comes within a minute.
    pub(super) fn set_update_channel(&mut self, channel: update::Channel, cx: &mut Context<Self>) {
        if self.update_channel() == channel {
            return;
        }
        self.settings.update_channel = Some(channel);
        self.updates.last_check = None;
        self.updates.announced = None;
        self.updates.waiting_for = None;
        // An update of the other channel is not wanted any more. Files that
        // an install already replaced stay: the restart starts them. A
        // check or download that runs now is ignored when it ends.
        let other = |version: &Version| version.channel() != channel;
        let discard = match &self.updates.status {
            Status::Idle | Status::UpToDate | Status::Failed(_) => true,
            Status::Ready(download) => other(&download.version),
            Status::Available(version) => other(version),
            Status::Installed(version) => {
                other(version) && matches!(self.updates.finish, Some(update::Finish::Run(_)))
            }
            Status::Checking | Status::Downloading(_) | Status::Installing(_) => false,
        };
        if discard {
            self.updates.status = Status::Idle;
            self.updates.finish = None;
        }
        self.changed(cx);
    }

    /// True when this copy can choose its channel.
    pub(super) fn can_switch_channel(&self) -> bool {
        self.updates
            .setup
            .as_ref()
            .is_some_and(|setup| setup.installs_updates())
    }

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
                    // A failed check waits some ticks. A clock set back also
                    // makes a check due.
                    let waiting = this.updates.retry_after > 0;
                    this.updates.retry_after = this.updates.retry_after.saturating_sub(1);
                    let old = this
                        .updates
                        .last_check
                        .is_none_or(|last| match last.elapsed() {
                            Ok(age) => age >= CHECK_INTERVAL,
                            Err(_) => true,
                        });
                    let due = !waiting && (old || this.updates.failed_since_check);
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

    /// Looks for a newer release. A manual check reports its result in a
    /// toast; an automatic check only shows a toast when there is news.
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
        self.updates.last_check = Some(std::time::SystemTime::now());
        self.updates.failed_since_check = false;
        cx.notify();
        let current = setup.current.clone();
        let channel = self.update_channel();
        self.updates.checked_channel = Some(channel);
        let check = cx
            .background_executor()
            .spawn(async move { updater.check(&current, channel) });
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
        // The channel changed during the check: the next tick checks again.
        if self.updates.checked_channel != Some(self.update_channel()) {
            self.updates.status = Status::Idle;
            self.updates.last_check = None;
            cx.notify();
            return;
        }
        match result {
            Ok(None) => {
                self.updates.status = Status::UpToDate;
                let channel = self.update_channel();
                // A switch to the other channel waits for a release of it
                // that is newer than this build.
                self.updates.waiting_for = (channel != setup.current.channel()).then_some(channel);
                let text = match self.updates.waiting_for {
                    Some(channel) => format!(
                        "No {} release is newer than Qrow {} yet",
                        channel_name(channel),
                        setup.current
                    ),
                    None => format!("Qrow {} is up to date", setup.current),
                };
                // A toast answers a check that the user asked for.
                if manual {
                    let kind = match self.updates.waiting_for {
                        Some(_) => NotificationType::Info,
                        None => NotificationType::Success,
                    };
                    window.push_notification(
                        toast::notice(kind, "update-up-to-date", text.clone(), None)
                            .id::<UpdateNotice>(),
                        cx,
                    );
                }
                self.log_update(Severity::Info, text, cx);
            }
            Ok(Some(candidate)) if setup.installs_updates() => {
                self.updates.waiting_for = None;
                self.updates.status = Status::Downloading(candidate.version.clone());
                if manual {
                    window.push_notification(
                        toast::notice(
                            NotificationType::Info,
                            "update-progress",
                            self.updates.status_line(),
                            None,
                        )
                        .id::<UpdateNotice>()
                        .autohide(true),
                        cx,
                    );
                }
                self.log_update(
                    Severity::Info,
                    format!("Downloading Qrow {}", candidate.version),
                    cx,
                );
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
                let instruction = self.updates.instruction().unwrap_or_default();
                self.log_update(
                    Severity::Info,
                    format!("Qrow {version} is available. {instruction}"),
                    cx,
                );
                if manual {
                    // Answer the request even if a toast already announced it.
                    self.updates.announced = None;
                }
                self.announce_update(&version, &setup, window, cx);
            }
            Err(error) => {
                // An automatic check fails quietly, for example offline, and
                // runs again at the next tick instead of the next day.
                eprintln!("Update check failed: {error:#}");
                // A failed automatic check is not an unseen error: offline,
                // it would come back every hour.
                let severity = if manual {
                    Severity::Error
                } else {
                    Severity::Info
                };
                self.log_update(
                    severity,
                    format!("Could not check for updates: {error:#}"),
                    cx,
                );
                self.updates.retry_after = RETRY_TICKS;
                self.updates.failed_since_check = true;
                self.updates.status = if manual {
                    window.push_notification(
                        toast::error(
                            "update-check-failed",
                            "Could not check for updates",
                            "Check the network connection and try again.",
                        )
                        .id::<UpdateNotice>(),
                        cx,
                    );
                    Status::Failed("Could not check for updates. Try again later.".into())
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
        // The channel changed during the download: drop it.
        if let Ok(download) = &result
            && download.version.channel() != self.update_channel()
        {
            self.updates.status = Status::Idle;
            self.updates.last_check = None;
            cx.notify();
            return;
        }
        match result {
            Ok(download) => {
                let version = download.version.clone();
                self.log_update(
                    Severity::Info,
                    format!("Downloaded and verified Qrow {version}"),
                    cx,
                );
                self.updates.status = Status::Ready(Arc::new(download));
                if self.updates.announced.as_ref() != Some(&version) {
                    self.updates.announced = Some(version.clone());
                    window.push_notification(ready_toast(&version, cx.weak_entity()), cx);
                }
            }
            Err(error) => {
                eprintln!("Update download failed: {error:#}");
                self.log_update(
                    Severity::Error,
                    format!("Could not download the update: {error:#}"),
                    cx,
                );
                self.updates.retry_after = RETRY_TICKS;
                self.updates.failed_since_check = true;
                self.updates.status =
                    Status::Failed("Could not download the update. Try again later.".into());
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
        window.push_notification(toast.id::<UpdateNotice>().autohide(true), cx);
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
                        Ok(finish) => {
                            let text = match finish {
                                update::Finish::Restart => {
                                    format!("Installed Qrow {version}. It starts after the restart.")
                                }
                                update::Finish::Run(_) => format!(
                                    "Prepared the installer of Qrow {version}. It runs when Qrow quits."
                                ),
                            };
                            this.log_update(Severity::Info, text, cx);
                            this.updates.finish = Some(finish);
                            this.updates.status = Status::Installed(version);
                            this.quit_to_restart(window, cx);
                        }
                        Err(error) => {
                            let error = format!("{error:#}");
                            eprintln!("Update install failed: {error}");
                            this.log_update(
                                Severity::Error,
                                format!("Could not install Qrow {version}: {error}"),
                                cx,
                            );
                            this.updates.status =
                                Status::Failed("Could not install the update.".into());
                            window.push_notification(
                                toast::error(
                                    "update-failed",
                                    "Could not install the update",
                                    error,
                                )
                                .id::<UpdateNotice>(),
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

    /// Quits and starts the installed version. Quit can still stop for
    /// running work or a failed save; their retry buttons keep the restart,
    /// and a later Quit from the user replaces it.
    fn quit_to_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.updates.restarting = true;
        self.request_quit(window, cx);
        cx.notify();
    }

    /// Writes an update event to the Application log of Activity.
    fn log_update(&self, severity: Severity, text: String, cx: &mut Context<Self>) {
        let entry = crate::activity::ActivityEntry::new(severity, text).with_source("Updates");
        self.record_activity(crate::activity::APPLICATION, entry, cx);
    }

    /// Ends the application: restarts into an installed update, or quits.
    pub(super) fn exit(&mut self, cx: &mut Context<Self>) {
        if self.updates.restarting {
            match self.updates.finish.take() {
                // The installer closes nothing that is still open: Qrow quits
                // right after it starts. It starts Qrow when it is done.
                Some(update::Finish::Run(mut installer)) => {
                    if let Err(error) = installer.spawn() {
                        eprintln!("Could not start the update installer: {error}");
                    }
                }
                _ => {
                    if let Some(path) = self.updates.setup.as_ref().and_then(|s| s.restart_path()) {
                        cx.set_restart_path(path);
                    }
                    cx.restart();
                }
            }
        }
        // The platform quits after it starts the new process. If it could not
        // start it, Qrow still quits; the new version starts at the next launch.
        cx.quit();
    }
}

impl Qrow {
    /// The Updates button of the status bar: a dot for news, the state in
    /// its tooltip, and a toast with the action that fits the state on click.
    pub(super) fn update_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = format!("Updates, {}", self.updates.status_line());
        let button = Button::new("update-button")
            .ghost()
            .small()
            .icon(Icon::new(AssetIconName::CircleFadingArrowUp).small())
            .accessibility_label(label)
            .on_click(cx.listener(|this, _, window, cx| this.show_update_state(window, cx)));
        div()
            .id("update-tooltip-trigger")
            .tooltip(StatusTooltip::live(
                cx.entity().downgrade(),
                None,
                |qrow, _| Some(StatusTooltip::new("Updates", qrow.updates.status_line())),
            ))
            .child(DotStatus::on_button(self.updates.dot(), button, cx))
            .into_any_element()
    }

    /// Answers a click on the Updates button. Without news it checks, and a
    /// toast reports the result. With news it shows the toast of the news
    /// again, so its action is back after the toast hid.
    fn show_update_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(setup) = self.updates.setup.clone() else {
            return;
        };
        match self.updates.status.clone() {
            Status::Idle | Status::UpToDate | Status::Failed(_) => {
                self.check_for_updates(true, window, cx)
            }
            Status::Checking | Status::Downloading(_) | Status::Installing(_) => {
                let title = self.updates.status_line();
                window.push_notification(
                    toast::notice(NotificationType::Info, "update-progress", title, None)
                        .id::<UpdateNotice>()
                        .autohide(true),
                    cx,
                );
            }
            Status::Available(version) => {
                self.updates.announced = None;
                self.announce_update(&version, &setup, window, cx);
            }
            Status::Ready(download) => {
                window.push_notification(ready_toast(&download.version, cx.weak_entity()), cx)
            }
            Status::Installed(version) => {
                window.push_notification(ready_toast(&version, cx.weak_entity()), cx)
            }
        }
    }
}

/// The ID that all update toasts share: a new one replaces the one on screen.
struct UpdateNotice;

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
    .id::<UpdateNotice>()
    .autohide(true)
}
