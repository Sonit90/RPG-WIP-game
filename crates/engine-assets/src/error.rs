//! What can go wrong between a source asset and a usable model.
//!
//! Every variant names the model and the thing inside it that is wrong, because the useful
//! question about a broken asset is never "did it fail" but "which one, and where". A
//! message that says "invalid model" costs an artist an afternoon.

use std::fmt;

/// A model that could not be imported, or could not be trusted once it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetError {
    /// The bytes are not a glTF/GLB document, or the document is malformed.
    Malformed {
        /// The asset being read.
        model: String,
        /// What the parser said.
        reason: String,
    },
    /// The document is well-formed but uses something this importer does not implement.
    Unsupported {
        /// The asset being read.
        model: String,
        /// The feature that was used.
        feature: String,
    },
    /// A model failed a structural check: a dangling index, a missing texture, bad weights.
    Invalid {
        /// The asset being checked.
        model: String,
        /// Where inside it — a submesh, a bone, a clip.
        location: String,
        /// What is wrong.
        reason: String,
    },
    /// A model is structurally fine but past a budget the project sets.
    OverBudget {
        /// The asset being checked.
        model: String,
        /// Which budget.
        limit: String,
        /// What it measured.
        measured: u64,
        /// What it was allowed.
        allowed: u64,
    },
    /// A [`crate::ModelId`] that names nothing in the library.
    NotFound {
        /// The identifier that was looked up.
        model: String,
    },
}

impl fmt::Display for AssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { model, reason } => {
                write!(formatter, "model '{model}' is malformed: {reason}")
            }
            Self::Unsupported { model, feature } => {
                write!(formatter, "model '{model}' uses unsupported {feature}")
            }
            Self::Invalid {
                model,
                location,
                reason,
            } => write!(formatter, "model '{model}' at {location}: {reason}"),
            Self::OverBudget {
                model,
                limit,
                measured,
                allowed,
            } => write!(
                formatter,
                "model '{model}' exceeds the {limit} budget: {measured} > {allowed}"
            ),
            Self::NotFound { model } => write!(formatter, "no model named '{model}'"),
        }
    }
}

impl std::error::Error for AssetError {}

impl AssetError {
    /// A structural failure inside `model`, at `location`.
    pub fn invalid(
        model: impl Into<String>,
        location: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::Invalid {
            model: model.into(),
            location: location.into(),
            reason: reason.into(),
        }
    }

    /// A budget failure.
    #[must_use]
    pub fn over_budget(
        model: impl Into<String>,
        limit: impl Into<String>,
        measured: u64,
        allowed: u64,
    ) -> Self {
        Self::OverBudget {
            model: model.into(),
            limit: limit.into(),
            measured,
            allowed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_names_the_model_and_the_place_inside_it() {
        // The point of the whole enum: "invalid model" is not a diagnostic.
        let error = AssetError::invalid("fire_mage", "submesh 2 'robe'", "index 991 of 640");
        let text = error.to_string();
        assert!(text.contains("fire_mage"), "{text}");
        assert!(text.contains("submesh 2 'robe'"), "{text}");
        assert!(text.contains("991"), "{text}");
    }

    #[test]
    fn a_budget_failure_reports_both_numbers() {
        let error = AssetError::over_budget("training_dummy", "triangle", 90_000, 20_000);
        let text = error.to_string();
        assert!(text.contains("90000") && text.contains("20000"), "{text}");
    }
}
