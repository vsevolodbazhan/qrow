//! The dbt manifest parser on synthetic manifests with the shapes of dbt-core
//! and dbt Fusion.
use crate::dbt_manifest::{self, Shape};
use qrow::dbt::{self, Kind, saved};

fn small() -> Shape {
    Shape {
        models: 60,
        sources: 12,
        macros: 8,
        columns: 6,
        compiled: true,
        fusion: false,
    }
}

fn check(shape: &Shape) -> dbt::Index {
    let bytes = dbt_manifest::generate(shape);
    let index = dbt::parse(&bytes).unwrap();
    let count = |kind| index.entries().iter().filter(|e| e.kind == kind).count();
    assert_eq!(count(Kind::Model), shape.models);
    assert_eq!(count(Kind::Seed), dbt_manifest::seeds(shape));
    assert_eq!(count(Kind::Snapshot), dbt_manifest::snapshots(shape));
    assert_eq!(count(Kind::Source), shape.sources);
    assert_eq!(index.test_count(), dbt_manifest::generic_tests(shape));
    assert_eq!(index.has_compiled_code, shape.compiled);
    assert!(index.find("model.synthetic_lake.disabled_0000").is_none());

    for model in [0, 7, shape.models - 1] {
        let position = index.find(&dbt_manifest::model_id(model)).unwrap();
        let entry = index.entry(position);
        assert_eq!(&*entry.name, dbt_manifest::model_name(model));
        assert_eq!(&*entry.identifier, dbt_manifest::model_alias(model));
        assert_eq!(
            index.symbol(entry.schema),
            dbt_manifest::model_schema(model)
        );
        assert_eq!(entry.database, None);
        assert_eq!(entry.columns.len(), shape.columns);
        assert_eq!(entry.compiled_path.is_some(), !shape.fusion);
        let names: Vec<&str> = index
            .tests(position)
            .iter()
            .map(|test| index.symbol(test.name))
            .collect();
        assert_eq!(names, dbt_manifest::test_names(model));
        for test in index.tests(position) {
            if test.to_text.is_some() {
                let target = index.entry(test.to.unwrap());
                assert_eq!(
                    test.to_text.as_deref(),
                    Some(format!("ref('{}')", target.name).as_str())
                );
            }
        }
        // Children are the inverse of parents.
        for parent in &entry.parents {
            assert!(index.children(*parent).contains(&position));
        }
    }
    let first_source = index.find(&dbt_manifest::source_id(0)).unwrap();
    assert_eq!(index.tests(first_source).len(), 1);
    let second_source = index.find(&dbt_manifest::source_id(1)).unwrap();
    assert!(index.tests(second_source).is_empty());
    index
}

#[test]
fn a_compiled_dbt_core_manifest_parses() {
    let index = check(&small());
    assert_eq!(index.semantic_models.len(), 60 / 30 + 1);
    assert_eq!(index.metrics.len(), 60 / 15 + 1);
    assert_eq!(index.exposures.len(), 60 / 50 + 1);
    assert!(
        index
            .semantic_models
            .iter()
            .all(|model| model.entry.is_some())
    );
}

#[test]
fn a_parsed_manifest_has_no_compiled_sql() {
    let index = check(&small().parsed());
    assert!(index.entries().iter().all(|e| e.compiled_code.is_none()));
}

#[test]
fn a_fusion_manifest_parses() {
    let index = check(&small().fusion());
    // Fusion writes no legacy semantic models or metrics.
    assert!(index.semantic_models.is_empty());
    assert!(index.metrics.is_empty());
}

#[test]
fn spans_of_a_generated_manifest_read_its_sql() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("manifest.json");
    let bytes = dbt_manifest::generate(&small());
    std::fs::write(&path, &bytes).unwrap();
    let index = dbt::parse(&bytes).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for model in [0, 33] {
        let id = dbt_manifest::model_id(model);
        let entry = index.entry(index.find(&id).unwrap());
        let node = &manifest["nodes"][&id];
        assert_eq!(
            dbt::read_sql(&path, entry.raw_code.unwrap()).unwrap(),
            node["raw_code"].as_str().unwrap()
        );
        assert_eq!(
            dbt::read_sql(&path, entry.compiled_code.unwrap()).unwrap(),
            node["compiled_code"].as_str().unwrap()
        );
    }
}

#[test]
fn a_saved_index_of_a_generated_manifest_loads_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("manifest.json");
    let bytes = dbt_manifest::generate(&small());
    std::fs::write(&path, &bytes).unwrap();
    let saved = saved::Saved {
        stamp: saved::Stamp::of(&path).unwrap(),
        index: dbt::parse(&bytes).unwrap(),
    };
    assert_eq!(saved.stamp.len, bytes.len() as u64);
    assert!(saved.stamp.modified.is_some());
    let loaded = saved::decode(&saved::encode(&saved)).unwrap();
    assert_eq!(loaded, saved);
}
