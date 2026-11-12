//! Workspace fields that this build does not know.
//!
//! A newer Qrow can save fields that an older one does not read. Without
//! care, the older one would drop them at its next save. Qrow records the
//! path and the value of each unknown field when it reads the workspace, and
//! puts them back when it writes it. List items are found again by their
//! `id`, because tabs and connections can move. A field of an item that the
//! user deleted goes with the item.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// One step from the workspace root to a field.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Key(String),
    /// A list item, by its `id` when it has one.
    Item {
        id: Option<String>,
        index: usize,
    },
}

/// The unknown fields of a workspace file, with their values.
#[derive(Clone, Debug, Default)]
pub struct Unknown {
    fields: Vec<(Vec<Step>, Value)>,
}

impl Unknown {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Reads `data` as `T` and records the fields that `T` does not have.
    /// Top-level keys in `own` belong to the file format, not to `T`.
    pub fn read<T: DeserializeOwned>(data: &[u8], own: &[&str]) -> Result<(T, Self)> {
        let original: Value = serde_json::from_slice(data)?;
        let mut paths = Vec::new();
        let mut deserializer = serde_json::Deserializer::from_slice(data);
        let value: T = serde_ignored::deserialize(&mut deserializer, |path| {
            let mut steps = Vec::new();
            raw_steps(&path, &mut steps);
            paths.push(steps);
        })?;
        let mut fields = Vec::new();
        for path in paths {
            if let [RawStep::Key(key)] = path.as_slice()
                && own.contains(&key.as_str())
            {
                continue;
            }
            if let Some(field) = locate(&original, &path) {
                fields.push(field);
            }
        }
        Ok((value, Self { fields }))
    }

    /// Puts the recorded fields back into `value`, the new form of the
    /// workspace. A field whose parent is gone stays out, and a field that
    /// this build wrote keeps the new value.
    pub fn restore(&self, value: &mut Value) {
        for (path, kept) in &self.fields {
            insert(value, path, kept);
        }
    }
}

enum RawStep {
    Key(String),
    Index(usize),
}

fn raw_steps(path: &serde_ignored::Path, steps: &mut Vec<RawStep>) {
    match path {
        serde_ignored::Path::Root => {}
        serde_ignored::Path::Seq { parent, index } => {
            raw_steps(parent, steps);
            steps.push(RawStep::Index(*index));
        }
        serde_ignored::Path::Map { parent, key } => {
            raw_steps(parent, steps);
            steps.push(RawStep::Key(key.clone()));
        }
        serde_ignored::Path::Some { parent }
        | serde_ignored::Path::NewtypeStruct { parent }
        | serde_ignored::Path::NewtypeVariant { parent } => raw_steps(parent, steps),
    }
}

/// The steps and the value of the field at `path` in `original`.
fn locate(original: &Value, path: &[RawStep]) -> Option<(Vec<Step>, Value)> {
    let mut current = original;
    let mut steps = Vec::with_capacity(path.len());
    for step in path {
        match step {
            RawStep::Key(key) => {
                current = current.get(key)?;
                steps.push(Step::Key(key.clone()));
            }
            RawStep::Index(index) => {
                current = current.get(*index)?;
                let id = current.get("id").and_then(Value::as_str).map(str::to_owned);
                steps.push(Step::Item { id, index: *index });
            }
        }
    }
    // Only an object key can be put back: a list item is not unknown.
    matches!(steps.last(), Some(Step::Key(_))).then(|| (steps, current.clone()))
}

fn insert(value: &mut Value, path: &[Step], kept: &Value) {
    let Some((Step::Key(last), parents)) = path.split_last() else {
        return;
    };
    let mut current = value;
    for step in parents {
        let next = match step {
            Step::Key(key) => current.get_mut(key),
            Step::Item { id: Some(id), .. } => current.as_array_mut().and_then(|items| {
                items
                    .iter_mut()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
            }),
            // Items without an id stay at their position.
            Step::Item { id: None, index } => current.get_mut(*index),
        };
        let Some(next) = next else {
            return;
        };
        current = next;
    }
    if let Some(object) = current.as_object_mut() {
        object.entry(last.clone()).or_insert_with(|| kept.clone());
    }
}

/// Writes a copy of `data` next to `path` as `NAME.backup.json`, readable only
/// by the owner, and returns its path.
pub fn back_up(path: &std::path::Path, data: &[u8]) -> Result<std::path::PathBuf> {
    let stem = path
        .file_stem()
        .context("Invalid workspace path")?
        .to_string_lossy();
    let backup = path.with_file_name(format!("{stem}.backup.json"));
    super::write_atomically(&backup, data).context("Could not back up the workspace")?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Item {
        id: String,
        name: String,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Root {
        size: f32,
        items: Vec<Item>,
        #[serde(default)]
        nested: Nested,
    }

    #[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
    struct Nested {
        on: bool,
    }

    const NEWER: &str = r#"{
        "size": 13.5,
        "future": {"a": [1, 2]},
        "written_by": "9.9.9",
        "items": [
            {"id": "one", "name": "One", "color": "red"},
            {"id": "two", "name": "Two", "color": "blue"}
        ],
        "nested": {"on": true, "level": 3}
    }"#;

    fn read() -> (Root, Unknown) {
        Unknown::read(NEWER.as_bytes(), &["written_by"]).unwrap()
    }

    #[test]
    fn keeps_unknown_fields_through_a_save() {
        let (root, unknown) = read();
        assert_eq!(root.size, 13.5);
        let mut value = serde_json::to_value(&root).unwrap();
        unknown.restore(&mut value);
        assert_eq!(value["future"], serde_json::json!({"a": [1, 2]}));
        assert_eq!(value["items"][0]["color"], "red");
        assert_eq!(value["items"][1]["color"], "blue");
        assert_eq!(value["nested"]["level"], 3);
        // The format's own key is not a field of the workspace.
        assert!(value.get("written_by").is_none());
    }

    #[test]
    fn finds_moved_items_by_id_and_drops_deleted_ones() {
        let (mut root, unknown) = read();
        root.items.reverse();
        root.items.retain(|item| item.id == "two");
        let mut value = serde_json::to_value(&root).unwrap();
        unknown.restore(&mut value);
        assert_eq!(value["items"].as_array().unwrap().len(), 1);
        assert_eq!(value["items"][0]["color"], "blue");
    }

    #[test]
    fn keeps_new_values_of_known_fields() {
        let (mut root, unknown) = read();
        root.size = 20.;
        root.items[0].name = "Renamed".into();
        let mut value = serde_json::to_value(&root).unwrap();
        unknown.restore(&mut value);
        assert_eq!(value["size"].as_f64(), Some(20.));
        assert_eq!(value["items"][0]["name"], "Renamed");
    }

    #[test]
    fn a_file_of_this_build_has_no_unknown_fields() {
        let data = br#"{"size": 1.0, "items": [], "nested": {"on": false}, "written_by": "1.0.0"}"#;
        let (_, unknown) = Unknown::read::<Root>(data, &["written_by"]).unwrap();
        assert!(unknown.is_empty());
    }
}
