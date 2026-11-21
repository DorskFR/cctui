#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseSummary {
    pub output_chars: Option<usize>,
    pub usage: Option<serde_json::Value>,
}

pub trait ResponseDigest: Send {
    fn feed(&mut self, chunk: &[u8]);
    fn finish(self: Box<Self>) -> ResponseSummary;
}

pub const TRACE_KEPT_FIELDS: [&str; 6] =
    ["model", "stream", "max_tokens", "temperature", "service_tier", "thinking"];
pub const TRACE_COUNTED_FIELDS: [&str; 3] = ["messages", "system", "tools"];
