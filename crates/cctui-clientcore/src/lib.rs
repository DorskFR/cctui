//! Client-side interaction logic shared by the webui and the TUI.
//!
//! Pure functions only: no IO, no async, no clock, no randomness. Anything
//! ambient is a parameter, so the same case table in `fixtures/parity/` can be
//! replayed against this crate and against the `TypeScript` originals.

pub mod bookmarks;
pub mod drafts;
pub mod format;
pub mod git;
pub mod history_nav;
pub mod labels;
pub mod macros;
pub mod mention;
pub mod profiles;
pub mod search;
pub mod session_failure;
pub mod spawn;
pub mod turnid;
pub mod uploads;
pub mod uri;
