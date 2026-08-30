#![forbid(unsafe_code)]

pub(crate) mod account_shell;
pub mod actions;
pub mod app;
pub(crate) mod clock;
pub mod domain;
pub mod navigator;
pub(crate) mod onboarding;
pub(crate) mod onboarding_broker;
pub(crate) mod onboarding_helper;
pub mod presentation;
mod private_tmux;
pub mod process;
pub mod provider;
pub(crate) mod provider_reconcile;
pub(crate) mod provisional;
pub mod repository;
pub(crate) mod review;
pub mod runtime;
pub(crate) mod shell_control;
pub(crate) mod shell_gate;
pub(crate) mod snapshot;
pub mod startup;
pub mod state;
