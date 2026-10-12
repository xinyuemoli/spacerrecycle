//! Cloud-backed storage integration.
//!
//! Phase A is local-only and deliberately so: OneDrive's Files On-Demand keeps
//! a placeholder on disk for every file, and releasing those placeholders
//! reclaims real space with no account, no network request, and no API key.
//! The Graph-backed quota and upload features live elsewhere and are opt-in.

pub mod account;
pub mod graph;
pub mod onedrive;