//! The dbt worker: saved indexes, refreshes, and the watch of manifests.
use crate::dbt_manifest::{self, Shape};
use qrow::{
    dbt::{
        Kind,
        saved::{self, Stamp},
        worker::{DbtWorker, Event, ManifestError, ManifestState, Use, manifest_key, saved_path},
    },
    logs::Severity,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const SETTLE: Duration = Duration::from_millis(150);
const TIMEOUT: Duration = Duration::from_secs(10);

fn shape(models: usize) -> Shape {
    Shape {
        models,
        sources: 3,
        macros: 2,
        columns: 3,
        compiled: false,
        fusion: false,
    }
}

fn models(state: &ManifestState) -> usize {
    state.index.as_ref().map_or(0, |index| {
        index
            .entries()
            .iter()
            .filter(|entry| entry.kind == Kind::Model)
            .count()
    })
}

/// A worker, its events, and a project folder with a manifest.
struct Fixture {
    _folder: tempfile::TempDir,
    data: PathBuf,
    manifest: PathBuf,
    worker: DbtWorker,
    states: Vec<Arc<ManifestState>>,
    logs: Vec<(Severity, String)>,
}

impl Fixture {
    fn new() -> Self {
        let folder = tempfile::tempdir().unwrap();
        let data = folder.path().join("data/dbt");
        let target = folder.path().join("project/target");
        std::fs::create_dir_all(&target).unwrap();
        let manifest = target.join("manifest.json");
        std::fs::write(&manifest, dbt_manifest::generate(&shape(10))).unwrap();
        let worker = Self::worker(&data);
        Self {
            _folder: folder,
            data,
            manifest,
            worker,
            states: vec![],
            logs: vec![],
        }
    }

    fn worker(data: &Path) -> DbtWorker {
        DbtWorker::with_settle(Some(data.to_owned()), Arc::new(|| {}), SETTLE)
    }

    fn use_manifest(&self, automatic: bool) -> Use {
        Use {
            manifest: self.manifest.clone(),
            automatic,
        }
    }

    fn drain(&mut self) {
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::State(state) => self.states.push(state),
                Event::Log { severity, text, .. } => self.logs.push((severity, text)),
            }
        }
    }

    /// Wait until the newest state passes `check`.
    fn wait(&mut self, what: &str, check: impl Fn(&ManifestState) -> bool) -> Arc<ManifestState> {
        let started = Instant::now();
        loop {
            self.drain();
            if let Some(state) = self.states.last()
                && check(state)
            {
                return state.clone();
            }
            assert!(started.elapsed() < TIMEOUT, "{what}: {:?}", self.logs);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Let events arrive for a while, and return the logs.
    fn settle(&mut self) -> Vec<(Severity, String)> {
        std::thread::sleep(SETTLE * 4);
        self.drain();
        std::mem::take(&mut self.logs)
    }

    fn write(&self, models: usize) {
        std::fs::write(&self.manifest, dbt_manifest::generate(&shape(models))).unwrap();
    }
}

#[test]
fn a_new_manifest_is_parsed_and_saved_and_a_launch_loads_it() {
    let mut fixture = Fixture::new();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    let state = fixture.wait("the parse", |state| models(state) == 10);
    assert_eq!(state.path, manifest_key(&fixture.manifest));
    assert!(state.is_current());
    assert!(state.error.is_none());
    let logs = fixture.settle();
    let counts = format!(
        "10 models, 1 seed, 1 snapshot, 3 sources, {} tests",
        dbt_manifest::generic_tests(&shape(10))
    );
    assert!(
        logs.iter()
            .any(|(severity, text)| *severity == Severity::Info
                && text.starts_with("dbt manifest refreshed in")
                && text.ends_with(&counts)),
        "{logs:?}"
    );
    let file = saved_path(&fixture.data, &state.path);
    let saved = saved::decode(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved.manifest, state.path);
    assert_eq!(Some(saved.stamp), state.stamp);

    // The next launch loads the saved index without a parse.
    fixture.worker = Fixture::worker(&fixture.data);
    fixture.states.clear();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    let state = fixture.wait("the load", |state| models(state) == 10);
    assert!(!state.parsing);
    assert!(fixture.settle().is_empty());
    assert!(fixture.states.iter().all(|state| !state.parsing));
}

#[test]
fn a_saved_index_that_does_not_agree_is_made_again() {
    let mut fixture = Fixture::new();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    let state = fixture.wait("the parse", |state| models(state) == 10);
    let file = saved_path(&fixture.data, &state.path);
    drop(std::mem::replace(
        &mut fixture.worker,
        Fixture::worker(&fixture.data),
    ));

    // A manual manifest that changed while Qrow was closed.
    fixture.write(12);
    fixture.states.clear();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    fixture.wait("the parse after a change", |state| models(state) == 12);

    // A damaged saved index, or one of another format version.
    for damage in [
        |bytes: &mut Vec<u8>| *bytes.last_mut().unwrap() ^= 1,
        |bytes: &mut Vec<u8>| bytes[8] = bytes[8].wrapping_add(1),
    ] {
        fixture.worker = Fixture::worker(&fixture.data);
        let mut bytes = std::fs::read(&file).unwrap();
        damage(&mut bytes);
        std::fs::write(&file, bytes).unwrap();
        fixture.states.clear();
        fixture.worker.configure(vec![fixture.use_manifest(false)]);
        fixture.wait("the parse of a damaged index", |state| {
            models(state) == 12 && !state.parsing
        });
        assert!(saved::decode(&std::fs::read(&file).unwrap()).is_ok());
    }
}

#[test]
fn an_automatic_manifest_is_parsed_again_when_it_settles_after_a_change() {
    let mut fixture = Fixture::new();
    fixture.worker.configure(vec![fixture.use_manifest(true)]);
    fixture.wait("the parse", |state| models(state) == 10);
    fixture.settle();

    // dbt writes the file in parts. A part that is not valid JSON fails,
    // and the index stays.
    let full = dbt_manifest::generate(&shape(14));
    std::fs::write(&fixture.manifest, &full[..full.len() / 2]).unwrap();
    let state = fixture.wait("the failed parse", |state| {
        matches!(state.error, Some(ManifestError::Invalid(_)))
    });
    assert_eq!(models(&state), 10);
    assert!(
        fixture
            .logs
            .iter()
            .any(|(severity, text)| *severity == Severity::Error
                && text.starts_with("dbt manifest refresh failed after")),
        "{:?}",
        fixture.logs
    );
    std::fs::write(&fixture.manifest, &full).unwrap();
    let state = fixture.wait("the parse of the full file", |state| models(state) == 14);
    assert!(state.error.is_none());

    // dbt can also replace the file.
    let replacement = fixture.manifest.with_extension("tmp");
    std::fs::write(&replacement, dbt_manifest::generate(&shape(16))).unwrap();
    std::fs::rename(&replacement, &fixture.manifest).unwrap();
    fixture.wait("the parse of the new file", |state| models(state) == 16);

    // A removed manifest reports once and keeps the index.
    std::fs::remove_file(&fixture.manifest).unwrap();
    let state = fixture.wait("the missing manifest", |state| {
        state.error == Some(ManifestError::NotFound)
    });
    assert_eq!(models(&state), 16);
    assert_eq!(
        state.error.as_ref().unwrap().to_string(),
        "Manifest not found: run dbt parse in the project"
    );
}

#[test]
fn a_manual_manifest_is_parsed_only_on_refresh() {
    let mut fixture = Fixture::new();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    fixture.wait("the parse", |state| models(state) == 10);
    fixture.settle();
    fixture.write(11);
    assert!(fixture.settle().is_empty());
    assert_eq!(models(fixture.states.last().unwrap()), 10);
    fixture.worker.refresh(&fixture.manifest);
    fixture.wait("the refresh", |state| models(state) == 11);

    // A Refresh parses also an unchanged file, so that Activity shows it.
    fixture.settle();
    fixture.worker.refresh(&fixture.manifest);
    let logs = fixture.settle();
    assert_eq!(logs.len(), 1, "{logs:?}");

    // When another connection makes the shared manifest automatic, a
    // change that happened before is parsed.
    fixture.write(13);
    fixture.worker.configure(vec![
        fixture.use_manifest(false),
        fixture.use_manifest(true),
    ]);
    fixture.wait("the catch-up parse", |state| models(state) == 13);
    fixture.write(15);
    fixture.wait("the automatic parse", |state| models(state) == 15);
}

#[test]
fn a_manifest_without_its_folder_is_found_when_dbt_makes_it() {
    let mut fixture = Fixture::new();
    std::fs::remove_dir_all(fixture.manifest.parent().unwrap()).unwrap();
    fixture.worker.configure(vec![fixture.use_manifest(true)]);
    fixture.wait("the missing manifest", |state| {
        state.error == Some(ManifestError::NotFound)
    });
    std::fs::create_dir_all(fixture.manifest.parent().unwrap()).unwrap();
    fixture.write(10);
    fixture.wait("the new manifest", |state| models(state) == 10);
}

#[test]
fn an_unsupported_manifest_reports_its_version() {
    let mut fixture = Fixture::new();
    let text = String::from_utf8(dbt_manifest::generate(&shape(4))).unwrap();
    std::fs::write(
        &fixture.manifest,
        text.replace("manifest/v12", "manifest/v11"),
    )
    .unwrap();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    let state = fixture.wait("the error", |state| state.error.is_some());
    assert_eq!(
        state.error.as_ref().unwrap().to_string(),
        "Unsupported manifest version v11: Qrow reads manifest v12 (dbt 1.8 and later)"
    );
    assert!(state.index.is_none());
}

#[test]
fn saved_indexes_that_no_connection_uses_are_deleted() {
    let mut fixture = Fixture::new();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    let state = fixture.wait("the parse", |state| models(state) == 10);
    let file = saved_path(&fixture.data, &state.path);
    // A saved index of a connection that was deleted while Qrow was closed.
    let stray = fixture.data.join("0123456789abcdef0123456789abcdef.index");
    std::fs::write(&stray, b"old").unwrap();
    let other = fixture.data.join("notes.txt");
    std::fs::write(&other, b"keep").unwrap();
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    fixture.settle();
    assert!(file.exists());
    assert!(!stray.exists());
    assert!(other.exists());
    fixture.worker.configure(vec![]);
    fixture.settle();
    assert!(!file.exists());
    let _ = Stamp::of(&fixture.manifest).unwrap();
}

#[test]
fn a_switch_to_manual_drops_a_waiting_automatic_refresh() {
    let mut fixture = Fixture::new();
    fixture.worker.configure(vec![fixture.use_manifest(true)]);
    fixture.wait("the parse", |state| models(state) == 10);
    fixture.settle();
    fixture.write(12);
    // The change waits for SETTLE. Manual refresh drops it.
    std::thread::sleep(SETTLE / 3);
    fixture.worker.configure(vec![fixture.use_manifest(false)]);
    assert!(fixture.settle().is_empty());
    assert_eq!(models(fixture.states.last().unwrap()), 10);
}

#[test]
fn a_manifest_link_reads_its_current_target() {
    let mut fixture = Fixture::new();
    let folder = fixture.manifest.parent().unwrap().to_owned();
    let (first, second) = (folder.join("first.json"), folder.join("second.json"));
    std::fs::write(&first, dbt_manifest::generate(&shape(5))).unwrap();
    std::fs::write(&second, dbt_manifest::generate(&shape(7))).unwrap();
    let link = folder.join("current.json");
    std::os::unix::fs::symlink(&first, &link).unwrap();
    let uses = vec![Use {
        manifest: link.clone(),
        automatic: false,
    }];
    fixture.worker.configure(uses);
    fixture.wait("the first target", |state| models(state) == 5);
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&second, &link).unwrap();
    fixture.worker.refresh(&link);
    fixture.wait("the second target", |state| models(state) == 7);
}

#[test]
fn a_parent_part_after_a_folder_link_names_the_parent_of_its_target() {
    let mut fixture = Fixture::new();
    let root = fixture
        .manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    // current -> releases/v1, so current/../manifest.json is
    // releases/manifest.json.
    let releases = root.join("releases");
    std::fs::create_dir_all(releases.join("v1")).unwrap();
    std::fs::write(
        releases.join("manifest.json"),
        dbt_manifest::generate(&shape(6)),
    )
    .unwrap();
    std::os::unix::fs::symlink(releases.join("v1"), root.join("current")).unwrap();
    let path = root.join("current/./../manifest.json");
    assert_eq!(manifest_key(&path), root.join("current/../manifest.json"));
    fixture.worker.configure(vec![Use {
        manifest: path,
        automatic: false,
    }]);
    fixture.wait("the target", |state| models(state) == 6);
}
