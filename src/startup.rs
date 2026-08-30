//! Current-only state startup classification.
//!
//! Startup delegates all root and schema authority to the schema-15 bootstrap
//! boundary. It never opens a historical database, inspects legacy
//! presentation state, or invokes a migration/reset workflow.

use thiserror::Error;

use crate::state::{
    CurrentState, FreshRootClassification, StateError, StateRoot,
    current::{CurrentRootClassification, classify_current_root},
};

/// The only outcomes of current startup classification.
pub enum StartupAssessment {
    Current(Box<CurrentState>),
    Fresh(FreshRootClassification),
}

/// Typed current startup failures. Operator-facing rendering belongs to the
/// CLI/application boundary; this type carries no path or provider payload.
#[derive(Debug, Error)]
pub enum StartupError {
    #[error(transparent)]
    State(#[from] StateError),
}

/// Opens the exact current root or reports that an absent/private-empty root
/// needs direct schema-15 creation. No legacy route is consulted.
///
/// # Errors
///
/// Returns [`StartupError`] when the root is not an exact current state or
/// when the state boundary cannot classify/open it safely.
pub fn assess_current_startup(root: &StateRoot) -> Result<StartupAssessment, StartupError> {
    match crate::state::open_current(root) {
        Ok(state) => Ok(StartupAssessment::Current(Box::new(state))),
        Err(StateError::FreshStateRequired) => {
            let classification = classify_current_root(root.base())?;
            match classification {
                CurrentRootClassification::Absent => {
                    Ok(StartupAssessment::Fresh(FreshRootClassification::Absent))
                }
                CurrentRootClassification::Empty => {
                    Ok(StartupAssessment::Fresh(FreshRootClassification::Empty))
                }
                CurrentRootClassification::NonEmpty => Err(StateError::FreshStateRequired.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}
