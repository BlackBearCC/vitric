use std::fmt;

use serde::Serialize;

/// One validation error: path is precise down to the field, with an error code and a fix hint.
/// After serialization it goes directly into the control-plane response — this is the structure the AI gets.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidationError {
    /// Stable error code, e.g. "VD003". Documentation is indexed by code.
    pub code: &'static str,
    /// Error location, e.g. "scenes/main.json#/entities/2/components/Sprite/image".
    pub path: String,
    /// What went wrong.
    pub message: String,
    /// How to fix it.
    pub hint: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}（{}）", self.code, self.path, self.message, self.hint)
    }
}

/// The full result of one validation pass. Validation does not stop at the first error — the AI gets all problems at once.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
}

impl ValidationReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn push(
        &mut self,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.errors.push(ValidationError {
            code,
            path: path.into(),
            message: message.into(),
            hint: hint.into(),
        });
    }

    pub fn merge(&mut self, other: ValidationReport) {
        self.errors.extend(other.errors);
    }

    /// Turn into Err; if there are no errors, hand back the passed-in value.
    pub fn into_result<T>(self, value: T) -> Result<T, ValidationReport> {
        if self.ok() {
            Ok(value)
        } else {
            Err(self)
        }
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "校验发现 {} 个问题:", self.errors.len())?;
        for e in &self.errors {
            writeln!(f, "  {e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationReport {}
