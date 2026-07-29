use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Contains,
    In,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Filter {
    pub field: String,
    pub op: FilterOp,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Empty,
    Text { value: String },
    Filter { filter: Filter },
    Not { child: Box<Self> },
    And { children: Vec<Self> },
    Or { children: Vec<Self> },
}

impl Node {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    #[must_use]
    pub fn free_text_terms(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut Vec<String>) {
        match self {
            Self::Text { value } => out.push(value.clone()),
            Self::Not { child } => child.collect_text(out),
            Self::And { children } | Self::Or { children } => {
                for c in children {
                    c.collect_text(out);
                }
            }
            Self::Empty | Self::Filter { .. } => {}
        }
    }
}
