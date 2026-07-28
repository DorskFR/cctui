//! Compiled-in adapter registry.

pub mod claude_code;
pub mod codex;
pub mod opencode;
pub mod uploads;

use crate::adapter_runtime::AdapterFactory;

#[must_use]
pub fn registry() -> Vec<Box<dyn AdapterFactory>> {
    vec![
        Box::new(claude_code::ClaudeCodeFactory),
        Box::new(codex::CodexFactory),
        Box::new(opencode::OpenCodeFactory),
    ]
}
