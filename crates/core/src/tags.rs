//! Creating and editing tags. A port of the logic in the desktop app's `TagWizard.tsx` and
//! `TagEditorDialog.tsx`. It is written against `TagApi`, so what matters (the order of calls,
//! and what happens when one of them fails part-way) can be tested with a fake.

use serde_json::Value;

use crate::{
    Core,
    model::{Asset, Organization, Tag},
    rules::{Rule, active_condition_count, rule_to_filter, validate_tag_name},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    Static,
    Dynamic,
}

/// The calls tag management makes. `Core` implements it for real; tests use a fake.
#[allow(async_fn_in_trait)]
pub trait TagApi {
    async fn create_static_tag(&self, org_id: &str, name: &str, description: &str, category: &str) -> Result<Tag, String>;
    async fn create_dynamic_tag(&self, org_id: &str, name: &str, description: &str, filter: &Value, category: &str) -> Result<Tag, String>;
    async fn update_static_tag(&self, id: &str, name: &str, description: &str) -> Result<Tag, String>;
    async fn update_dynamic_tag(&self, id: &str, name: &str, description: &str, filter: &Value) -> Result<Tag, String>;
    async fn assign_static_tags(&self, tag_ids: &[String], asset_ids: &[String], label: Option<&str>) -> Result<(), String>;
}

impl TagApi for Core {
    async fn create_static_tag(&self, org_id: &str, name: &str, description: &str, category: &str) -> Result<Tag, String> {
        Core::create_static_tag(self, org_id, name, description, category).await
    }
    async fn create_dynamic_tag(&self, org_id: &str, name: &str, description: &str, filter: &Value, category: &str) -> Result<Tag, String> {
        Core::create_dynamic_tag(self, org_id, name, description, filter, category).await
    }
    async fn update_static_tag(&self, id: &str, name: &str, description: &str) -> Result<Tag, String> {
        Core::update_static_tag(self, id, name, description).await
    }
    async fn update_dynamic_tag(&self, id: &str, name: &str, description: &str, filter: &Value) -> Result<Tag, String> {
        Core::update_dynamic_tag(self, id, name, description, filter).await
    }
    async fn assign_static_tags(&self, tag_ids: &[String], asset_ids: &[String], label: Option<&str>) -> Result<(), String> {
        Core::assign_static_tags(self, tag_ids, asset_ids, label).await
    }
}

/// Everything entered for a new tag.
#[derive(Clone, Debug)]
pub struct NewTag {
    pub kind: TagKind,
    pub name: String,
    pub description: String,
    pub category: String,
    /// Where the tag can be used: an organization, customer or site (and everything under it).
    pub org: Option<Organization>,
    /// Static tags: the devices to put in it from the start.
    pub devices: Vec<Asset>,
    /// Dynamic tags: the rule.
    pub rule: Rule,
}

impl NewTag {
    pub fn new(kind: TagKind, org: Option<Organization>) -> Self {
        Self { kind, name: String::new(), description: String::new(), category: String::new(), org, devices: vec![], rule: crate::rules::empty_rule() }
    }
}

/// What is wrong with a tag form, by field.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TagProblems {
    pub name: Option<&'static str>,
    pub access: Option<&'static str>,
    pub rule: Option<&'static str>,
}

impl TagProblems {
    pub fn any(&self) -> bool {
        self.name.is_some() || self.access.is_some() || self.rule.is_some()
    }
}

const NEEDS_CONDITION: &str = "Add at least one condition";

pub fn check_new(t: &NewTag) -> TagProblems {
    TagProblems {
        name: validate_tag_name(t.name.trim()),
        access: t.org.is_none().then_some("Choose where the tag can be used"),
        rule: (t.kind == TagKind::Dynamic && active_condition_count(&t.rule) == 0).then_some(NEEDS_CONDITION),
    }
}

/// A creation that did not finish. A static tag can be created and then fail to get its devices;
/// `created` is then the tag that exists, so a retry only adds the devices.
#[derive(Debug)]
pub struct CreateFailure {
    pub message: String,
    pub created: Option<Tag>,
}

pub async fn create_tag<A: TagApi>(api: &A, t: &NewTag, resume: Option<Tag>) -> Result<Tag, CreateFailure> {
    let fail = |message: String, created: Option<Tag>| CreateFailure { message, created };
    if check_new(t).any() {
        return Err(fail("Fix the highlighted fields first.".into(), resume));
    }
    let org = t.org.as_ref().expect("checked above");
    let (name, description, category) = (t.name.trim(), t.description.trim(), t.category.trim());
    match t.kind {
        TagKind::Static => {
            let tag = match resume {
                Some(existing) => existing,
                None => api.create_static_tag(&org.id, name, description, category).await.map_err(|e| fail(e, None))?,
            };
            if !t.devices.is_empty() {
                let ids: Vec<String> = t.devices.iter().map(|d| d.id.clone()).collect();
                if let Err(e) = api.assign_static_tags(std::slice::from_ref(&tag.id), &ids, Some(&tag.name)).await {
                    return Err(fail(format!("Created {}, but couldn't add the devices: {e}", tag.name), Some(tag)));
                }
            }
            Ok(Tag { asset_count: Some(t.devices.len() as i64), ..tag })
        }
        TagKind::Dynamic => api.create_dynamic_tag(&org.id, name, description, &rule_to_filter(&t.rule), category).await.map_err(|e| fail(e, None)),
    }
}

/// Changes to an existing tag.
#[derive(Clone, Debug)]
pub struct EditTag {
    pub tag: Tag,
    pub name: String,
    pub description: String,
    /// A dynamic tag's rule. None keeps the stored filter as it is, for rules the editor can't represent.
    pub rule: Option<Rule>,
}

pub fn check_edit(e: &EditTag) -> TagProblems {
    TagProblems {
        name: validate_tag_name(e.name.trim()),
        access: None,
        rule: (e.tag.is_dynamic() && e.rule.as_ref().is_some_and(|r| active_condition_count(r) == 0)).then_some(NEEDS_CONDITION),
    }
}

pub async fn update_tag<A: TagApi>(api: &A, e: &EditTag) -> Result<Tag, String> {
    if check_edit(e).any() {
        return Err("Fix the highlighted fields first.".into());
    }
    let name = e.name.trim();
    if !e.tag.is_dynamic() {
        return api.update_static_tag(&e.tag.id, name, &e.description).await;
    }
    let filter = match &e.rule {
        Some(rule) => rule_to_filter(rule),
        None => {
            let stored = e.tag.filter.as_deref().ok_or("This tag has no rule to keep.")?;
            serde_json::from_str(stored).map_err(|_| "The tag's stored rule could not be read, so it was left unchanged.".to_string())?
        }
    };
    api.update_dynamic_tag(&e.tag.id, name, &e.description, &filter).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::rules::{Match, RuleCondition, RuleField, RuleOp};

    /// Records every call and fails the ones it is told to.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        fail_assign: Mutex<bool>,
        fail_create: Mutex<Option<String>>,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn log(&self, s: String) {
            self.calls.lock().unwrap().push(s);
        }
    }

    fn tag(id: &str, name: &str, dynamic: bool, filter: Option<&str>) -> Tag {
        Tag { id: id.into(), name: name.into(), typename: if dynamic { "DynamicTag" } else { "StaticTag" }.into(), asset_count: Some(0), updated_at: None, description: None, organization: None, filter: filter.map(String::from) }
    }

    impl TagApi for Fake {
        async fn create_static_tag(&self, org_id: &str, name: &str, description: &str, category: &str) -> Result<Tag, String> {
            self.log(format!("create_static {org_id} {name:?} {description:?} {category:?}"));
            match self.fail_create.lock().unwrap().clone() {
                Some(e) => Err(e),
                None => Ok(tag("new-id", name, false, None)),
            }
        }
        async fn create_dynamic_tag(&self, org_id: &str, name: &str, description: &str, filter: &Value, category: &str) -> Result<Tag, String> {
            self.log(format!("create_dynamic {org_id} {name:?} {description:?} {category:?} {filter}"));
            Ok(tag("dyn-id", name, true, Some(&filter.to_string())))
        }
        async fn update_static_tag(&self, id: &str, name: &str, description: &str) -> Result<Tag, String> {
            self.log(format!("update_static {id} {name:?} {description:?}"));
            Ok(tag(id, name, false, None))
        }
        async fn update_dynamic_tag(&self, id: &str, name: &str, description: &str, filter: &Value) -> Result<Tag, String> {
            self.log(format!("update_dynamic {id} {name:?} {description:?} {filter}"));
            Ok(tag(id, name, true, Some(&filter.to_string())))
        }
        async fn assign_static_tags(&self, tag_ids: &[String], asset_ids: &[String], label: Option<&str>) -> Result<(), String> {
            self.log(format!("assign {tag_ids:?} {asset_ids:?} {label:?}"));
            if *self.fail_assign.lock().unwrap() { Err("boom".into()) } else { Ok(()) }
        }
    }

    fn org() -> Organization {
        Organization { id: "o1".into(), name: "Acme".into(), typename: "Customer".into() }
    }

    fn new_static(name: &str, devices: usize) -> NewTag {
        let mut t = NewTag::new(TagKind::Static, Some(org()));
        t.name = name.into();
        t.devices = (0..devices).map(|i| Asset::stub(&format!("a{i}"), &format!("PC-{i}"))).collect();
        t
    }

    fn new_dynamic(name: &str, value: &str) -> NewTag {
        let mut t = NewTag::new(TagKind::Dynamic, Some(org()));
        t.name = name.into();
        t.rule = Rule { r#match: Match::Any, conditions: vec![RuleCondition { field: RuleField::Os, op: RuleOp::Contains, value: value.into() }] };
        t
    }

    #[test]
    fn a_tag_form_is_checked_field_by_field() {
        let mut t = NewTag::new(TagKind::Dynamic, None);
        let p = check_new(&t);
        assert_eq!((p.name, p.access, p.rule), (Some("Enter a name."), Some("Choose where the tag can be used"), Some("Add at least one condition")));
        t.name = "has space".into();
        assert!(check_new(&t).name.unwrap().contains("no spaces"));
        t.name = "Windows10".into();
        t.org = Some(org());
        t.rule = new_dynamic("x", "Windows 10").rule;
        assert!(!check_new(&t).any());
        // A static tag needs no rule, and a blank condition doesn't count.
        let mut s = NewTag::new(TagKind::Static, Some(org()));
        s.name = "Servers".into();
        assert!(!check_new(&s).any());
        t.rule.conditions[0].value = "   ".into();
        assert_eq!(check_new(&t).rule, Some("Add at least one condition"));
    }

    #[tokio::test]
    async fn a_static_tag_without_devices_is_just_created_with_its_text_trimmed() {
        let api = Fake::default();
        let mut t = new_static("  Servers ", 0);
        t.description = " the servers ".into();
        t.category = " Infra ".into();
        let made = create_tag(&api, &t, None).await.unwrap();
        assert_eq!(api.calls(), [r#"create_static o1 "Servers" "the servers" "Infra""#]);
        assert_eq!((made.name.as_str(), made.asset_count), ("Servers", Some(0)));
    }

    #[tokio::test]
    async fn a_static_tag_with_devices_is_created_and_then_filled_in_that_order() {
        let api = Fake::default();
        let made = create_tag(&api, &new_static("Servers", 3), None).await.unwrap();
        let calls = api.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].starts_with("create_static") && calls[1].contains(r#"["new-id"]"#) && calls[1].contains(r#"["a0", "a1", "a2"]"#) && calls[1].contains(r#"Some("Servers")"#), "{calls:?}");
        assert_eq!(made.asset_count, Some(3));
    }

    #[tokio::test]
    async fn if_adding_devices_fails_the_tag_still_exists_and_a_retry_only_adds_the_devices() {
        let api = Fake::default();
        *api.fail_assign.lock().unwrap() = true;
        let t = new_static("Servers", 2);
        let failure = create_tag(&api, &t, None).await.unwrap_err();
        assert_eq!(failure.message, "Created Servers, but couldn't add the devices: boom");
        let created = failure.created.expect("the tag that exists is handed back");
        assert_eq!(created.id, "new-id");

        *api.fail_assign.lock().unwrap() = false;
        let made = create_tag(&api, &t, Some(created)).await.unwrap();
        let creates = api.calls().iter().filter(|c| c.starts_with("create_static")).count();
        assert_eq!(creates, 1, "a retry must not create a second tag with the same name");
        assert_eq!(api.calls().iter().filter(|c| c.starts_with("assign")).count(), 2);
        assert_eq!(made.asset_count, Some(2));
    }

    #[tokio::test]
    async fn a_creation_that_fails_outright_says_why_and_leaves_nothing_behind() {
        let api = Fake::default();
        *api.fail_create.lock().unwrap() = Some("Could not create the tag: Tag with this name already exists".into());
        let failure = create_tag(&api, &new_static("Servers", 2), None).await.unwrap_err();
        assert!(failure.message.contains("already exists") && failure.created.is_none());
        assert_eq!(api.calls().len(), 1, "no devices are added to a tag that wasn't made");
    }

    #[tokio::test]
    async fn a_dynamic_tag_sends_its_rule_as_the_api_filter() {
        let api = Fake::default();
        let made = create_tag(&api, &new_dynamic("Windows10", "Windows 10"), None).await.unwrap();
        let call = &api.calls()[0];
        assert!(call.starts_with(r#"create_dynamic o1 "Windows10""#) && call.contains(r#""operatingSystem":{"name":{"caseSensitivity":"NON_SENSITIVE","contains":"Windows 10"}}"#), "{call}");
        assert!(call.contains(r#"{"or":["#), "match any becomes or: {call}");
        assert!(made.is_dynamic());
        assert_eq!(api.calls().len(), 1, "a dynamic tag fills itself: no devices are assigned");
    }

    #[tokio::test]
    async fn nothing_is_sent_for_a_form_with_problems() {
        let api = Fake::default();
        let failure = create_tag(&api, &NewTag::new(TagKind::Static, Some(org())), None).await.unwrap_err();
        assert_eq!(failure.message, "Fix the highlighted fields first.");
        assert!(api.calls().is_empty());
    }

    fn edit(tag: Tag, name: &str, rule: Option<Rule>) -> EditTag {
        EditTag { description: "d".into(), name: name.into(), rule, tag }
    }

    #[tokio::test]
    async fn editing_a_static_tag_updates_its_name_and_description() {
        let api = Fake::default();
        update_tag(&api, &edit(tag("1", "Old", false, None), " New ", None)).await.unwrap();
        assert_eq!(api.calls(), [r#"update_static 1 "New" "d""#]);
    }

    #[tokio::test]
    async fn editing_a_dynamic_tag_sends_the_new_rule() {
        let api = Fake::default();
        let rule = new_dynamic("x", "macOS").rule;
        update_tag(&api, &edit(tag("1", "Mac", true, Some("{}")), "Mac", Some(rule))).await.unwrap();
        assert!(api.calls()[0].contains("macOS"), "{:?}", api.calls());
    }

    #[tokio::test]
    async fn a_rule_the_editor_cant_represent_is_kept_exactly_as_stored() {
        let api = Fake::default();
        let stored = r#"{"and":[{"cpu":{"cores":{"gte":4}}}]}"#;
        update_tag(&api, &edit(tag("1", "Big", true, Some(stored)), "Big2", None)).await.unwrap();
        assert!(api.calls()[0].ends_with(&json!({ "and": [{ "cpu": { "cores": { "gte": 4 } } }] }).to_string()), "{:?}", api.calls());
        // A stored rule that isn't even JSON is refused rather than replaced by something else.
        let err = update_tag(&api, &edit(tag("1", "Big", true, Some("not json")), "Big", None)).await.unwrap_err();
        assert!(err.contains("could not be read"));
        let err = update_tag(&api, &edit(tag("1", "Big", true, None), "Big", None)).await.unwrap_err();
        assert!(err.contains("no rule"));
    }

    #[tokio::test]
    async fn an_edit_with_a_bad_name_or_an_emptied_rule_sends_nothing() {
        let api = Fake::default();
        assert!(update_tag(&api, &edit(tag("1", "A", false, None), "bad name", None)).await.is_err());
        let mut empty = new_dynamic("x", "").rule;
        empty.conditions[0].value.clear();
        assert!(update_tag(&api, &edit(tag("1", "A", true, Some("{}")), "A", Some(empty))).await.is_err());
        assert!(api.calls().is_empty());
    }

    #[tokio::test]
    async fn in_dry_run_a_real_tag_creation_is_held_back_before_anything_else() {
        let core = Core::for_tests("tags-dry");
        core.set_dry_run(true);
        let failure = create_tag(&*core, &new_static("Servers", 2), None).await.unwrap_err();
        assert_eq!(failure.message, crate::DRY_RUN_ERROR);
        assert!(failure.created.is_none());
        let log = core.dry_run_log();
        assert_eq!(log.len(), 1, "the devices are not even attempted");
        assert_eq!(log[0].summary, "Created static tag Servers");
    }
}
