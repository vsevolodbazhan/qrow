//! Update checks and downloads against a local release server with a
//! synthetic signing key.

use super::verify::tests::{TestKey, hex, sha256};
use super::*;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};

/// Serves fixed responses over HTTP/1.1 and records the requested paths.
struct Server {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    links: Arc<Mutex<HashMap<String, String>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let links: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let (served, recorded, linked) = (files.clone(), requests.clone(), links.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap() <= 2 {
                        break;
                    }
                }
                recorded.lock().unwrap().push(path.clone());
                let body = served.lock().unwrap().get(&path).cloned();
                let (status, body) = match body {
                    Some(body) => ("200 OK", body),
                    None => ("404 Not Found", Vec::new()),
                };
                let link = linked
                    .lock()
                    .unwrap()
                    .get(&path)
                    .map(|link| format!("Link: {link}\r\n"))
                    .unwrap_or_default();
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{link}Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Self {
            base,
            files,
            links,
            requests,
        }
    }

    fn serve(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.files.lock().unwrap().insert(path.into(), body.into());
    }

    fn link(&self, path: &str, link: String) {
        self.links.lock().unwrap().insert(path.into(), link);
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// A release server with release `tag` that has `package` as its asset.
struct Fixture {
    server: Server,
    key: TestKey,
    updater: Updater,
    downloads: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let server = Server::start();
        let key = TestKey::new();
        let source = Source {
            releases: format!("{}/releases", server.base),
            downloads: format!("{}/download", server.base),
            public_key: key.public.clone(),
            https_only: false,
        };
        let updater = Updater::new(source, &crate::tls::Trust::system()).unwrap();
        let downloads = tempfile::tempdir().unwrap();
        Self {
            server,
            key,
            updater,
            downloads,
        }
    }

    /// Publishes a release with `package` named `name`. `sign_as` is the tag
    /// in the trusted comment; `listed` is the content of SHA256SUMS.
    fn publish(&self, tag: &str, name: &str, package: &[u8], sign_as: &str, listed: &[u8]) {
        let sums = format!("{}  ./{name}\n", hex(&sha256(listed)));
        let signature = self
            .key
            .sign(sums.as_bytes(), &format!("Qrow release {sign_as}"));
        let mut assets = Vec::new();
        for (asset, body) in [
            ("SHA256SUMS", sums.into_bytes()),
            ("SHA256SUMS.minisig", signature.into_bytes()),
            (name, package.to_vec()),
        ] {
            let path = format!("/download/{tag}/{asset}");
            assets.push(serde_json::json!({
                "name": asset,
                "size": body.len(),
                "browser_download_url": format!("{}{path}", self.server.base),
            }));
            self.server.serve(&path, body);
        }
        let releases = serde_json::json!([
            {"tag_name": tag, "draft": false, "prerelease": false, "assets": assets},
            {"tag_name": "v0.4.0", "draft": false, "prerelease": false, "assets": []},
        ]);
        self.server
            .serve("/releases", serde_json::to_vec(&releases).unwrap());
    }

    fn download(&self, name_arch: &str) -> Result<Download> {
        let candidate = self
            .updater
            .check(&current(), Channel::Stable)?
            .context("no update")?;
        let install = Install::Archive {
            prefix: "/unused".into(),
        };
        self.updater
            .download(&candidate, &install, name_arch, self.downloads.path())
    }
}

fn current() -> Version {
    Version::parse("0.5.0").unwrap()
}

const NAME: &str = "Qrow-0.6.0-linux-x86_64.tar.gz";

#[test]
fn downloads_and_verifies_the_package_of_a_newer_release() {
    let fixture = Fixture::new();
    fixture.publish("v0.6.0", NAME, b"package", "v0.6.0", b"package");
    let download = fixture.download("x86_64").unwrap();
    assert_eq!(download.version, Version::parse("0.6.0").unwrap());
    assert_eq!(std::fs::read(&download.path).unwrap(), b"package");
    let requests = fixture.server.requests();
    assert_eq!(requests[0], "/releases");
    assert!(requests[1..].contains(&format!("/download/v0.6.0/{NAME}")));
}

#[test]
fn reports_no_update_when_the_newest_release_is_running() {
    let fixture = Fixture::new();
    fixture.publish(
        "v0.5.0",
        "Qrow-0.5.0-linux-x86_64.tar.gz",
        b"p",
        "v0.5.0",
        b"p",
    );
    assert_eq!(
        fixture.updater.check(&current(), Channel::Stable).unwrap(),
        None
    );
}

#[test]
fn reads_later_release_pages_until_the_channel_appears() {
    let fixture = Fixture::new();
    fixture.publish("v0.6.0", NAME, b"package", "v0.6.0", b"package");
    let stable = fixture.server.files.lock().unwrap()["/releases"].clone();
    fixture.server.serve("/releases?page=2", stable);
    let nightlies: Vec<_> = (1..=3)
        .map(|run| {
            serde_json::json!({"tag_name": format!("v0.7.0-nightly.20261112.{run}"),
                "draft": false, "prerelease": true, "assets": []})
        })
        .collect();
    fixture
        .server
        .serve("/releases", serde_json::to_vec(&nightlies).unwrap());
    let base = &fixture.server.base;
    fixture.server.link(
        "/releases",
        format!(r#"<{base}/releases?page=2>; rel="next", <{base}/releases?page=2>; rel="last""#),
    );
    let candidate = fixture
        .updater
        .check(&current(), Channel::Stable)
        .unwrap()
        .unwrap();
    assert_eq!(candidate.tag(), "v0.6.0");
    let nightly = Version::parse("0.6.0-nightly.20261101.1").unwrap();
    let candidate = fixture
        .updater
        .check(&nightly, Channel::Nightly)
        .unwrap()
        .unwrap();
    assert_eq!(candidate.tag(), "v0.7.0-nightly.20261112.3");
    // A nightly build stops at the first page, which has its channel.
    let requests = fixture.server.requests();
    assert_eq!(requests, ["/releases", "/releases?page=2", "/releases"]);
}

#[test]
fn rejects_a_package_that_does_not_match_the_signed_checksum() {
    let fixture = Fixture::new();
    fixture.publish("v0.6.0", NAME, b"changed package", "v0.6.0", b"package");
    let error = fixture.download("x86_64").unwrap_err();
    assert_eq!(
        error.to_string(),
        "The update package does not match its signed checksum"
    );
}

#[test]
fn rejects_checksums_signed_for_another_release() {
    let fixture = Fixture::new();
    fixture.publish("v0.6.0", NAME, b"package", "v0.5.0", b"package");
    let error = fixture.download("x86_64").unwrap_err();
    assert_eq!(
        error.to_string(),
        "The release signature is for another release"
    );
    // Qrow does not download the package before the checksums verify.
    assert!(
        !fixture
            .server
            .requests()
            .contains(&format!("/download/v0.6.0/{NAME}"))
    );
}

#[test]
fn rejects_checksums_signed_with_another_key() {
    let fixture = Fixture::new();
    fixture.publish("v0.6.0", NAME, b"package", "v0.6.0", b"package");
    let other = TestKey::new();
    let sums = format!("{}  ./{NAME}\n", hex(&sha256(b"package")));
    fixture.server.serve(
        "/download/v0.6.0/SHA256SUMS.minisig",
        other.sign(sums.as_bytes(), "Qrow release v0.6.0"),
    );
    assert!(fixture.download("x86_64").is_err());
}

#[test]
fn reports_a_missing_package_for_the_architecture() {
    let fixture = Fixture::new();
    fixture.publish("v0.6.0", NAME, b"package", "v0.6.0", b"package");
    let error = fixture.download("aarch64").unwrap_err();
    assert_eq!(
        error.to_string(),
        "Release v0.6.0 has no Qrow-0.6.0-linux-aarch64.tar.gz"
    );
}

#[test]
fn refuses_plain_http_for_the_public_source() {
    let mut source = Source::github();
    source.releases = "http://127.0.0.1:9/releases".into();
    let updater = Updater::new(source, &crate::tls::Trust::system()).unwrap();
    assert!(updater.check(&current(), Channel::Stable).is_err());
}

#[test]
fn the_public_source_uses_the_release_key() {
    let source = Source::github();
    assert_eq!(source.public_key, verify::RELEASE_PUBLIC_KEY);
    assert!(source.https_only);
    assert!(
        source
            .releases
            .starts_with("https://api.github.com/repos/vsevolodbazhan/qrow/")
    );
}

#[test]
fn the_build_channel_follows_the_release_version() {
    // Local builds have no channel. Release validation builds have one.
    match build_channel() {
        None => assert_eq!(running_version(), None),
        Some(channel) => assert_eq!(running_version().map(|v| v.channel()), Some(channel)),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn installs_a_verified_linux_archive() {
    let fixture = Fixture::new();
    let package = archive::tests::release_archive(b"new qrow");
    fixture.publish("v0.6.0", NAME, &package, "v0.6.0", &package);
    let download = fixture.download("x86_64").unwrap();
    let prefix = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(prefix.path().join("bin")).unwrap();
    std::fs::write(prefix.path().join("bin/qrow"), b"old qrow").unwrap();
    let finish = apply(
        &download,
        &Install::Archive {
            prefix: prefix.path().into(),
        },
    )
    .unwrap();
    assert!(matches!(finish, Finish::Restart));
    assert_eq!(
        std::fs::read(prefix.path().join("bin/qrow")).unwrap(),
        b"new qrow"
    );
}

#[test]
fn managed_copies_do_not_apply_updates() {
    let folder = tempfile::tempdir().unwrap();
    let download = Download {
        version: current(),
        path: folder.path().join("x"),
        _folder: folder,
    };
    let error = apply(&download, &Install::Managed(Manager::Snap)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Another program updates this copy of Qrow"
    );
}

#[test]
fn the_windows_installer_runs_silently_into_the_installed_folder() {
    let setup =
        Path::new("C:/Users/me/AppData/Local/Qrow/updates/Qrow-0.6.0-windows-x86_64-setup.exe");
    let folder = Path::new("C:/Program Files/Qrow");
    for (all_users, scope) in [(false, "/CURRENTUSER"), (true, "/ALLUSERS")] {
        let command = installer_command(setup, folder, all_users);
        assert_eq!(command.get_program(), setup.as_os_str());
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            arguments,
            [
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/CLOSEAPPLICATIONS",
                "/QROWRELAUNCH=1",
                scope,
                "/DIR=C:/Program Files/Qrow",
            ]
        );
    }
}

#[test]
fn the_windows_installer_gets_the_folder_without_the_verbatim_prefix() {
    // Windows `canonicalize` returns verbatim paths. Inno Setup rejects them
    // and quits without installing.
    let setup = Path::new(r"C:\Users\me\AppData\Local\Qrow\updates\setup.exe");
    for (folder, expected) in [
        (
            r"\\?\C:\Users\me\AppData\Local\Programs\Qrow",
            r"/DIR=C:\Users\me\AppData\Local\Programs\Qrow",
        ),
        (r"\\?\UNC\server\share\Qrow", r"/DIR=\\server\share\Qrow"),
        (r"C:\Program Files\Qrow", r"/DIR=C:\Program Files\Qrow"),
        // A verbatim path without a drive letter has no plain form.
        (r"\\?\Volume{0b1e}\Qrow", r"/DIR=\\?\Volume{0b1e}\Qrow"),
    ] {
        let command = installer_command(setup, Path::new(folder), false);
        let last = command.get_args().last().unwrap().to_string_lossy();
        assert_eq!(last, expected, "{folder}");
    }
}

#[test]
fn each_platform_installs_its_own_kind_of_copy() {
    let setup = |install| Setup {
        source: Source::github(),
        current: current(),
        install,
        arch: "x86_64".into(),
        downloads: "/unused".into(),
    };
    let archive = setup(Install::Archive {
        prefix: "/p".into(),
    });
    let bundle = setup(Install::AppBundle {
        app: "/A/Qrow.app".into(),
    });
    let installer = setup(Install::WindowsInstaller {
        folder: "C:/Q".into(),
    });
    assert_eq!(archive.installs_updates(), cfg!(target_os = "linux"));
    assert_eq!(bundle.installs_updates(), cfg!(target_os = "macos"));
    assert_eq!(installer.installs_updates(), cfg!(windows));
    assert!(!setup(Install::Managed(Manager::WindowsPortable)).installs_updates());
}
