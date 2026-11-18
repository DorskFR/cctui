use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Contains,
    In,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct Filter {
    pub field: String,
    pub op: FilterOp,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Empty,
    Text {
        value: String,
        /// Typed between quotes: matched as an exact substring, not from a word start.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        phrase: bool,
    },
    Filter {
        filter: Filter,
    },
    Not {
        child: Box<Self>,
    },
    And {
        children: Vec<Self>,
    },
    Or {
        children: Vec<Self>,
    },
}

impl Node {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    #[must_use]
    pub fn free_text_terms(&self) -> Vec<String> {
        self.free_text_leaves().into_iter().map(|(value, _)| value.to_owned()).collect()
    }

    /// Every free-text value with its `phrase` flag.
    #[must_use]
    pub fn free_text_leaves(&self) -> Vec<(&str, bool)> {
        let mut out = Vec::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text<'a>(&'a self, out: &mut Vec<(&'a str, bool)>) {
        match self {
            Self::Text { value, phrase } => out.push((value.as_str(), *phrase)),
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
