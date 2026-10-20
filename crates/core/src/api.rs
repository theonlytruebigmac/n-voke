//! Every GraphQL query and mutation the app makes. A port of `api.ts`: the
//! queries are the same text, so the API notes in the README apply unchanged
//! (e.g. `assetsScriptRun` selects only `items { id }`, and polling never asks
//! for a task's `name`).

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    Core,
    audit::AuditMeta,
    inputs::InputType,
    model::{Asset, InputValue, Organization, Script, Tag, TaskSnapshot},
    sync::DELTA_LIMIT,
};

pub const PAGE_SIZE: usize = 25;
/// Hard stop when paging through devices, so a huge tag can't hang the app.
pub const MAX_RESOLVED_DEVICES: usize = 1000;

/// Every mutation error type implements the `Error` interface.
const MUTATION_ERRORS: &str = "errors { __typename ... on Error { message } }";

const ASSET_FIELDS: &str = "id name customer { name } site { name } agentConnection { status } operatingSystem { name }";
const TAG_FIELDS: &str = "id name __typename assetCount description updatedAt organization { id name }";
const SCRIPT_FIELDS: &str = "id name language description timeoutSeconds updatedAt inputs { name variable type }";

/// `name` is deliberately not polled: on a just-created task it resolves to
/// null despite being non-nullable, which nulls the whole task in the response.
const TASK_FIELDS: &str = "
  id
  status
  createdAt
  ... on RunScriptTask { script { id name } }
  executions {
    nodes {
      id
      status
      exitCode
      errorMessage
      startedAt
      updatedAt
      durationMilliseconds
      ... on ScriptTaskExecution {
        output {
          runAs
          stdOutput
          stdError
          outputs {
            name
            variable
            type
            value { stringValue booleanValue intValue floatValue dateTimeValue }
          }
        }
      }
    }
  }";

pub(crate) fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Whether an error is N-able refusing the API key (as opposed to anything else going wrong).
pub fn is_access_denied(error: &str) -> bool {
    error.starts_with("Access denied")
}

// ---- interpreting responses (no network) -------------------------------------

/// What came back from one operation: the data and any (tolerated) errors.
#[derive(Debug)]
pub struct Reply {
    pub data: Value,
    pub errors: Vec<String>,
}

/// Turns an HTTP status and body into data or a readable error. With
/// `tolerate_errors`, a response that has both data and errors returns the data
/// (the errors are passed back for display) instead of failing.
pub fn interpret(status: u16, body: &Value, tolerate_errors: bool) -> Result<Reply, String> {
    let errors: Vec<String> = body
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|e| e.get("message").and_then(Value::as_str).unwrap_or("Unknown error").to_string())
        .collect();
    let data = body.get("data").filter(|d| !d.is_null());

    if status == 401 || status == 403 {
        // The most common cause by far: the key was rotated, revoked or has expired.
        if errors.iter().any(|e| e.to_lowercase().contains("token is not active")) {
            return Err(format!("Access denied (HTTP {status}): your API key is no longer active. It has expired or been revoked: create a new one in the N-able dashboard and replace it in Settings."));
        }
        let detail = if errors.is_empty() { String::new() } else { format!(": {}", errors.join("; ")) };
        return Err(format!("Access denied (HTTP {status}){detail}. Check the API key in Settings."));
    }
    let usable_despite_errors = tolerate_errors && data.is_some();
    if !errors.is_empty() && !usable_despite_errors {
        return Err(errors.join("; "));
    }
    if status >= 400 {
        return Err(format!("The N-able API returned HTTP {status}."));
    }
    let data = data.ok_or("The API response contained no data.")?;
    Ok(Reply { data: data.clone(), errors })
}

/// Fails with the mutation's own error messages, if it reported any.
pub(crate) fn throw_if_errors(payload: &Value, action: &str) -> Result<(), String> {
    let errors = payload.get("errors").and_then(Value::as_array).cloned().unwrap_or_default();
    if errors.is_empty() {
        return Ok(());
    }
    let messages: Vec<&str> =
        errors.iter().map(|e| e.get("message").and_then(Value::as_str).or_else(|| e.get("__typename").and_then(Value::as_str)).unwrap_or("Unknown error")).collect();
    Err(format!("{action}: {}", messages.join("; ")))
}

pub(crate) fn parse<T: DeserializeOwned>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|e| format!("Unexpected response from the API: {e}"))
}

/// The non-null entries of a connection's `nodes`.
pub(crate) fn nodes<T: DeserializeOwned>(connection: &Value) -> Result<Vec<T>, String> {
    connection
        .get("nodes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|n| !n.is_null()).map(parse).collect())
        .unwrap_or_else(|| Ok(vec![]))
}

fn total_count(connection: &Value) -> i64 {
    connection.get("totalCount").and_then(Value::as_i64).unwrap_or(0)
}

/// Name matches first, then extra whole-word matches, de-duplicated.
fn merge_by_id<T: Clone>(id: impl Fn(&T) -> &str, lists: &[&[T]]) -> Vec<T> {
    let mut seen = std::collections::HashSet::new();
    lists.iter().flat_map(|l| l.iter()).filter(|n| seen.insert(id(n).to_string())).take(PAGE_SIZE).cloned().collect()
}

// ---- building queries (no network) -------------------------------------------

/// Script and device searches: `name: { contains }` is a case-insensitive
/// substring match, while `text: { contains }` is a whole-word search that also
/// covers descriptions. Both run in one request (aliased) and are merged. With
/// an empty term, the unfiltered search gives a browsable list.
pub fn build_search(op: &str, root: &str, fields: &str, term: &str) -> String {
    if term.is_empty() {
        return format!("query {op}($orgIds: [ID!]!) {{ byName: {root}(inOrganizations: $orgIds, first: {PAGE_SIZE}) {{ nodes {{ {fields} }} }} }}");
    }
    format!(
        "query {op}($orgIds: [ID!]!, $name: String!) {{
    byName: {root}(inOrganizations: $orgIds, where: {{ name: {{ contains: $name }} }}, first: {PAGE_SIZE}) {{ nodes {{ {fields} }} }}
    byText: {root}(inOrganizations: $orgIds, where: {{ text: {{ contains: $name }} }}, first: {PAGE_SIZE}) {{ nodes {{ {fields} }} }}
  }}"
    )
}

/// Script-run task IDs are base64 "<task uuid>:<asset uuid>".
pub fn asset_id_from_task_id(task_id: &str) -> Option<String> {
    let bytes = B64.decode(task_id).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    text.split(':').nth(1).map(String::from)
}

/// Fetches several tasks in one request using aliases (t0, t1, ...).
fn build_get_tasks(count: usize) -> String {
    let vars = (0..count).map(|i| format!("$id{i}: ID!")).collect::<Vec<_>>().join(", ");
    let fields = (0..count).map(|i| format!("t{i}: task(id: $id{i}) {{ {TASK_FIELDS} }}")).collect::<Vec<_>>().join("\n");
    format!("query GetTasks({vars}) {{ {fields} }}")
}

fn build_run_script(input: &RunScriptInput) -> Value {
    let mut configuration = json!({ "scriptId": input.script_id, "name": input.name, "timeoutSeconds": input.timeout_seconds });
    if input.run_when_online {
        configuration["shouldRunWhenOnline"] = json!(true);
    }
    if !input.inputs.is_empty() {
        configuration["inputs"] = json!(
            input.inputs.iter().map(|i| json!({ "name": i.name, "variable": i.variable, "type": i.kind_name(), "value": i.value })).collect::<Vec<_>>()
        );
    }
    json!({ "input": { "assetIds": input.asset_ids, "configuration": configuration } })
}

// ---- operations ---------------------------------------------------------------

pub struct RunInput {
    pub name: String,
    pub variable: String,
    pub kind: InputType,
    pub value: InputValue,
}

impl RunInput {
    pub(crate) fn kind_name(&self) -> &'static str {
        match self.kind {
            InputType::String => "STRING",
            InputType::Password => "PASSWORD",
            InputType::Boolean => "BOOLEAN",
            InputType::Int => "INT",
            InputType::Float => "FLOAT",
            InputType::DateTime => "DATE_TIME",
        }
    }
}

pub struct RunScriptInput {
    pub script_id: String,
    /// For the audit log.
    pub script_name: String,
    pub target_label: String,
    pub asset_ids: Vec<String>,
    pub name: String,
    pub timeout_seconds: i64,
    pub inputs: Vec<RunInput>,
    /// Queue the run for a device that is offline now (without it the API refuses an offline device).
    pub run_when_online: bool,
}

pub struct RunResult {
    pub tasks: Vec<(String, Option<String>)>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Devices {
    pub assets: Vec<Asset>,
    pub total_count: i64,
}

pub struct TaskFetch {
    pub tasks: std::collections::HashMap<String, Option<TaskSnapshot>>,
    pub errors: Vec<String>,
}

impl Core {
    /// Runs one operation. With `tolerate_errors`, partial data is returned alongside the errors.
    pub async fn gql(&self, query: &str, variables: Value, tolerate_errors: bool, audit: Option<AuditMeta>) -> Result<Reply, String> {
        let response = self.graphql(query, &variables, audit).await?;
        interpret(response.status, &response.body, tolerate_errors)
    }

    pub(crate) async fn query(&self, query: &str, variables: Value) -> Result<Value, String> {
        Ok(self.gql(query, variables, false, None).await?.data)
    }

    pub(crate) async fn mutate(&self, query: &str, variables: Value, action: &str, summary: String) -> Result<Value, String> {
        Ok(self.gql(query, variables, false, Some(AuditMeta { action: action.into(), summary })).await?.data)
    }

    pub async fn search_organizations(&self, term: &str) -> Result<Vec<Organization>, String> {
        let query = if term.is_empty() {
            format!("query FindOrgs {{ organizationSearch(first: {PAGE_SIZE}) {{ nodes {{ id name __typename }} }} }}")
        } else {
            format!("query FindOrgs($name: String!) {{ organizationSearch(where: {{ name: {{ contains: $name }} }}, first: {PAGE_SIZE}) {{ nodes {{ id name __typename }} }} }}")
        };
        let data = self.query(&query, if term.is_empty() { json!({}) } else { json!({ "name": term }) }).await?;
        nodes(&data["organizationSearch"])
    }

    pub async fn search_scripts(&self, org_id: &str, term: &str) -> Result<Vec<Script>, String> {
        let fields = "id name language description timeoutSeconds inputs { name variable type }";
        let mut vars = json!({ "orgIds": [org_id] });
        if !term.is_empty() {
            vars["name"] = json!(term);
        }
        let data = self.query(&build_search("FindScripts", "scriptSearch", fields, term), vars).await?;
        Ok(merge_by_id(|s: &Script| &s.id, &[&nodes(&data["byName"])?, &data.get("byText").map(nodes::<Script>).transpose()?.unwrap_or_default()]))
    }

    pub async fn search_assets(&self, org_id: &str, term: &str) -> Result<Vec<Asset>, String> {
        let mut vars = json!({ "orgIds": [org_id] });
        if !term.is_empty() {
            vars["name"] = json!(term);
        }
        let data = self.query(&build_search("FindAssets", "assetSearch", ASSET_FIELDS, term), vars).await?;
        Ok(merge_by_id(|a: &Asset| &a.id, &[&nodes(&data["byName"])?, &data.get("byText").map(nodes::<Asset>).transpose()?.unwrap_or_default()]))
    }

    /// All devices in `org_id` matching an `AssetWhereInput` filter, following pagination.
    pub async fn find_all_assets(&self, org_id: &str, filter: &Value) -> Result<Devices, String> {
        let query = format!(
            "query FindAllAssets($orgIds: [ID!]!, $where: AssetWhereInput, $after: String) {{
    assetSearch(inOrganizations: $orgIds, where: $where, first: 100, after: $after) {{
      totalCount
      pageInfo {{ hasNextPage endCursor }}
      nodes {{ {ASSET_FIELDS} }}
    }}
  }}"
        );
        let mut assets: Vec<Asset> = vec![];
        let mut after = Value::Null;
        let mut total = 0;
        loop {
            let data = self.query(&query, json!({ "orgIds": [org_id], "where": filter, "after": after })).await?;
            let page = &data["assetSearch"];
            total = total_count(page).max(total);
            assets.extend(nodes::<Asset>(page)?);
            let next = page["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false);
            after = page["pageInfo"]["endCursor"].clone();
            if !next || after.is_null() || assets.len() >= MAX_RESOLVED_DEVICES {
                break;
            }
        }
        Ok(Devices { assets, total_count: total })
    }

    /// First page of devices matching a filter, for live previews.
    pub async fn preview_assets(&self, org_id: &str, filter: &Value, first: u32) -> Result<Devices, String> {
        let query = format!(
            "query PreviewAssets($orgIds: [ID!]!, $where: AssetWhereInput, $first: Int) {{
      assetSearch(inOrganizations: $orgIds, where: $where, first: $first) {{ totalCount nodes {{ {ASSET_FIELDS} }} }}
    }}"
        );
        let data = self.query(&query, json!({ "orgIds": [org_id], "where": filter, "first": first })).await?;
        Ok(Devices { assets: nodes(&data["assetSearch"])?, total_count: total_count(&data["assetSearch"]) })
    }

    // ---- tags -----------------------------------------------------------------

    /// Tag search can't be scoped to an organization in the API, so tags from
    /// `prefer_org` are listed first and the rest follow.
    pub async fn search_tags(&self, term: &str, prefer_org: Option<&str>) -> Result<Vec<Tag>, String> {
        let query = format!(
            "query FindTags{} {{ tagSearch({} first: 50) {{ nodes {{ {TAG_FIELDS} }} }} }}",
            if term.is_empty() { "" } else { "($name: String!)" },
            if term.is_empty() { "" } else { "where: { name: { contains: $name } }," }
        );
        let data = self.query(&query, if term.is_empty() { json!({}) } else { json!({ "name": term }) }).await?;
        let tags: Vec<Tag> = nodes(&data["tagSearch"])?;
        Ok(prefer_tags_from(tags, prefer_org))
    }

    pub async fn get_tag(&self, id: &str) -> Result<Option<Tag>, String> {
        let query = format!("query GetTag($id: ID!) {{ tag(id: $id) {{ {TAG_FIELDS} ... on DynamicTag {{ filter }} }} }}");
        let data = self.query(&query, json!({ "id": id })).await?;
        if data["tag"].is_null() { Ok(None) } else { parse(&data["tag"]).map(Some) }
    }

    pub async fn create_static_tag(&self, org_id: &str, name: &str, description: &str, category: &str) -> Result<Tag, String> {
        let mut input = json!({ "organizationId": org_id, "name": name });
        if !description.is_empty() {
            input["description"] = json!(description);
        }
        if !category.is_empty() {
            input["category"] = json!(category);
        }
        let query = format!("mutation CreateStaticTag($input: StaticTagCreateMutationInput!) {{ staticTagCreate(input: $input) {{ item {{ {TAG_FIELDS} }} {MUTATION_ERRORS} }} }}");
        let data = self.mutate(&query, json!({ "input": input }), "tag.create", format!("Created static tag {name}")).await?;
        tag_from_payload(&data["staticTagCreate"], "Could not create the tag")
    }

    pub async fn update_static_tag(&self, id: &str, name: &str, description: &str) -> Result<Tag, String> {
        let query = format!("mutation UpdateStaticTag($input: StaticTagUpdateMutationInput!) {{ staticTagUpdate(input: $input) {{ item {{ {TAG_FIELDS} }} {MUTATION_ERRORS} }} }}");
        let data = self.mutate(&query, json!({ "input": { "id": id, "name": name, "description": description } }), "tag.update", format!("Updated static tag {name}")).await?;
        tag_from_payload(&data["staticTagUpdate"], "Could not update the tag")
    }

    pub async fn create_dynamic_tag(&self, org_id: &str, name: &str, description: &str, filter: &Value, category: &str) -> Result<Tag, String> {
        let mut input = json!({ "organizationId": org_id, "name": name, "filter": filter });
        if !description.is_empty() {
            input["description"] = json!(description);
        }
        if !category.is_empty() {
            input["category"] = json!(category);
        }
        let query = format!("mutation CreateDynamicTag($input: AssetDynamicTagCreateMutationInput!) {{ assetDynamicTagCreate(input: $input) {{ item {{ {TAG_FIELDS} }} {MUTATION_ERRORS} }} }}");
        let data = self.mutate(&query, json!({ "input": input }), "tag.create", format!("Created dynamic tag {name}")).await?;
        tag_from_payload(&data["assetDynamicTagCreate"], "Could not create the tag")
    }

    pub async fn update_dynamic_tag(&self, id: &str, name: &str, description: &str, filter: &Value) -> Result<Tag, String> {
        let query = format!("mutation UpdateDynamicTag($input: AssetDynamicTagUpdateMutationInput!) {{ assetDynamicTagUpdate(input: $input) {{ item {{ {TAG_FIELDS} }} {MUTATION_ERRORS} }} }}");
        let input = json!({ "id": id, "name": name, "description": description, "filter": filter });
        let data = self.mutate(&query, json!({ "input": input }), "tag.update", format!("Updated dynamic tag {name}")).await?;
        tag_from_payload(&data["assetDynamicTagUpdate"], "Could not update the tag")
    }

    pub async fn assign_static_tags(&self, tag_ids: &[String], asset_ids: &[String], tag_label: Option<&str>) -> Result<(), String> {
        let label = tag_label.map_or_else(|| plural(tag_ids.len(), "tag"), String::from);
        let query = format!("mutation AssignTags($input: AssetStaticTagsAssignMutationInput!) {{ assetStaticTagsAssign(input: $input) {{ items {{ asset {{ id }} }} {MUTATION_ERRORS} }} }}");
        let summary = format!("Added {} to {label}", plural(asset_ids.len(), "device"));
        let data = self.mutate(&query, json!({ "input": { "tagIds": tag_ids, "assetIds": asset_ids } }), "tag.assign", summary).await?;
        throw_if_errors(&data["assetStaticTagsAssign"], "Could not add devices to the tag")
    }

    pub async fn unassign_static_tags(&self, tag_ids: &[String], asset_ids: &[String], tag_label: Option<&str>) -> Result<(), String> {
        let label = tag_label.map_or_else(|| plural(tag_ids.len(), "tag"), String::from);
        let query = format!("mutation UnassignTags($input: AssetStaticTagsUnassignMutationInput!) {{ assetStaticTagsUnassign(input: $input) {{ items {{ asset {{ id }} }} {MUTATION_ERRORS} }} }}");
        let summary = format!("Removed {} from {label}", plural(asset_ids.len(), "device"));
        let data = self.mutate(&query, json!({ "input": { "tagIds": tag_ids, "assetIds": asset_ids } }), "tag.unassign", summary).await?;
        throw_if_errors(&data["assetStaticTagsUnassign"], "Could not remove devices from the tag")
    }

    pub async fn delete_tags(&self, ids: &[String], label: Option<&str>) -> Result<(), String> {
        let label = label.map_or_else(|| plural(ids.len(), "tag"), String::from);
        let query = format!("mutation DeleteTags($input: TagsDeleteMutationInput!) {{ tagsDelete(input: $input) {{ ids {MUTATION_ERRORS} }} }}");
        let data = self.mutate(&query, json!({ "input": { "ids": ids } }), "tag.delete", format!("Deleted {label}")).await?;
        throw_if_errors(&data["tagsDelete"], "Could not delete the tag")
    }

    // ---- script runs ----------------------------------------------------------

    /// Starts a run; the API creates one task per device.
    pub async fn run_script(&self, input: &RunScriptInput) -> Result<RunResult, String> {
        // Only `id` is selected on items: on a just-created task the non-nullable
        // `name` resolves to null and would error the whole response.
        let query = format!("mutation RunScript($input: AssetsScriptRunMutationInput!) {{ assetsScriptRun(input: $input) {{ items {{ id }} {MUTATION_ERRORS} }} }}");
        let summary = format!("Ran {} on {} ({})", input.script_name, plural(input.asset_ids.len(), "device"), input.target_label);
        let reply = self.gql(&query, build_run_script(input), true, Some(AuditMeta { action: "script.run".into(), summary })).await?;
        let mut result = run_result_from_payload(&reply.data["assetsScriptRun"]).map_err(|e| if reply.errors.is_empty() { e } else { format!("{e} {}", reply.errors.join("; ")) })?;
        result.warnings.extend(reply.errors);
        Ok(result)
    }

    pub async fn get_task(&self, task_id: &str) -> Result<(Option<TaskSnapshot>, Vec<String>), String> {
        let fetch = |fields: String| {
            let query = format!("query GetTask($id: ID!) {{ task(id: $id) {{ {fields} }} }}");
            async move { self.gql(&query, json!({ "id": task_id }), true, None).await }
        };
        let with_name = fetch(format!("name {TASK_FIELDS}")).await?;
        if !with_name.data["task"].is_null() || with_name.errors.is_empty() {
            return Ok((snapshot_from(&with_name.data["task"])?, with_name.errors));
        }
        // A just-created task's null `name` nulls the whole task: ask again without it.
        let without = fetch(TASK_FIELDS.to_string()).await?;
        Ok((snapshot_from(&without.data["task"])?, without.errors))
    }

    /// Fetches several tasks in one request.
    pub async fn get_tasks(&self, task_ids: &[String]) -> Result<TaskFetch, String> {
        let variables: serde_json::Map<String, Value> = task_ids.iter().enumerate().map(|(i, id)| (format!("id{i}"), json!(id))).collect();
        let reply = self.gql(&build_get_tasks(task_ids.len()), Value::Object(variables), true, None).await?;
        let mut tasks = std::collections::HashMap::new();
        for (i, id) in task_ids.iter().enumerate() {
            tasks.insert(id.clone(), snapshot_from(&reply.data[format!("t{i}")])?);
        }
        Ok(TaskFetch { tasks, errors: reply.errors })
    }

    // ---- full loads and deltas for the local cache ----------------------------

    /// Follows `after` cursors until the connection is exhausted.
    pub(crate) async fn fetch_all_pages<T: DeserializeOwned>(&self, root: &str, args: &str, fields: &str, variables: Value, var_defs: &str) -> Result<Vec<T>, String> {
        let query = format!(
            "query FetchAll($after: String{var_defs}) {{
      page: {root}({args}{}first: 100, after: $after) {{ totalCount pageInfo {{ hasNextPage endCursor }} nodes {{ {fields} }} }}
    }}",
            if args.is_empty() { "" } else { ", " }
        );
        let mut items = vec![];
        let mut after = Value::Null;
        loop {
            let mut vars = variables.clone();
            vars["after"] = after.clone();
            let data = self.query(&query, vars).await?;
            items.extend(nodes::<T>(&data["page"])?);
            let next = data["page"]["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false);
            after = data["page"]["pageInfo"]["endCursor"].clone();
            if !next || after.is_null() {
                return Ok(items);
            }
        }
    }

    pub async fn fetch_all_organizations(&self) -> Result<Vec<Organization>, String> {
        self.fetch_all_pages("organizationSearch", "", "id name __typename", json!({}), "").await
    }

    pub async fn count_organizations(&self) -> Result<i64, String> {
        Ok(total_count(&self.query("{ organizationSearch(first: 1) { totalCount } }", json!({})).await?["organizationSearch"]))
    }

    pub async fn fetch_all_tags(&self) -> Result<Vec<Tag>, String> {
        self.fetch_all_pages("tagSearch", "", TAG_FIELDS, json!({}), "").await
    }

    /// Tags changed after `since`, plus the current total (to spot deletions).
    pub async fn fetch_tags_changed_since(&self, since: &str) -> Result<(Vec<Tag>, i64), String> {
        let query = format!(
            "query TagDelta($since: DateTime!) {{
      changed: tagSearch(where: {{ updatedAt: {{ gt: $since }} }}, first: {DELTA_LIMIT}) {{ nodes {{ {TAG_FIELDS} }} }}
      all: tagSearch(first: 1) {{ totalCount }}
    }}"
        );
        let data = self.query(&query, json!({ "since": since })).await?;
        Ok((nodes(&data["changed"])?, total_count(&data["all"])))
    }

    pub async fn fetch_all_scripts(&self, org_id: &str) -> Result<Vec<Script>, String> {
        self.fetch_all_pages("scriptSearch", "inOrganizations: $orgIds", SCRIPT_FIELDS, json!({ "orgIds": [org_id] }), ", $orgIds: [ID!]!").await
    }

    pub async fn fetch_scripts_changed_since(&self, org_id: &str, since: &str) -> Result<(Vec<Script>, i64), String> {
        let query = format!(
            "query ScriptDelta($orgIds: [ID!]!, $since: DateTime!) {{
      changed: scriptSearch(inOrganizations: $orgIds, where: {{ updatedAt: {{ gt: $since }} }}, first: {DELTA_LIMIT}) {{ nodes {{ {SCRIPT_FIELDS} }} }}
      all: scriptSearch(inOrganizations: $orgIds, first: 1) {{ totalCount }}
    }}"
        );
        let data = self.query(&query, json!({ "orgIds": [org_id], "since": since })).await?;
        Ok((nodes(&data["changed"])?, total_count(&data["all"])))
    }

    pub async fn get_asset_name(&self, asset_id: &str) -> Result<Option<String>, String> {
        let data = self.query("query GetAsset($id: ID!) { asset(id: $id) { id name } }", json!({ "id": asset_id })).await?;
        Ok(data["asset"]["name"].as_str().map(String::from))
    }
}

fn prefer_tags_from(tags: Vec<Tag>, prefer_org: Option<&str>) -> Vec<Tag> {
    let Some(org) = prefer_org else { return tags.into_iter().take(PAGE_SIZE).collect() };
    let from_org = |t: &Tag| t.organization.as_ref().is_some_and(|o| o.id == org);
    let (own, rest): (Vec<Tag>, Vec<Tag>) = tags.into_iter().partition(from_org);
    own.into_iter().chain(rest).take(PAGE_SIZE).collect()
}

fn tag_from_payload(payload: &Value, action: &str) -> Result<Tag, String> {
    throw_if_errors(payload, action)?;
    parse(&payload["item"])
}

fn snapshot_from(value: &Value) -> Result<Option<TaskSnapshot>, String> {
    if value.is_null() { Ok(None) } else { parse(value).map(Some) }
}

fn run_result_from_payload(payload: &Value) -> Result<RunResult, String> {
    let ids: Vec<String> = payload["items"].as_array().map(|a| a.iter().filter_map(|i| i["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    let warnings: Vec<String> = payload["errors"]
        .as_array()
        .map(|a| a.iter().map(|e| e["message"].as_str().or_else(|| e["__typename"].as_str()).unwrap_or("Unknown error").to_string()).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return Err(format!("The script run was not created. {}", warnings.join("; ")).trim().to_string());
    }
    Ok(RunResult { tasks: ids.into_iter().map(|id| { let asset = asset_id_from_task_id(&id); (id, asset) }).collect(), warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_device_from_a_task_id() {
        let id = B64.encode("3f2c9a1e-0000-4000-8000-000000000001:9b7d5e2a-1111-4111-8111-000000000002");
        assert_eq!(asset_id_from_task_id(&id).as_deref(), Some("9b7d5e2a-1111-4111-8111-000000000002"));
        assert_eq!(asset_id_from_task_id("!!not base64!!"), None);
        assert_eq!(asset_id_from_task_id(&B64.encode("no-colon")), None);
    }

    #[test]
    fn search_runs_both_name_and_whole_word_matches_when_there_is_a_term() {
        let q = build_search("FindScripts", "scriptSearch", "id name", "bit");
        assert!(q.contains("byName: scriptSearch") && q.contains("name: { contains: $name }"));
        assert!(q.contains("byText: scriptSearch") && q.contains("text: { contains: $name }"));
        let browse = build_search("FindScripts", "scriptSearch", "id name", "");
        assert!(browse.contains("byName") && !browse.contains("byText") && !browse.contains("$name"));
    }

    #[test]
    fn polling_batches_tasks_under_aliases_and_never_asks_for_the_name() {
        let q = build_get_tasks(2);
        assert!(q.contains("$id0: ID!, $id1: ID!") && q.contains("t0: task(id: $id0)") && q.contains("t1: task(id: $id1)"));
        assert!(!q.contains("\n  name\n") && !q.contains("task(id: $id0) { name"), "name nulls a just-created task");
    }

    #[test]
    fn run_script_sends_typed_inputs_only_when_there_are_any() {
        let mut input = RunScriptInput {
            script_id: "s1".into(),
            script_name: "Cleanup".into(),
            target_label: "PC".into(),
            asset_ids: vec!["a1".into(), "a2".into()],
            name: "Run Cleanup".into(),
            timeout_seconds: 120,
            inputs: vec![],
            run_when_online: false,
        };
        let vars = build_run_script(&input);
        assert_eq!(vars["input"]["assetIds"], json!(["a1", "a2"]));
        assert_eq!(vars["input"]["configuration"], json!({ "scriptId": "s1", "name": "Run Cleanup", "timeoutSeconds": 120 }));
        input.inputs = vec![RunInput { name: "Count".into(), variable: "n".into(), kind: InputType::Int, value: InputValue { int_value: Some(3), ..Default::default() } }];
        let vars = build_run_script(&input);
        assert_eq!(vars["input"]["configuration"]["inputs"], json!([{ "name": "Count", "variable": "n", "type": "INT", "value": { "intValue": 3 } }]));
    }

    #[test]
    fn interprets_responses_like_the_desktop_app() {
        let ok = json!({ "data": { "x": 1 } });
        assert_eq!(interpret(200, &ok, false).unwrap().data, json!({ "x": 1 }));

        let denied = interpret(401, &json!({ "errors": [{ "message": "bad token" }] }), false).unwrap_err();
        assert!(denied.contains("Access denied (HTTP 401): bad token") && denied.contains("Settings"));

        let partial = json!({ "data": { "t0": null }, "errors": [{ "message": "output not readable yet" }] });
        assert_eq!(interpret(200, &partial, false).unwrap_err(), "output not readable yet");
        let tolerated = interpret(200, &partial, true).unwrap();
        assert_eq!(tolerated.errors, ["output not readable yet"]);

        assert!(interpret(500, &json!({ "data": { "x": 1 } }), false).unwrap_err().contains("HTTP 500"));
        assert!(interpret(200, &json!({}), false).unwrap_err().contains("no data"));
        assert!(interpret(200, &json!({ "data": null }), false).unwrap_err().contains("no data"));
    }

    #[test]
    fn an_inactive_token_says_what_happened_and_what_to_do() {
        let body = json!({ "errors": [{ "message": "denied by external authz, unauthorized: token is not active" }] });
        let error = interpret(403, &body, false).unwrap_err();
        assert!(error.contains("no longer active") && error.contains("expired or been revoked") && error.contains("new one") && error.contains("Settings"), "{error}");
        assert!(is_access_denied(&error));
        assert!(!is_access_denied("The task wasn't found."));
        assert!(!is_access_denied("HTTP 502: the API returned a non-JSON response: x"));
    }

    #[test]
    fn mutation_errors_become_readable_messages() {
        let payload = json!({ "item": null, "errors": [{ "__typename": "StaticTagAlreadyExistsError", "message": "Tag with this name already exists" }] });
        assert_eq!(tag_from_payload(&payload, "Could not create the tag").unwrap_err(), "Could not create the tag: Tag with this name already exists");
        let no_message = json!({ "errors": [{ "__typename": "SomeError" }] });
        assert_eq!(throw_if_errors(&no_message, "Failed").unwrap_err(), "Failed: SomeError");
        assert!(throw_if_errors(&json!({ "errors": [] }), "x").is_ok());
        assert!(throw_if_errors(&json!({ "errors": null }), "x").is_ok());
    }

    #[test]
    fn a_run_that_created_no_tasks_is_an_error_with_the_apis_reasons() {
        let payload = json!({ "items": [], "errors": [{ "message": "Device offline" }] });
        assert_eq!(run_result_from_payload(&payload).err().unwrap(), "The script run was not created. Device offline");
        let task = B64.encode("task-1:asset-1");
        let ok = run_result_from_payload(&json!({ "items": [{ "id": task }], "errors": [{ "message": "PC-9 skipped" }] })).unwrap();
        assert_eq!(ok.tasks, [(task, Some("asset-1".to_string()))]);
        assert_eq!(ok.warnings, ["PC-9 skipped"]);
    }

    #[test]
    fn tags_from_the_preferred_organization_come_first() {
        let tag = |id: &str, org: &str| Tag { id: id.into(), name: id.into(), typename: "StaticTag".into(), asset_count: None, updated_at: None, description: None, organization: Some(crate::model::OrgRef { id: org.into(), name: org.into() }), filter: None };
        let out = prefer_tags_from(vec![tag("a", "o2"), tag("b", "o1"), tag("c", "o2"), tag("d", "o1")], Some("o1"));
        assert_eq!(out.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["b", "d", "a", "c"]);
    }

    #[test]
    fn merging_search_results_keeps_name_matches_first_and_drops_duplicates() {
        let org = |id: &str| Organization { id: id.into(), name: id.into(), typename: String::new() };
        let merged = merge_by_id(|o: &Organization| &o.id, &[&[org("1"), org("2")], &[org("2"), org("3")]]);
        assert_eq!(merged.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
    }

    #[test]
    fn parses_a_realistic_task_response() {
        let reply = json!({ "data": { "t0": {
            "id": "x", "status": "IN_PROGRESS", "createdAt": "2026-10-17T12:00:00Z",
            "script": { "id": "s", "name": "Cleanup" },
            "executions": { "nodes": [null, { "id": "e", "status": "RUNNING", "exitCode": null, "errorMessage": null, "startedAt": null, "updatedAt": null, "durationMilliseconds": null, "output": null }] }
        }, "t1": null } });
        let r = interpret(200, &reply, true).unwrap();
        let t0 = snapshot_from(&r.data["t0"]).unwrap().unwrap();
        assert_eq!(t0.executions.unwrap().nodes.iter().flatten().count(), 1);
        assert!(snapshot_from(&r.data["t1"]).unwrap().is_none());
    }
}
