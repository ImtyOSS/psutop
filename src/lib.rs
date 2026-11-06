//! psutop: an nvtop-style monitor for the per-wire current of the 12V-2x6 GPU power cables on
//! MSI MPG Ai1600TS / Ai1300TS PSUs. Built on code copied from MeltAlarm (MIT, © 2026 Denis
//! Domanskii) into `third_party/meltalarm`: `meltalarm-core` decides every level and alarm,
//! `meltalarm-source-msi` talks to the PSU.

pub mod config;
pub mod doctor;
pub mod frame;
pub mod history;
pub mod monitor;
pub mod output;
pub mod sim;
pub mod source;
pub mod ui;
