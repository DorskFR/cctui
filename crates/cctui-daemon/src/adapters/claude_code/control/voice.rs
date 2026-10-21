//! Per-hop timing of a spoken prompt's way into its worker, keyed by session
//! and the server's command id, in ms since the adapter picked the reply up.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
struct Clock {
    command_id: Option<uuid::Uuid>,
    start: Instant,
}

#[derive(Debug, Default)]
pub(super) struct VoiceClocks(Mutex<HashMap<String, Clock>>);

impl VoiceClocks {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Clock>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Times `text` toward `local_id` when it is a spoken prompt; any other
    /// reply drops a clock its predecessor left running.
    pub(super) fn start(&self, local_id: &str, text: &str, command_id: Option<uuid::Uuid>) {
        if !cctui_proto::injected::is_spoken_prompt(text) {
            self.lock().remove(local_id);
            return;
        }
        let clock = Clock { command_id, start: Instant::now() };
        self.lock().insert(local_id.to_owned(), clock);
        log_hop(local_id, clock, "adapter picked up");
    }

    pub(super) fn hop(&self, local_id: &str, hop: &str) {
        let clock = self.lock().get(local_id).copied();
        if let Some(clock) = clock {
            log_hop(local_id, clock, hop);
        }
    }

    pub(super) fn tailed(&self, local_id: &str, text: &str) -> bool {
        if !cctui_proto::injected::is_spoken_prompt(text) {
            return false;
        }
        let clock = self.lock().remove(local_id);
        clock.inspect(|&clock| log_hop(local_id, clock, "transcript user line tailed")).is_some()
    }
}

fn log_hop(local_id: &str, clock: Clock, hop: &str) {
    let ms = u64::try_from(clock.start.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(%local_id, command_id = ?clock.command_id, hop, ms, "voice delivery hop");
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOKEN: &str =
        "<cctui-session-context voice=\"spoken\">\nbody\n</cctui-session-context>\n\nhi";

    #[test]
    fn a_spoken_prompt_is_timed_until_the_tail_reads_it_back() {
        let clocks = VoiceClocks::default();
        clocks.start("s", SPOKEN, Some(uuid::Uuid::nil()));
        assert!(!clocks.tailed("s", "a tool result"), "only the prompt itself ends the clock");
        assert!(clocks.tailed("s", SPOKEN));
        assert!(!clocks.tailed("s", SPOKEN), "a clock ends once");
    }

    #[test]
    fn a_typed_reply_is_not_timed_and_retires_a_stale_clock() {
        let clocks = VoiceClocks::default();
        clocks.start("s", "typed", None);
        assert!(!clocks.tailed("s", SPOKEN));
        clocks.start("s", SPOKEN, None);
        clocks.start("s", "typed", None);
        assert!(!clocks.tailed("s", SPOKEN));
    }
}
