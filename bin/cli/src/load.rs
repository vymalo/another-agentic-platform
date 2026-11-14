//! Reading the documents: files, directories and standard input, as the objects of `agents.vymalo.com/v1alpha1`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// The kinds `aapctl` reads: the four of the platform's CRDs.
pub const KINDS: [&str; 4] = [
    "AgentService",
    "AgentConfig",
    "ModelEndpoint",
    "ToolProvider",
];

const GROUP: &str = "agents.vymalo.com";
const API_VERSION: &str = "agents.vymalo.com/v1alpha1";

/// One document, as it was read: where from, which object it is and its JSON.
#[derive(Clone, Debug)]
pub struct Object {
    /// A path, or `<stdin>`.
    pub file: String,
    /// One of [`KINDS`].
    pub kind: &'static str,
    /// `metadata.namespace`, if there is one.
    pub namespace: Option<String>,
    /// `metadata.name`.
    pub name: String,
    /// `Kind/name`, or `Kind/namespace/name` when the documents hold more than one namespace.
    pub label: String,
    /// The document, with nulls removed (an API server treats a null as absent) and no `status` (the operator writes it).
    pub doc: Value,
}

/// What was read, and what was left out with a word about it.
#[derive(Debug)]
pub struct Loaded {
    /// The objects, in the order given.
    pub objects: Vec<Object>,
    /// One line each for what was skipped, for stderr.
    pub notes: Vec<String>,
}

/// The files a path stands for: the path itself, or the `*.yaml` and `*.yml` files of a directory (not below it, any case of
/// the extension), in name order. What a directory holds under such a name that is not a regular file (a broken symlink, a
/// directory) is a note.
fn files_of(path: &str, notes: &mut Vec<String>) -> Result<(Vec<PathBuf>, bool), String> {
    let p = Path::new(path);
    let meta = std::fs::metadata(p).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_dir() {
        return Ok((vec![p.to_path_buf()], false));
    }
    let mut names: Vec<PathBuf> = std::fs::read_dir(p)
        .map_err(|e| format!("{path}: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|f| {
            f.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("yaml") || x.eq_ignore_ascii_case("yml"))
        })
        .collect();
    names.sort();
    let mut files = Vec::new();
    for f in names {
        if f.is_file() {
            files.push(f);
        } else {
            notes.push(format!(
                "{}: skipped: not a regular file (a broken symlink, or a directory)",
                f.display()
            ));
        }
    }
    Ok((files, true))
}

/// What a document is, in a few words, for a message.
fn describe(doc: &Value) -> String {
    match doc {
        Value::Object(_) => {
            let kind = doc["kind"].as_str();
            let api = doc["apiVersion"].as_str();
            match (kind, api) {
                (Some(k), Some(a)) => format!("a {k} of {a}"),
                (Some(k), None) => format!("a {k} with no apiVersion"),
                (None, Some(a)) => format!("a document of apiVersion {a} with no kind"),
                (None, None) => "a mapping with no apiVersion and no kind".to_owned(),
            }
        }
        Value::Array(_) => "a list (a YAML sequence)".to_owned(),
        _ => "a scalar".to_owned(),
    }
}

/// A document is ours when its `apiVersion` is of the group `agents.vymalo.com`.
fn in_group(doc: &Value) -> bool {
    doc["apiVersion"]
        .as_str()
        .is_some_and(|a| a.starts_with(&format!("{GROUP}/")))
}

/// What a `null` is to an API server: absent. Removed from every mapping (a null in a list stays: it is an element).
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// The documents of a text as JSON. A key that a mapping has twice is an error (`serde_yaml`'s own value type refuses it; a
/// JSON value would keep the last silently), as is a document that is not YAML.
fn documents(text: &str, file: &str) -> Result<Vec<Value>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    for (n, document) in serde_yaml::Deserializer::from_str(text).enumerate() {
        let at = format!("{file}: document {}", n + 1);
        let yaml = serde_yaml::Value::deserialize(document)
            .map_err(|e| format!("{at}: not YAML, or a key is given twice: {e}"))?;
        let json = serde_json::to_value(&yaml).map_err(|e| format!("{at}: not readable: {e}"))?;
        out.push(json);
    }
    Ok(out)
}

/// Every object of `paths` (`-` is standard input).
///
/// A path given twice (the same file once resolved) is read once. In a **directory**, a document of another group (a
/// `Deployment`, a `kustomization.yaml`) is skipped with one note per file; named as a file, or on standard input, it is an
/// error.
///
/// # Errors
///
/// What is a usage or read error, as a sentence: a path that cannot be read, a file that is not YAML or has a key twice, a
/// document of the group `agents.vymalo.com` that is not of the version and a kind we read, a foreign document that was named
/// itself, an object with no name, two objects of one kind, namespace and name, and nothing at all to read.
pub fn load(paths: &[String], stdin: &mut dyn Read) -> Result<Loaded, String> {
    let mut notes = Vec::new();
    // (file label, text, from a directory)
    let mut sources: Vec<(String, String, bool)> = Vec::new();
    let mut read_files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stdin_read = false;
    for path in paths {
        if path == "-" {
            if std::mem::replace(&mut stdin_read, true) {
                continue;
            }
            let mut text = String::new();
            stdin
                .read_to_string(&mut text)
                .map_err(|e| format!("<stdin>: {e}"))?;
            sources.push(("<stdin>".to_owned(), text, false));
            continue;
        }
        let (files, from_dir) = files_of(path, &mut notes)?;
        for file in files {
            let canonical = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
            if !read_files.insert(canonical) {
                continue;
            }
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            sources.push((file.display().to_string(), text, from_dir));
        }
    }

    let mut objects: Vec<Object> = Vec::new();
    let mut seen: BTreeMap<(String, String, String), String> = BTreeMap::new();
    for (file, text, from_dir) in sources {
        let mut skipped = false;
        for (n, mut doc) in documents(&text, &file)?.into_iter().enumerate() {
            if doc.is_null() {
                continue; // an empty document, or one of only comments
            }
            let at = format!("{file}: document {}", n + 1);
            if !in_group(&doc) {
                if from_dir {
                    skipped = true;
                    continue;
                }
                return Err(format!(
                    "{at}: {}, not an object of {API_VERSION}",
                    describe(&doc)
                ));
            }
            if doc["apiVersion"] != API_VERSION {
                return Err(format!(
                    "{at}: apiVersion {} is not {API_VERSION}, the one version there is",
                    doc["apiVersion"].as_str().unwrap_or_default()
                ));
            }
            let kind = doc["kind"].as_str().unwrap_or_default();
            let Some(kind) = KINDS.iter().find(|k| **k == kind) else {
                return Err(format!(
                    "{at}: kind {kind:?} of {GROUP} is not one of {}",
                    KINDS.join(", ")
                ));
            };
            strip_nulls(&mut doc);
            if let Some(map) = doc.as_object_mut() {
                map.remove("status");
            }
            let name = doc["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if name.is_empty() {
                return Err(format!("{at}: {kind} has no metadata.name"));
            }
            let namespace = doc["metadata"]["namespace"].as_str().map(str::to_owned);
            let key = (
                (*kind).to_owned(),
                namespace.clone().unwrap_or_default(),
                name.clone(),
            );
            if let Some(first) = seen.insert(key, file.clone()) {
                return Err(format!(
                    "{kind}/{name} appears twice (a duplicate): in {first} and in {file}"
                ));
            }
            objects.push(Object {
                file: file.clone(),
                kind,
                namespace,
                label: String::new(),
                name,
                doc,
            });
        }
        if skipped {
            notes.push(format!(
                "{file}: skipped: not an {GROUP} object (another group's document)"
            ));
        }
    }
    if objects.is_empty() {
        return Err("nothing to read: no document of AgentService, AgentConfig, ModelEndpoint or ToolProvider was given".to_owned());
    }
    let namespaces: BTreeSet<&str> = objects
        .iter()
        .map(|o| o.namespace.as_deref().unwrap_or_default())
        .collect();
    let several = namespaces.len() > 1;
    for o in &mut objects {
        o.label = match (&o.namespace, several) {
            (Some(ns), true) => format!("{}/{ns}/{}", o.kind, o.name),
            _ => format!("{}/{}", o.kind, o.name),
        };
    }
    Ok(Loaded { objects, notes })
}
