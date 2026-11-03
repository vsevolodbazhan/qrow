use super::*;

/// Run each worker step directly, without a watcher thread or timer waits.
struct Fixture {
    _folder: tempfile::TempDir,
    manifest: PathBuf,
    runner: Runner,
    events: mpsc::Receiver<Event>,
}

impl Fixture {
    fn new() -> Self {
        let folder = tempfile::tempdir().unwrap();
        let manifest = folder.path().join("manifest.json");
        let (_, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let runner = Runner {
            directory: None,
            manifests: HashMap::new(),
            watcher: None,
            watched: HashSet::new(),
            due: HashMap::new(),
            settle: SETTLE,
            rx,
            tx,
            wake: Arc::new(|| {}),
        };
        let fixture = Self {
            _folder: folder,
            manifest,
            runner,
            events,
        };
        fixture.write("original");
        fixture
    }

    fn write(&self, project: &str) {
        let manifest = serde_json::json!({
            "metadata": {
                "dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v12.json",
                "project_name": project,
            },
            "nodes": {},
        });
        fs::write(&self.manifest, manifest.to_string()).unwrap();
    }

    fn configure(&mut self, automatic: bool) {
        self.runner.configure(vec![Use {
            manifest: self.manifest.clone(),
            automatic,
        }]);
    }

    fn state(&self) -> &Arc<ManifestState> {
        &self.runner.manifests[&self.manifest].state
    }

    fn make_due(&mut self) {
        self.runner.due.get_mut(&self.manifest).unwrap().at = Instant::now();
    }
}

#[test]
fn a_switch_to_manual_drops_a_waiting_automatic_refresh() {
    let mut fixture = Fixture::new();
    fixture.configure(true);
    fixture.runner.refresh_due();
    let original = fixture.state().clone();
    assert_eq!(
        original.index.as_ref().unwrap().project.as_ref(),
        "original"
    );
    assert!(original.is_current());
    fixture.events.try_iter().for_each(drop);

    fixture.write("changed project");
    assert!(!original.is_current());
    fixture.runner.changed(vec![fixture.manifest.clone()]);
    assert!(!fixture.runner.due[&fixture.manifest].forced);

    // The change is pending even if its deadline passes before Configure.
    fixture.make_due();
    fixture.configure(false);
    fixture.runner.refresh_due();
    assert!(!fixture.runner.due.contains_key(&fixture.manifest));
    assert!(Arc::ptr_eq(fixture.state(), &original));
    assert!(fixture.events.try_recv().is_err());

    // A watcher event delivered after Configure cannot queue another parse.
    fixture.runner.changed(vec![fixture.manifest.clone()]);
    fixture.runner.refresh_due();
    assert!(!fixture.runner.due.contains_key(&fixture.manifest));
    assert!(Arc::ptr_eq(fixture.state(), &original));
    assert!(fixture.events.try_recv().is_err());
}

#[test]
fn a_switch_to_manual_keeps_a_requested_refresh() {
    let mut fixture = Fixture::new();
    fixture.configure(true);
    // Opening a manifest requests its first parse, also in Manual mode.
    assert!(fixture.runner.due[&fixture.manifest].forced);
    fixture.write("changed project");
    fixture.runner.changed(vec![fixture.manifest.clone()]);
    assert!(fixture.runner.due[&fixture.manifest].forced);
    fixture.configure(false);
    assert!(fixture.runner.due[&fixture.manifest].forced);

    fixture.make_due();
    fixture.runner.refresh_due();
    let state = fixture.state();
    assert_eq!(
        state.index.as_ref().unwrap().project.as_ref(),
        "changed project"
    );
    assert!(state.is_current());
    assert!(state.error.is_none());
    assert!(!state.parsing);
    assert!(fixture.events.try_iter().any(|event| matches!(
        event,
        Event::Log {
            severity: Severity::Info,
            ..
        }
    )));
}
