//! Client-side interaction logic shared by the webui and the TUI.
//!
//! Pure functions only: no IO, no async, no clock, no randomness. Anything
//! ambient is a parameter, so the same case table in `fixtures/parity/` can be
//! replayed against this crate and against the `TypeScript` originals.

pub mod account_switch;
pub mod accounts;
pub mod admin;
pub mod bookmarks;
pub mod format;
pub mod git;
pub mod history_nav;
pub mod images;
pub mod instance;
pub mod labels;
pub mod mention;
pub mod search;
pub mod session_failure;
pub mod spend;
pub mod turnid;
pub mod uploads;
pub mod uri;
pub mod usage;
