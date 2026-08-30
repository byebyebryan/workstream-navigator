//! Bounded local navigator projections and presentation state.
//!
//! This module deliberately deals in display metadata and explicit action
//! revisions. It never reads provider turns, terminal screens, prompts, or
//! provider payloads.

mod controller;
pub(crate) mod view;

pub(crate) use controller::{
    ManagedAction, NavigatorError, apply_managed_action, materialize_initial_provisional_shell,
    run_navigator,
};
