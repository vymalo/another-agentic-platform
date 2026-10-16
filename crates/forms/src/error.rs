//! What a form says is wrong, and where.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A mistake in a form, with **the field of the form** it is about: what the Platform API's `422`
/// carries ("each error with its form field", §60a). The field is the path of the form as the client
/// sent it: `name`, `tags[2]`, `model.endpoint`, `agent.github.owners[0]`, `headers[Authorization].secret`;
/// a field of the deployment's defaults is `deployment.…` (the person cannot edit it: it is read-only
/// on the form). An empty field is the form as a whole.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FormError {
    /// The form field.
    pub field: String,
    /// What is wrong, and what to do about it. Never a secret value (a form has none).
    pub message: String,
}

impl FormError {
    pub(crate) fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for FormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.field, self.message)
        }
    }
}

/// Collects errors; the same field and message once.
#[derive(Debug, Default)]
pub(crate) struct Errors(pub(crate) Vec<FormError>);

impl Errors {
    pub(crate) fn add(&mut self, field: impl Into<String>, message: impl Into<String>) {
        let e = FormError::new(field, message);
        if !self.0.contains(&e) {
            self.0.push(e);
        }
    }

    pub(crate) fn has(&self, field: &str) -> bool {
        self.0.iter().any(|e| e.field == field)
    }
}
