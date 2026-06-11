//! Workflow guard — markdown-driven step engine for Claude worker sessions.
//!
//! Rust port of the Python `workflow-guard/daemon.py` reference implementation.
//! Parses `[allowed]`/`[disallowed]`/`[transition]`/`[network]` blocks from a
//! prompt markdown file and enforces them through a localhost HTTP server that
//! Claude Code's `PreToolUse` hook calls before every tool invocation.
//!
//! See the crate README for the canonical prompt-step and guard-rules format.

pub mod engine;
pub mod parser;
pub mod rules;
pub mod server;

pub use engine::WorkflowEngine;
pub use parser::{
    Step, parse_guard_rules, parse_guard_rules_str, parse_keywords, parse_steps, parse_transitions,
};
pub use rules::{check_rules, split_bash_segments};
