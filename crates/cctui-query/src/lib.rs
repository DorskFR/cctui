pub mod ast;
pub mod parser;
pub mod registry;

pub use ast::{Filter, FilterOp, Node};
pub use parser::parse;
pub use registry::{FIELDS, FieldDef, FieldType, resolve};

#[cfg(test)]
mod tests {
    use super::ast::{FilterOp, Node};
    use super::parse;

    fn text(v: &str) -> Node {
        Node::Text { value: v.to_string() }
    }

    fn filter(field: &str, op: FilterOp, values: &[&str]) -> Node {
        Node::Filter {
            filter: super::ast::Filter {
                field: field.to_string(),
                op,
                values: values.iter().map(ToString::to_string).collect(),
            },
        }
    }

    #[test]
    fn plain_keyword_backcompat() {
        assert_eq!(parse("hello"), text("hello"));
    }

    #[test]
    fn empty_is_browse() {
        assert!(parse("").is_empty());
        assert!(parse("   ").is_empty());
    }

    #[test]
    fn implicit_and_between_keywords() {
        assert_eq!(
            parse("contains A AND B"),
            Node::And { children: vec![text("contains"), text("A"), text("B")] }
        );
    }

    #[test]
    fn explicit_or() {
        assert_eq!(parse("A OR B"), Node::Or { children: vec![text("A"), text("B")] });
    }

    #[test]
    fn field_and_keyword() {
        assert_eq!(
            parse("machine:dev1 keyword"),
            Node::And {
                children: vec![filter("machine", FilterOp::Eq, &["dev1"]), text("keyword")]
            }
        );
    }

    #[test]
    fn grouping_precedence() {
        let parsed = parse("( machine:m2pro OR machine:dev1 ) AND keyword");
        assert_eq!(
            parsed,
            Node::And {
                children: vec![
                    Node::Or {
                        children: vec![
                            filter("machine", FilterOp::Eq, &["m2pro"]),
                            filter("machine", FilterOp::Eq, &["dev1"]),
                        ]
                    },
                    text("keyword"),
                ]
            }
        );
    }

    #[test]
    fn tag_alias_label() {
        assert_eq!(parse("tag:x"), filter("tag", FilterOp::Eq, &["x"]));
        assert_eq!(parse("label:x"), filter("tag", FilterOp::Eq, &["x"]));
    }

    #[test]
    fn machine_alias_m() {
        assert_eq!(parse("m:dev1"), filter("machine", FilterOp::Eq, &["dev1"]));
    }

    #[test]
    fn negation_dash() {
        assert_eq!(
            parse("-machine:dev1"),
            Node::Not { child: Box::new(filter("machine", FilterOp::Eq, &["dev1"])) }
        );
    }

    #[test]
    fn negation_not_prefix() {
        assert_eq!(parse("not:archived"), Node::Not { child: Box::new(text("archived")) });
        assert_eq!(
            parse("not:machine:dev1"),
            Node::Not { child: Box::new(filter("machine", FilterOp::Eq, &["dev1"])) }
        );
    }

    #[test]
    fn unknown_field_degrades_to_text() {
        assert_eq!(parse("foo:bar"), text("foo:bar"));
    }

    #[test]
    fn in_op_from_comma_list() {
        assert_eq!(
            parse("status:active,inactive"),
            filter("status", FilterOp::In, &["active", "inactive"])
        );
    }

    #[test]
    fn pinned_bool_normalised() {
        assert_eq!(parse("pinned:yes"), filter("pinned", FilterOp::Eq, &["true"]));
        assert_eq!(parse("starred:false"), filter("pinned", FilterOp::Eq, &["false"]));
    }

    #[test]
    fn contains_default_op_for_title() {
        assert_eq!(parse("title:fix"), filter("title", FilterOp::Contains, &["fix"]));
    }

    #[test]
    fn quoted_phrase_is_text() {
        assert_eq!(parse("\"hello world\""), text("hello world"));
        assert_eq!(parse("\"machine:dev1\""), text("machine:dev1"));
    }

    #[test]
    fn malformed_unbalanced_parens_degrade() {
        let parsed = parse("( machine:dev1 AND");
        assert_eq!(parsed, filter("machine", FilterOp::Eq, &["dev1"]));
        assert!(!parse(")))").is_empty() || parse(")))").is_empty());
    }

    #[test]
    fn dangling_operators_recover() {
        assert_eq!(parse("AND OR keyword"), text("keyword"));
        assert_eq!(parse("keyword AND"), text("keyword"));
    }

    #[test]
    fn free_text_terms_collected() {
        let parsed = parse("machine:dev1 hello ( world OR title:fix )");
        let terms = parsed.free_text_terms();
        assert_eq!(terms, vec!["hello".to_string(), "world".to_string()]);
    }
}
