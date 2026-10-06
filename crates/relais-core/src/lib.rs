//! Core of Relais: accounts, per-account profiles, shared configuration,
//! usage readings and conversation index.
//!
//! Relais never reads, copies, refreshes or stores a credential. Every sign-in
//! and every token refresh is done by Anthropic's own apps; Relais only decides
//! which per-account profile is active.

pub mod claude;
pub mod error;
pub mod fsutil;
pub mod layout;
pub mod registry;
pub mod service;
pub mod usage;

pub use error::{Error, Result};
pub use layout::Layout;
