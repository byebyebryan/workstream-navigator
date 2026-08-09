//! Bounded local navigator projections and presentation state.
//!
//! This module deliberately deals in display metadata and explicit action
//! revisions. It never reads provider turns, terminal screens, prompts, or
//! provider payloads.

mod controller;
mod model;
mod render;
mod snapshot;
mod view;

#[cfg(test)]
mod tests;

pub use controller::run_local_navigator;
pub use model::{
    LocalNavigatorSnapshot, NavigatorError, NavigatorHost, NavigatorHostOverview,
    NavigatorOperation, NavigatorRuntimeStatus, NavigatorWorkstream, RemoteHostIssue,
    RemoteHostReachability,
};
pub use snapshot::local_snapshot;
pub use view::NavigatorView;
