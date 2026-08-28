//! Bounded local navigator projections and presentation state.
//!
//! This module deliberately deals in display metadata and explicit action
//! revisions. It never reads provider turns, terminal screens, prompts, or
//! provider payloads.

pub(crate) mod d17;
mod d17_controller;

pub(crate) use d17_controller::{
    D17NavigatorError, ManagedAction, apply_managed_action, materialize_initial_provisional_shell,
    run_d17_navigator,
};
