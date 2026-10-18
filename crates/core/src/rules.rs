//! Dynamic tag rules as an editor sees them: "match all/any" of a flat list of
//! conditions. Converts to and from the API filter JSON, which has the same
//! shape for `AssetDynamicTagWhereInput` (saving) and `AssetWhereInput`
//! (previewing with assetSearch). A port of `rules.ts`.

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleField {
    Name,
    Os,
    Customer,
    Site,
    Tag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleOp {
    Contains,
    Equals,
    StartsWith,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleCondition {
    pub field: RuleField,
    pub op: RuleOp,
    pub value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    All,
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub r#match: Match,
    pub conditions: Vec<RuleCondition>,
}

impl RuleField {
    pub const ALL: [RuleField; 5] = [Self::Name, Self::Os, Self::Customer, Self::Site, Self::Tag];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Device name",
            Self::Os => "Operating system",
            Self::Customer => "Customer",
            Self::Site => "Site",
            Self::Tag => "Has tag",
        }
    }

    /// Which operators the API supports for the field.
    pub fn ops(self) -> &'static [RuleOp] {
        match self {
            Self::Tag => &[RuleOp::Equals],
            _ => &[RuleOp::Contains, RuleOp::Equals, RuleOp::StartsWith],
        }
    }

    fn path(self) -> &'static [&'static str] {
        match self {
            Self::Name => &["name"],
            Self::Os => &["operatingSystem", "name"],
            Self::Customer => &["customer", "name"],
            Self::Site => &["site", "name"],
            Self::Tag => &["tags", "name"],
        }
    }
}

impl RuleField {
    /// The next field in the editor's order, wrapping round (for stepping through a dropdown).
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl RuleCondition {
    /// A blank condition, as the editor adds one.
    pub fn blank() -> Self {
        Self { field: RuleField::Name, op: RuleOp::Contains, value: String::new() }
    }

    /// Changes the field; the operator stays if the new field supports it, else becomes the field's first.
    pub fn set_field(&mut self, field: RuleField) {
        self.field = field;
        if !field.ops().contains(&self.op) {
            self.op = field.ops()[0];
        }
    }

    /// Steps to the next operator the field supports, wrapping round.
    pub fn next_op(&mut self) {
        let ops = self.field.ops();
        let i = ops.iter().position(|o| *o == self.op).unwrap_or(0);
        self.op = ops[(i + 1) % ops.len()];
    }
}

impl RuleOp {
    pub const ALL: [RuleOp; 3] = [Self::Contains, Self::Equals, Self::StartsWith];

    pub fn label(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Equals => "is",
            Self::StartsWith => "starts with",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Equals => "equals",
            Self::StartsWith => "startsWith",
        }
    }
}

pub fn empty_rule() -> Rule {
    Rule { r#match: Match::All, conditions: vec![RuleCondition { field: RuleField::Name, op: RuleOp::Contains, value: String::new() }] }
}

/// How many conditions have a value (blank rows are ignored).
pub fn active_condition_count(rule: &Rule) -> usize {
    active_conditions(rule).len()
}

/// Conditions with a value; blank rows are ignored.
pub fn active_conditions(rule: &Rule) -> Vec<&RuleCondition> {
    rule.conditions.iter().filter(|c| !c.value.trim().is_empty()).collect()
}

pub fn rule_to_filter(rule: &Rule) -> Value {
    let clauses: Vec<Value> = active_conditions(rule)
        .into_iter()
        .map(|c| {
            let leaf = json!({ c.op.key(): c.value.trim(), "caseSensitivity": "NON_SENSITIVE" });
            c.field.path().iter().rev().fold(leaf, |inner, key| json!({ *key: inner }))
        })
        .collect();
    json!({ if rule.r#match == Match::All { "and" } else { "or" }: clauses })
}

/// Parses a stored filter (JSON text or a value) back into an editable rule.
/// Returns None when the filter uses anything the editor can't represent
/// (nesting, other fields).
pub fn filter_to_rule(filter: &Value) -> Option<Rule> {
    let parsed;
    let f = match filter {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s).ok()?;
            &parsed
        }
        other => other,
    };
    let obj = f.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    let (key, clauses) = obj.iter().next()?;
    let r#match = match key.as_str() {
        "and" => Match::All,
        "or" => Match::Any,
        _ => return None,
    };
    let conditions = clauses.as_array()?.iter().map(clause_to_condition).collect::<Option<Vec<_>>>()?;
    Some(Rule { r#match, conditions })
}

fn clause_to_condition(clause: &Value) -> Option<RuleCondition> {
    'fields: for field in RuleField::ALL {
        let mut node = clause;
        for key in field.path() {
            match node.as_object() {
                Some(m) if m.len() == 1 && m.contains_key(*key) => node = &m[*key],
                _ => continue 'fields,
            }
        }
        let Some(leaf) = node.as_object() else { continue };
        let Some(op) = RuleOp::ALL.into_iter().find(|o| leaf.get(o.key()).is_some_and(Value::is_string)) else { continue };
        let extra = leaf.keys().any(|k| k != op.key() && k != "caseSensitivity");
        if !extra {
            return Some(RuleCondition { field, op, value: leaf[op.key()].as_str()?.to_string() });
        }
    }
    None
}

/// One-line summary, e.g. `Device name contains “SLS” and Site is “Lab”`.
pub fn describe_rule(rule: &Rule) -> String {
    let parts: Vec<String> = active_conditions(rule).iter().map(|c| format!("{} {} “{}”", c.field.label(), c.op.label(), c.value.trim())).collect();
    if parts.is_empty() { "No conditions".into() } else { parts.join(if rule.r#match == Match::All { " and " } else { " or " }) }
}

/// Tag names: letters, numbers, hyphens and underscores, up to 64 characters.
pub fn validate_tag_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("Enter a name.")
    } else if name.chars().count() > 64 {
        Some("Use 64 characters or fewer.")
    } else if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        Some("Use only letters, numbers, hyphens and underscores (no spaces).")
    } else {
        None
    }
}

/// Turns free text into a valid tag name suggestion: "Bitlocker not ready" -> "Bitlocker_not_ready".
pub fn suggest_tag_name(text: &str) -> String {
    let underscored: String = text.split_whitespace().collect::<Vec<_>>().join("_");
    underscored.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(field: RuleField, op: RuleOp, value: &str) -> RuleCondition {
        RuleCondition { field, op, value: value.into() }
    }

    #[test]
    fn builds_the_nested_filter_shape_the_api_stores() {
        let rule = Rule {
            r#match: Match::Any,
            conditions: vec![
                cond(RuleField::Name, RuleOp::Equals, "SLS-2188"),
                cond(RuleField::Os, RuleOp::Contains, "Windows 10"),
                cond(RuleField::Tag, RuleOp::Equals, "PatchGroupTwo"),
            ],
        };
        assert_eq!(
            rule_to_filter(&rule),
            json!({ "or": [
                { "name": { "equals": "SLS-2188", "caseSensitivity": "NON_SENSITIVE" } },
                { "operatingSystem": { "name": { "contains": "Windows 10", "caseSensitivity": "NON_SENSITIVE" } } },
                { "tags": { "name": { "equals": "PatchGroupTwo", "caseSensitivity": "NON_SENSITIVE" } } },
            ] })
        );
    }

    #[test]
    fn skips_blank_conditions() {
        let rule = Rule { r#match: Match::All, conditions: vec![cond(RuleField::Site, RuleOp::Contains, "  ")] };
        assert_eq!(rule_to_filter(&rule), json!({ "and": [] }));
    }

    #[test]
    fn parses_the_filter_stored_on_an_existing_dynamic_tag() {
        // Exactly what the API returned for the Windows10 tag.
        let stored = Value::String(r#"{"and": [{"operatingSystem": {"name": {"contains": "Windows 10", "caseSensitivity": "NON_SENSITIVE"}}}]}"#.into());
        assert_eq!(filter_to_rule(&stored), Some(Rule { r#match: Match::All, conditions: vec![cond(RuleField::Os, RuleOp::Contains, "Windows 10")] }));
    }

    #[test]
    fn round_trips_a_rule() {
        let rule = Rule {
            r#match: Match::All,
            conditions: vec![cond(RuleField::Customer, RuleOp::StartsWith, "Jason"), cond(RuleField::Site, RuleOp::Equals, "Parsippany")],
        };
        assert_eq!(filter_to_rule(&rule_to_filter(&rule)), Some(rule));
    }

    #[test]
    fn returns_none_for_filters_the_editor_cant_represent() {
        assert_eq!(filter_to_rule(&json!({ "and": [{ "or": [{ "name": { "equals": "x" } }] }] })), None);
        assert_eq!(filter_to_rule(&json!({ "and": [{ "cpu": { "cores": { "gte": 4 } } }] })), None);
        assert_eq!(filter_to_rule(&Value::String("not json".into())), None);
        assert_eq!(filter_to_rule(&json!({ "and": [], "or": [] })), None);
    }

    #[test]
    fn stepping_through_fields_wraps_round_both_ways() {
        assert_eq!(RuleField::Name.next(), RuleField::Os);
        assert_eq!(RuleField::Tag.next(), RuleField::Name, "wraps to the first");
        assert_eq!(RuleField::Name.previous(), RuleField::Tag, "and back to the last");
        let mut f = RuleField::Name;
        for _ in 0..RuleField::ALL.len() {
            f = f.next();
        }
        assert_eq!(f, RuleField::Name, "a full lap is a no-op");
    }

    #[test]
    fn changing_a_conditions_field_keeps_the_operator_only_if_the_field_supports_it() {
        let mut c = RuleCondition { field: RuleField::Name, op: RuleOp::StartsWith, value: "x".into() };
        c.set_field(RuleField::Os);
        assert_eq!(c.op, RuleOp::StartsWith, "operating system supports starts-with");
        c.set_field(RuleField::Tag);
        assert_eq!(c.op, RuleOp::Equals, "a tag can only be matched exactly");
        assert_eq!(c.value, "x", "the value is never lost");
    }

    #[test]
    fn the_operator_steps_only_through_what_the_field_allows() {
        let mut c = RuleCondition::blank();
        assert_eq!(c.op, RuleOp::Contains);
        c.next_op();
        assert_eq!(c.op, RuleOp::Equals);
        c.next_op();
        assert_eq!(c.op, RuleOp::StartsWith);
        c.next_op();
        assert_eq!(c.op, RuleOp::Contains, "wraps round");
        c.set_field(RuleField::Tag);
        c.next_op();
        assert_eq!(c.op, RuleOp::Equals, "the only choice for a tag");
    }

    #[test]
    fn summarizes_conditions() {
        let rule = Rule { r#match: Match::Any, conditions: vec![cond(RuleField::Name, RuleOp::Contains, "SLS"), cond(RuleField::Site, RuleOp::Equals, "Lab")] };
        assert_eq!(describe_rule(&rule), "Device name contains “SLS” or Site is “Lab”");
        assert_eq!(describe_rule(&empty_rule()), "No conditions");
    }

    #[test]
    fn validates_the_apis_tag_naming_rules() {
        assert_eq!(validate_tag_name("Patch_Group-2"), None);
        assert!(validate_tag_name("has space").unwrap().contains("no spaces"));
        assert!(validate_tag_name(&"x".repeat(65)).unwrap().contains("64"));
        assert!(validate_tag_name("").unwrap().contains("Enter"));
    }

    #[test]
    fn suggests_a_valid_name_from_free_text() {
        assert_eq!(suggest_tag_name("  Bitlocker not ready! "), "Bitlocker_not_ready");
        assert_eq!(validate_tag_name(&suggest_tag_name("a  b   c!!")), None);
    }
}
