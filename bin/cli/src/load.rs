//! Reading the documents: files, directories and standard input, as the objects of `agents.vymalo.com/v1alpha1`.

use std::collections::BTreeMap;
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
    /// The document.
    pub doc: Value,
}

impl Object {
    /// `AgentService/coder`: the object as a person names it.
    pub fn label(&self) -> String {
        format!("{}/{}", self.kind, self.name)
    }
}

/// The files a path stands for: the path itself, or the `*.yaml` and `*.yml` files of a directory (not below it), in name
/// order.
fn files_of(path: &str) -> Result<Vec<PathBuf>, String> {
    let p = Path::new(path);
    let meta = std::fs::metadata(p).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_dir() {
        return Ok(vec![p.to_path_buf()]);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(p)
        .map_err(|e| format!("{path}: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|f| f.is_file() && f.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    files.sort();
    Ok(files)
}

/// Every object of `paths` (`-` is standard input).
///
/// # Errors
///
/// What is a usage or read error, as a sentence: a path that cannot be read, a file that is not YAML, a document that is not an
/// object of one of the four kinds, an object with no name, two objects of one kind, namespace and name, and nothing at all to
/// read.
pub fn load(paths: &[String], stdin: &mut dyn Read) -> Result<Vec<Object>, String> {
    let mut sources: Vec<(String, String)> = Vec::new();
    for path in paths {
        if path == "-" {
            let mut text = String::new();
            stdin
                .read_to_string(&mut text)
                .map_err(|e| format!("<stdin>: {e}"))?;
            sources.push(("<stdin>".to_owned(), text));
            continue;
        }
        for file in files_of(path)? {
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            sources.push((file.display().to_string(), text));
        }
    }

    let mut objects: Vec<Object> = Vec::new();
    let mut seen: BTreeMap<(String, String, String), String> = BTreeMap::new();
    for (file, text) in sources {
        for (n, document) in serde_yaml::Deserializer::from_str(&text).enumerate() {
            let doc = Value::deserialize(document).map_err(|e| format!("{file}: not YAML: {e}"))?;
            if doc.is_null() {
                continue; // an empty document, or one of only comments
            }
            let at = format!("{file}: document {}", n + 1);
            let kind = doc["kind"].as_str().unwrap_or_default();
            let Some(kind) = KINDS.iter().find(|k| **k == kind) else {
                return Err(format!(
                    "{at}: kind {kind:?} is not one of {}",
                    KINDS.join(", ")
                ));
            };
            if doc["apiVersion"] != API_VERSION {
                return Err(format!("{at}: {kind} must have apiVersion {API_VERSION}"));
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
                name,
                doc,
            });
        }
    }
    if objects.is_empty() {
        return Err("nothing to read: no document of AgentService, AgentConfig, ModelEndpoint or ToolProvider was given".to_owned());
    }
    Ok(objects)
}
