//! Bounded local navigator projections and presentation state.
//!
//! This module deliberately deals in display metadata and explicit action
//! revisions. It never reads provider turns, terminal screens, prompts, or
//! provider payloads.

mod d16;
mod d16_controller;
pub(crate) mod d17;
mod d17_controller;

pub use d16::{
    D16Command, D16ListGeometry, D16LocationRow, D16Modal, D16Model, D16Navigator, D16OperationRow,
    D16Page, D16ProjectBrowser, D16ProjectHeader, D16ProviderChooser, D16ProviderRequest, D16Row,
    D16RowId, D16WorkstreamRow,
};
pub use d16_controller::{D16NavigatorError, run_local_navigator};
pub(crate) use d17_controller::{D17NavigatorError, run_d17_navigator};
