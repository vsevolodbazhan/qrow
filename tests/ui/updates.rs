//! Update checks of the window against a local release server with a
//! synthetic signing key. No test contacts GitHub.

use crate::support::*;
use gpui_kit::TestAppContext;
use qrow::{
    model::Workspace,
    ui::{OpenAbout, OpenSettings},
    update::{Install, Manager, Setup, Source, Version},
};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// A release server: fixed responses over HTTP/1.1, and the requested paths.
struct Releases {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    requests: Arc<Mutex<Vec<String>>>,
    key: minisign::KeyPair,
    _downloads: tempfile::TempDir,
    downloads: std::path::PathBuf,
}

impl Releases {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let (served, recorded) = (files.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                let _ = reader.read_line(&mut request);
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                        break;
                    }
                }
                recorded.lock().unwrap().push(path.clone());
                let (status, body) = match served.lock().unwrap().get(&path).cloned() {
                    Some(body) => ("200 OK", body),
                    None => ("404 Not Found", Vec::new()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let downloads = tempfile::tempdir().unwrap();
        Self {
            base,
            files,
            requests,
            key: minisign::KeyPair::generate_unencrypted_keypair().unwrap(),
            downloads: downloads.path().join("updates"),
            _downloads: downloads,
        }
    }

    /// Publishes `tag` with one package and its signed checksums.
    fn publish(&self, tag: &str, name: &str, package: &[u8]) {
        let digest = ring::digest::digest(&ring::digest::SHA256, package);
        let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let sums = format!("{hex}  ./{name}\n");
        let signature = minisign::sign(
            Some(&self.key.pk),
            &self.key.sk,
            sums.as_bytes(),
            Some(&format!("Qrow release {tag}")),
            None,
        )
        .unwrap()
        .into_string();
        let mut assets = Vec::new();
        for (asset, body) in [
            ("SHA256SUMS", sums.into_bytes()),
            ("SHA256SUMS.minisig", signature.into_bytes()),
            (name, package.to_vec()),
        ] {
            let path = format!("/download/{tag}/{asset}");
            assets.push(serde_json::json!({
                "name": asset, "size": body.len(),
                "browser_download_url": format!("{}{path}", self.base),
            }));
            self.files.lock().unwrap().insert(path, body);
        }
        let releases = serde_json::json!([
            {"tag_name": tag, "draft": false, "prerelease": false, "assets": assets}
        ]);
        self.files
            .lock()
            .unwrap()
            .insert("/releases".into(), serde_json::to_vec(&releases).unwrap());
    }

    fn setup(&self, install: Install) -> Setup {
        Setup {
            source: Source {
                releases: format!("{}/releases", self.base),
                downloads: format!("{}/download", self.base),
                public_key: self.key.pk.to_box().unwrap().into_string(),
                https_only: false,
            },
            current: Version::parse("0.5.0").unwrap(),
            install,
            arch: "x86_64".into(),
            downloads: self.downloads.clone(),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn homebrew() -> Install {
    Install::Managed(Manager::Homebrew {
        cask: "qrow".into(),
    })
}

/// Runs the automatic check that starts after the window settles.
fn run_first_check(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_secs(11));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn about_checks_on_request_and_reports_an_up_to_date_copy(cx: &mut TestAppContext) {
    let releases = Releases::start();
    releases.publish("v0.5.0", "Qrow-0.5.0-x86_64.dmg", b"dmg");
    let app = TestApp::launch_with_updates(cx, Workspace::default(), releases.setup(homebrew()));
    app.dispatch(cx, OpenAbout);
    app.wait_for(cx, "about-check-updates");
    app.click(cx, "about-check-updates");
    app.wait_for(cx, "about-update-status");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "about-update-status").as_deref(),
            Some("Qrow is up to date.")
        );
    });
    assert_eq!(releases.requests(), ["/releases"]);
}

#[gpui_kit::test]
fn a_managed_copy_shows_the_command_that_updates_it(cx: &mut TestAppContext) {
    let releases = Releases::start();
    releases.publish("v0.6.0", "Qrow-0.6.0-x86_64.dmg", b"dmg");
    let app = TestApp::launch_with_updates(cx, Workspace::default(), releases.setup(homebrew()));
    run_first_check(cx);
    app.wait_for(cx, "update-available");
    app.update(cx, |window, _| {
        assert!(shows(window, "Qrow 0.6.0 is available"));
        assert!(shows(window, "Run brew upgrade --cask qrow to update."));
        assert!(present(window, &"update-copy-command".into()));
    });
    // Homebrew installs the package, so Qrow does not download it.
    assert_eq!(releases.requests(), ["/releases"]);
    app.dispatch(cx, OpenAbout);
    app.wait_for(cx, "about-update-instruction");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "about-update-status").as_deref(),
            Some("Qrow 0.6.0 is available.")
        );
    });
}

#[gpui_kit::test]
fn automatic_checks_follow_the_setting(cx: &mut TestAppContext) {
    let releases = Releases::start();
    releases.publish("v0.6.0", "Qrow-0.6.0-x86_64.dmg", b"dmg");
    let mut workspace = Workspace::default();
    workspace.settings.check_for_updates = false;
    let app = TestApp::launch_with_updates(cx, workspace, releases.setup(homebrew()));
    run_first_check(cx);
    assert!(releases.requests().is_empty());
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "upgrade");
    app.wait_for(cx, "check-for-updates");
    app.click(cx, "check-for-updates");
    app.wait_saved(cx, "update checks on", |workspace| {
        workspace.settings.check_for_updates
    });
    // The hourly timer notices the switch.
    cx.executor().advance_clock(Duration::from_secs(60 * 60));
    cx.run_until_parked();
    assert_eq!(releases.requests(), ["/releases"]);
}

#[gpui_kit::test]
fn a_failed_automatic_check_runs_again_within_the_hour(cx: &mut TestAppContext) {
    let releases = Releases::start();
    // No release list yet: the first check fails.
    let app = TestApp::launch_with_updates(cx, Workspace::default(), releases.setup(homebrew()));
    run_first_check(cx);
    assert_eq!(releases.requests(), ["/releases"]);
    app.update(cx, |window, _| {
        assert!(!present(window, &"update-available".into()))
    });
    releases.publish("v0.6.0", "Qrow-0.6.0-x86_64.dmg", b"dmg");
    cx.executor().advance_clock(Duration::from_secs(60 * 60));
    cx.run_until_parked();
    app.wait_for(cx, "update-available");
    assert_eq!(releases.requests(), ["/releases", "/releases"]);
}

#[gpui_kit::test]
fn builds_without_updates_have_no_update_controls(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenAbout);
    app.wait_for(cx, "about-copyright");
    app.update(cx, |window, _| {
        assert!(!present(window, &"about-check-updates".into()));
    });
    app.press(cx, "escape");
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "upgrade");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(!present(window, &"check-for-updates".into()));
    });
}

/// A gzip tar archive with the layout of the Linux package.
#[cfg(target_os = "linux")]
fn linux_archive(executable: &[u8]) -> Vec<u8> {
    let package = tempfile::tempdir().unwrap();
    for (path, data) in [
        ("bin/qrow", executable),
        ("share/applications/qrow.desktop", b"[Desktop Entry]\n"),
    ] {
        let file = package.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, data).unwrap();
    }
    let output = tempfile::tempdir().unwrap();
    let archive = output.path().join("qrow.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("-C")
        .arg(package.path())
        .args(["-czf"])
        .arg(&archive)
        .arg(".")
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(archive).unwrap()
}

#[cfg(target_os = "linux")]
fn write(path: &Path, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn a_downloaded_update_installs_and_restarts_into_the_new_version(cx: &mut TestAppContext) {
    let releases = Releases::start();
    let name = "Qrow-0.6.0-linux-x86_64.tar.gz";
    releases.publish("v0.6.0", name, &linux_archive(b"new qrow"));
    let prefix = tempfile::tempdir().unwrap();
    write(&prefix.path().join("bin/qrow"), b"old qrow");
    write(&prefix.path().join("share/applications/qrow.desktop"), b"");
    let install = Install::Archive {
        prefix: prefix.path().into(),
    };
    let app = TestApp::launch_with_updates(cx, Workspace::default(), releases.setup(install));
    run_first_check(cx);
    app.wait_for(cx, "update-ready");
    assert!(
        releases
            .requests()
            .contains(&format!("/download/v0.6.0/{name}"))
    );
    // Nothing changes before the user asks for the restart.
    assert_eq!(
        std::fs::read(prefix.path().join("bin/qrow")).unwrap(),
        b"old qrow"
    );
    let mut restart = cx.expect_restart();
    app.click(cx, "update-restart");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let (path, _) = loop {
        cx.run_until_parked();
        if let Ok(Some(restart)) = restart.try_recv() {
            break restart;
        }
        assert!(std::time::Instant::now() < deadline, "Qrow did not restart");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(path, Some(prefix.path().join("bin/qrow")));
    assert_eq!(
        std::fs::read(prefix.path().join("bin/qrow")).unwrap(),
        b"new qrow"
    );
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn a_cancelled_restart_does_not_restart_a_later_quit(cx: &mut TestAppContext) {
    use std::sync::atomic::AtomicBool;
    let releases = Releases::start();
    let name = "Qrow-0.6.0-linux-x86_64.tar.gz";
    releases.publish("v0.6.0", name, &linux_archive(b"new qrow"));
    let prefix = tempfile::tempdir().unwrap();
    write(&prefix.path().join("bin/qrow"), b"old qrow");
    write(&prefix.path().join("share/applications/qrow.desktop"), b"");
    let install = Install::Archive {
        prefix: prefix.path().into(),
    };
    let app = TestApp::launch_with_updates(cx, Workspace::default(), releases.setup(install));
    run_first_check(cx);
    app.wait_for(cx, "update-ready");
    // Running work makes Quit ask first.
    let guard = cx.update(|cx| {
        cx.global::<qrow::export::Jobs>()
            .register(Arc::new(AtomicBool::new(false)))
    });
    let mut restart = cx.expect_restart();
    app.click(cx, "update-restart");
    app.wait_for(cx, "keep-working");
    app.click(cx, "keep-working");
    app.wait_gone(cx, "keep-working");
    // The update is in place; it starts at the next launch.
    assert_eq!(
        std::fs::read(prefix.path().join("bin/qrow")).unwrap(),
        b"new qrow"
    );
    drop(guard);
    // The test platform does not end the app on quit, so the check is that
    // the quit that follows does not ask for a restart.
    app.dispatch(cx, qrow::ui::Quit);
    app.settle(cx);
    cx.run_until_parked();
    assert!(
        matches!(restart.try_recv(), Ok(None)),
        "A later Quit must not restart Qrow"
    );
}
