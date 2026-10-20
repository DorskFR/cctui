//! `elicitation/create` forms rendered as an `AskUserQuestion` form, and the
//! form's answer turned back into the typed `content` the agent asked for.

use serde_json::{Map, Value, json};

const YES: &str = "Yes";
const NO: &str = "No";

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Text,
    Number,
    Integer,
    Boolean,
    Choice { values: Vec<String>, labels: Vec<String>, multi: bool },
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    key: String,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub message: String,
    fields: Vec<Field>,
}

pub enum Action {
    Accept(Value),
    Decline,
    Cancel,
}

impl Form {
    /// `None` for anything but a form-mode request (a URL elicitation has
    /// nothing the clients can render).
    #[must_use]
    pub fn parse(params: &Value) -> Option<Self> {
        if params.get("mode").and_then(Value::as_str).is_some_and(|m| m != "form") {
            return None;
        }
        let schema = params.get("requestedSchema")?;
        let fields = schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(key, prop)| Field { key: key.clone(), kind: kind_of(prop) })
            .collect();
        let message = params.get("message").and_then(Value::as_str).unwrap_or_default().to_owned();
        Some(Self { message, fields })
    }

    /// The `questions` payload of an `AskQuestion`, one question per field.
    #[must_use]
    pub fn questions(&self, params: &Value) -> Value {
        let props = params.pointer("/requestedSchema/properties");
        Value::Array(
            self.fields
                .iter()
                .map(|f| {
                    let prop = props.and_then(|p| p.get(&f.key)).unwrap_or(&Value::Null);
                    let title = prop.get("title").and_then(Value::as_str).unwrap_or(&f.key);
                    let question = prop.get("description").and_then(Value::as_str).unwrap_or(title);
                    let (options, multi): (Vec<&str>, bool) = match &f.kind {
                        Kind::Boolean => (vec![YES, NO], false),
                        Kind::Choice { labels, multi, .. } => {
                            (labels.iter().map(String::as_str).collect(), *multi)
                        }
                        _ => (Vec::new(), false),
                    };
                    json!({
                        "header": title,
                        "question": question,
                        "multiSelect": multi,
                        "options": options.iter().map(|l| json!({ "label": l })).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        )
    }

    /// The clients' answer: `→ ` lines in question order, plus option picks
    /// when no free text was typed. Unparsable values are left out.
    #[must_use]
    pub fn content(&self, text: &str, picks: Option<&[Vec<usize>]>) -> Value {
        let answers: Vec<&str> =
            text.lines().filter_map(|l| l.strip_prefix("→ ")).map(str::trim).collect();
        let mut out = Map::new();
        for (i, field) in self.fields.iter().enumerate() {
            let picked = picks.and_then(|p| p.get(i));
            let raw = answers.get(i).copied().unwrap_or_default();
            if let Some(value) = field.value(raw, picked) {
                out.insert(field.key.clone(), value);
            }
        }
        Value::Object(out)
    }
}

impl Field {
    fn value(&self, raw: &str, picked: Option<&Vec<usize>>) -> Option<Value> {
        match &self.kind {
            Kind::Text => (!raw.is_empty()).then(|| json!(raw)),
            Kind::Number => raw.parse::<f64>().ok().map(|n| json!(n)),
            Kind::Integer => raw.parse::<i64>().ok().map(|n| json!(n)),
            Kind::Boolean => match picked.and_then(|p| p.first()) {
                Some(0) => Some(json!(true)),
                Some(_) => Some(json!(false)),
                None => match raw.to_ascii_lowercase().as_str() {
                    "yes" | "true" => Some(json!(true)),
                    "no" | "false" => Some(json!(false)),
                    _ => None,
                },
            },
            Kind::Choice { values, labels, multi } => {
                let chosen: Vec<String> = match picked {
                    Some(p) => p.iter().filter_map(|&i| values.get(i).cloned()).collect(),
                    None if !*multi => vec![label_value(labels, values, raw)],
                    None => raw.split(", ").map(|r| label_value(labels, values, r)).collect(),
                };
                let chosen: Vec<String> = chosen.into_iter().filter(|v| !v.is_empty()).collect();
                if *multi {
                    Some(json!(chosen))
                } else {
                    chosen.into_iter().next().map(Value::String)
                }
            }
        }
    }
}

fn label_value(labels: &[String], values: &[String], raw: &str) -> String {
    labels
        .iter()
        .position(|l| l == raw)
        .and_then(|i| values.get(i))
        .cloned()
        .unwrap_or_else(|| raw.to_owned())
}

fn kind_of(prop: &Value) -> Kind {
    let choice = |list: Option<&Value>, multi: bool| -> Option<Kind> {
        let list = list?.as_array()?;
        let (values, labels) = list
            .iter()
            .filter_map(|o| match o {
                Value::String(s) => Some((s.clone(), s.clone())),
                o => {
                    let value = o.get("const").and_then(Value::as_str)?.to_owned();
                    let label = o.get("title").and_then(Value::as_str).unwrap_or(&value).to_owned();
                    Some((value, label))
                }
            })
            .unzip();
        Some(Kind::Choice { values, labels, multi })
    };
    match prop.get("type").and_then(Value::as_str) {
        Some("number") => Kind::Number,
        Some("integer") => Kind::Integer,
        Some("boolean") => Kind::Boolean,
        Some("array") => {
            let items = prop.get("items");
            choice(items.and_then(|i| i.get("enum")), true)
                .or_else(|| choice(items.and_then(|i| i.get("anyOf")), true))
                .unwrap_or(Kind::Text)
        }
        _ => choice(prop.get("enum"), false)
            .or_else(|| choice(prop.get("oneOf"), false))
            .unwrap_or(Kind::Text),
    }
}

#[must_use]
pub fn response(action: Action) -> Value {
    match action {
        Action::Accept(content) => json!({ "action": "accept", "content": content }),
        Action::Decline => json!({ "action": "decline" }),
        Action::Cancel => json!({ "action": "cancel" }),
    }
}

/// Whether a reply is the form's answer rather than an unrelated message.
#[must_use]
pub fn is_answer(text: &str, picks: Option<&[Vec<usize>]>) -> bool {
    picks.is_some() || text.lines().any(|l| l.starts_with("→ "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Value {
        json!({
            "mode": "form",
            "sessionId": "s",
            "message": "Configure the deploy",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "color": { "type": "string", "title": "Color",
                               "oneOf": [{ "const": "r", "title": "Red" }, { "const": "g", "title": "Green" }] },
                    "count": { "type": "integer", "title": "Count" },
                    "name": { "type": "string", "description": "Who are you?" },
                    "ok": { "type": "boolean", "title": "Proceed" },
                    "ratio": { "type": "number" },
                    "tags": { "type": "array", "items": { "enum": ["a", "b", "c"] } },
                },
                "required": ["name"],
            },
        })
    }

    #[test]
    fn a_form_becomes_one_question_per_field() {
        let p = params();
        let form = Form::parse(&p).unwrap();
        assert_eq!(form.message, "Configure the deploy");
        let qs = form.questions(&p);
        assert_eq!(qs.as_array().unwrap().len(), 6);
        assert_eq!(qs[0]["header"], "Color");
        assert_eq!(qs[0]["options"][1]["label"], "Green");
        assert_eq!(qs[2]["question"], "Who are you?");
        assert_eq!(qs[2]["options"], json!([]));
        assert_eq!(qs[3]["options"][0]["label"], "Yes");
        assert_eq!(qs[5]["multiSelect"], true);
    }

    #[test]
    fn a_url_elicitation_is_not_a_form() {
        assert!(Form::parse(&json!({ "mode": "url", "url": "https://x.dev" })).is_none());
    }

    #[test]
    fn free_text_answers_are_typed_per_field() {
        let form = Form::parse(&params()).unwrap();
        let text = "**Color** — Color\n→ Green\n\n**Count** — Count\n→ 7\n\n\
                    **name** — Who are you?\n→ Ada\n\n**Proceed** — Proceed\n→ Yes\n\n\
                    **ratio** — ratio\n→ 0.5\n\n**tags** — tags\n→ a, c";
        assert_eq!(
            form.content(text, None),
            json!({ "color": "g", "count": 7, "name": "Ada", "ok": true, "ratio": 0.5, "tags": ["a", "c"] })
        );
    }

    #[test]
    fn picks_win_over_labels_and_bad_numbers_are_dropped() {
        let form = Form::parse(&params()).unwrap();
        let text = "→ Red\n→ many\n→ Ada\n→ No\n→ \n→ b";
        let picks = vec![vec![0], vec![], vec![], vec![1], vec![], vec![1, 2]];
        assert_eq!(
            form.content(text, Some(&picks)),
            json!({ "color": "r", "name": "Ada", "ok": false, "tags": ["b", "c"] })
        );
    }

    #[test]
    fn responses_carry_the_acp_actions() {
        assert_eq!(response(Action::Accept(json!({ "a": 1 })))["content"]["a"], 1);
        assert_eq!(response(Action::Decline), json!({ "action": "decline" }));
        assert_eq!(response(Action::Cancel), json!({ "action": "cancel" }));
        assert!(is_answer("q\n→ x", None));
        assert!(!is_answer("never mind", None));
    }
}
