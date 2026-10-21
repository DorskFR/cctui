//! Which gateway-streamed text is the session's own reply. A request belongs
//! to the main thread when its latest user text contains the turn's prompt;
//! a subagent's Task prompt or a side call never does.

use std::collections::VecDeque;

const PROMPTS: usize = 4;
const REQUESTS: usize = 16;
/// Held per request until a prompt claims it: the echo can trail the request.
const PENDING_BYTES: usize = 32 * 1024;

pub type Delta = (String, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Daemon,
    Gateway,
}

#[derive(Debug)]
enum Request {
    Accepted,
    Pending { text: String, held: Vec<Delta>, bytes: usize },
}

#[derive(Debug, Default)]
pub struct MainThread {
    prompts: VecDeque<String>,
    requests: VecDeque<(String, Request)>,
    /// The daemon streams this session's text itself; gateway text would
    /// say it twice.
    daemon_streams: bool,
    turn: Option<Source>,
}

pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn gateway_request(item: &str) -> Option<&str> {
    item.strip_prefix("gw:")?.rsplit_once(':').map(|(id, _)| id)
}

impl MainThread {
    pub const fn new_turn(&mut self) {
        self.turn = None;
    }

    pub fn prompt(&mut self, prompt: &str) -> Vec<Delta> {
        let needle = squash(prompt);
        if needle.is_empty() {
            return Vec::new();
        }
        let mut released = Vec::new();
        for (_, req) in &mut self.requests {
            if matches!(req, Request::Pending { text, .. } if text.contains(&needle))
                && let Request::Pending { held, .. } = std::mem::replace(req, Request::Accepted)
            {
                released.extend(held);
            }
        }
        if self.prompts.len() == PROMPTS {
            self.prompts.pop_front();
        }
        self.prompts.push_back(needle);
        self.claim(released)
    }

    pub fn request(&mut self, id: String, user_text: &str) {
        let text = squash(user_text);
        let req = if self.prompts.iter().any(|p| text.contains(p.as_str())) {
            Request::Accepted
        } else {
            Request::Pending { text, held: Vec::new(), bytes: 0 }
        };
        if self.requests.len() == REQUESTS {
            self.requests.pop_front();
        }
        self.requests.push_back((id, req));
    }

    pub fn delta(&mut self, item: String, text: String) -> Vec<Delta> {
        let Some(id) = gateway_request(&item).map(str::to_owned) else {
            self.daemon_streams = true;
            if self.turn == Some(Source::Gateway) {
                return Vec::new();
            }
            self.turn = Some(Source::Daemon);
            return vec![(item, text)];
        };
        let Some((_, req)) = self.requests.iter_mut().find(|(r, _)| *r == id) else {
            return Vec::new();
        };
        if let Request::Pending { held, bytes, .. } = req {
            if *bytes + text.len() <= PENDING_BYTES {
                *bytes += text.len();
                held.push((item, text));
            }
            return Vec::new();
        }
        self.claim(vec![(item, text)])
    }

    fn claim(&mut self, deltas: Vec<Delta>) -> Vec<Delta> {
        if deltas.is_empty() || self.daemon_streams || self.turn == Some(Source::Daemon) {
            return Vec::new();
        }
        self.turn = Some(Source::Gateway);
        deltas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(item: &str, text: &str) -> Delta {
        (item.to_owned(), text.to_owned())
    }

    const PROMPT: &str = "<cctui-session-context voice=\"spoken\">\nspoken\n</cctui-session-context>\n\nwhat time is it";

    #[test]
    fn a_request_naming_the_prompt_is_spoken_and_a_subagents_is_not() {
        let mut m = MainThread::default();
        m.prompt(PROMPT);
        m.request("main".into(), &format!("<system-reminder>x</system-reminder>\n{PROMPT}"));
        m.request("task".into(), "Find out what time is it in Tokyo");
        assert_eq!(m.delta("gw:main:0".into(), "It is".into()), vec![d("gw:main:0", "It is")]);
        assert!(m.delta("gw:task:0".into(), "Tokyo".into()).is_empty());
        assert!(m.delta("gw:unknown:0".into(), "?".into()).is_empty());
    }

    #[test]
    fn text_streamed_before_the_prompt_echo_is_held_then_released() {
        let mut m = MainThread::default();
        m.request("r".into(), "typed   prompt\nhere");
        assert!(m.delta("gw:r:0".into(), "Hel".into()).is_empty());
        assert!(m.delta("gw:r:0".into(), "lo".into()).is_empty());
        assert_eq!(m.prompt("typed prompt here"), vec![d("gw:r:0", "Hel"), d("gw:r:0", "lo")]);
        assert_eq!(m.delta("gw:r:0".into(), ".".into()), vec![d("gw:r:0", ".")]);
        assert!(m.prompt("typed prompt here").is_empty(), "released once");
    }

    #[test]
    fn a_session_the_daemon_streams_never_speaks_gateway_text() {
        let mut m = MainThread::default();
        m.prompt(PROMPT);
        m.request("r".into(), PROMPT);
        assert_eq!(m.delta("m:0".into(), "Hi".into()), vec![d("m:0", "Hi")]);
        assert!(m.delta("gw:r:0".into(), "Hi".into()).is_empty());
        m.new_turn();
        m.request("r2".into(), PROMPT);
        assert!(m.delta("gw:r2:0".into(), "Again".into()).is_empty());
    }

    #[test]
    fn a_turn_the_gateway_already_speaks_drops_late_daemon_text() {
        let mut m = MainThread::default();
        m.prompt(PROMPT);
        m.request("r".into(), PROMPT);
        assert_eq!(m.delta("gw:r:1".into(), "Hi".into()), vec![d("gw:r:1", "Hi")]);
        assert!(m.delta("m:0".into(), "Hi".into()).is_empty());
        m.new_turn();
        assert_eq!(m.delta("m:0".into(), "Next".into()), vec![d("m:0", "Next")]);
    }

    #[test]
    fn the_item_names_its_request() {
        assert_eq!(gateway_request("gw:abc:3"), Some("abc"));
        assert_eq!(gateway_request("msg_1:0"), None);
        assert_eq!(gateway_request("gw:"), None);
    }
}
