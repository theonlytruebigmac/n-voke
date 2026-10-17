import { invoke } from "@tauri-apps/api/core";
import type { Asset, InputType, InputValue, Organization, Script, Tag, TaskSnapshot } from "./types";

export class ApiError extends Error {}

export interface Settings {
  apiUrl: string;
  hasApiKey: boolean;
  /** Days of run history kept on disk; 0 = not saved. */
  historyDays: number;
  /** Local data can be encrypted (the system keychain is available). */
  encryptionAvailable: boolean;
  encryptionError: string | null;
}

export const getSettings = () => invoke<Settings>("get_settings");
export const saveSettings = (apiUrl: string, apiKey?: string, historyDays?: number) =>
  invoke<Settings>("save_settings", { apiUrl, apiKey: apiKey || null, historyDays: historyDays ?? null });
export const clearApiKey = () => invoke<void>("clear_api_key");

export interface AuditEntry {
  timestamp: string;
  user: string;
  action: string;
  summary: string;
  outcome: "ok" | "error";
  error?: string;
  details?: { operation?: string | null; variables?: unknown } & Record<string, unknown>;
}

export const getAuditLog = (limit = 500) => invoke<{ path: string; entries: AuditEntry[] }>("get_audit_log", { limit });

/** Readable context recorded with a mutation in the local audit log. */
export interface AuditMeta {
  action: string;
  summary: string;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

interface RawResponse {
  status: number;
  body: { data?: unknown; errors?: { message?: string }[] } | null;
}

/**
 * Runs one GraphQL operation through the Rust side. With `tolerateErrors`,
 * a response that has both data and errors returns the data (the errors are
 * passed back for display) instead of throwing.
 */
export async function gql<T>(
  query: string,
  variables: Record<string, unknown> = {},
  { tolerateErrors = false, audit }: { tolerateErrors?: boolean; audit?: AuditMeta } = {},
): Promise<{ data: T; errors: string[] }> {
  let res: RawResponse;
  try {
    res = await invoke<RawResponse>("graphql", { query, variables, audit: audit ?? null });
  } catch (e) {
    throw new ApiError(String(e));
  }

  const { status, body } = res;
  const errors = (body?.errors ?? []).map((e) => e?.message ?? "Unknown error");
  const hasData = body?.data != null;

  if (status === 401 || status === 403) {
    const detail = errors.length ? `: ${errors.join("; ")}` : "";
    throw new ApiError(`Access denied (HTTP ${status})${detail}. Check the API key in Settings.`);
  }
  if (errors.length && !(tolerateErrors && hasData)) {
    throw new ApiError(errors.join("; "));
  }
  if (status >= 400) throw new ApiError(`The N-able API returned HTTP ${status}.`);
  if (!hasData) throw new ApiError("The API response contained no data.");
  return { data: body!.data as T, errors };
}

/** Every mutation error type implements the `Error` interface. */
const MUTATION_ERRORS = "errors { __typename ... on Error { message } }";
type MutationErrors = { __typename: string; message?: string }[] | null;

function throwIfErrors(errors: MutationErrors, action: string) {
  if (errors?.length) {
    throw new ApiError(`${action}: ${errors.map((e) => e.message ?? e.__typename).join("; ")}`);
  }
}

export const PAGE_SIZE = 25;

/** Name matches first, then extra whole-word matches, de-duplicated. */
function mergeById<T extends { id: string }>(...lists: (T[] | null | undefined)[]): T[] {
  const seen = new Set<string>();
  return lists
    .flatMap((l) => l ?? [])
    .filter((n) => n && !seen.has(n.id) && seen.add(n.id))
    .slice(0, PAGE_SIZE);
}

export async function searchOrganizations(term: string): Promise<Organization[]> {
  const where = term ? `where: { name: { contains: $name } },` : "";
  const query = `query FindOrgs${term ? "($name: String!)" : ""} {
    organizationSearch(${where} first: ${PAGE_SIZE}) { nodes { id name __typename } }
  }`;
  const { data } = await gql<{ organizationSearch: { nodes: Organization[] } }>(query, term ? { name: term } : {});
  return data.organizationSearch.nodes.filter(Boolean);
}

/*
 * Script and device searches: `name: { contains }` is a case-insensitive
 * substring match, while `text: { contains }` is a whole-word search that
 * also covers descriptions. Both run in one request (aliased) and are merged.
 * With an empty term, the unfiltered search gives a browsable list.
 */
function buildSearch(op: string, root: string, fields: string, term: string) {
  if (!term) {
    return `query ${op}($orgIds: [ID!]!) {
      byName: ${root}(inOrganizations: $orgIds, first: ${PAGE_SIZE}) { nodes { ${fields} } }
    }`;
  }
  return `query ${op}($orgIds: [ID!]!, $name: String!) {
    byName: ${root}(inOrganizations: $orgIds, where: { name: { contains: $name } }, first: ${PAGE_SIZE}) { nodes { ${fields} } }
    byText: ${root}(inOrganizations: $orgIds, where: { text: { contains: $name } }, first: ${PAGE_SIZE}) { nodes { ${fields} } }
  }`;
}

type MergedSearch<T> = { byName: { nodes: T[] }; byText?: { nodes: T[] } };

export async function searchScripts(orgId: string, term: string): Promise<Script[]> {
  const fields = "id name language description timeoutSeconds inputs { name variable type }";
  const { data } = await gql<MergedSearch<Script>>(buildSearch("FindScripts", "scriptSearch", fields, term), {
    orgIds: [orgId],
    ...(term ? { name: term } : {}),
  });
  return mergeById(data.byName.nodes, data.byText?.nodes);
}

const ASSET_FIELDS = "id name customer { name } site { name } agentConnection { status } operatingSystem { name }";

export async function searchAssets(orgId: string, term: string): Promise<Asset[]> {
  const { data } = await gql<MergedSearch<Asset>>(buildSearch("FindAssets", "assetSearch", ASSET_FIELDS, term), {
    orgIds: [orgId],
    ...(term ? { name: term } : {}),
  });
  return mergeById(data.byName.nodes, data.byText?.nodes);
}

export async function getAssetName(assetId: string): Promise<string | null> {
  const { data } = await gql<{ asset: { name: string } | null }>(
    `query GetAsset($id: ID!) { asset(id: $id) { id name } }`,
    { id: assetId },
  );
  return data.asset?.name ?? null;
}

/** Hard stop when paging through devices, so a huge tag can't hang the app. */
export const MAX_RESOLVED_DEVICES = 1000;

/**
 * All devices in `orgId` matching an `AssetWhereInput` filter, following
 * pagination. Returns the devices and the API's total count.
 */
export async function findAllAssets(orgId: string, where: object): Promise<{ assets: Asset[]; totalCount: number }> {
  const query = `query FindAllAssets($orgIds: [ID!]!, $where: AssetWhereInput, $after: String) {
    assetSearch(inOrganizations: $orgIds, where: $where, first: 100, after: $after) {
      totalCount
      pageInfo { hasNextPage endCursor }
      nodes { ${ASSET_FIELDS} }
    }
  }`;
  const assets: Asset[] = [];
  let after: string | null = null;
  let totalCount = 0;
  do {
    const { data }: { data: { assetSearch: { totalCount: number; pageInfo: { hasNextPage: boolean; endCursor: string | null }; nodes: Asset[] } } } =
      await gql(query, { orgIds: [orgId], where, after });
    totalCount = data.assetSearch.totalCount;
    assets.push(...data.assetSearch.nodes.filter(Boolean));
    after = data.assetSearch.pageInfo.hasNextPage ? data.assetSearch.pageInfo.endCursor : null;
  } while (after && assets.length < MAX_RESOLVED_DEVICES);
  return { assets, totalCount };
}

/** First page of devices matching a filter, for live previews. */
export async function previewAssets(orgId: string, where: object, first = 10): Promise<{ assets: Asset[]; totalCount: number }> {
  const { data } = await gql<{ assetSearch: { totalCount: number; nodes: Asset[] } }>(
    `query PreviewAssets($orgIds: [ID!]!, $where: AssetWhereInput, $first: Int) {
      assetSearch(inOrganizations: $orgIds, where: $where, first: $first) { totalCount nodes { ${ASSET_FIELDS} } }
    }`,
    { orgIds: [orgId], where, first },
  );
  return { assets: data.assetSearch.nodes.filter(Boolean), totalCount: data.assetSearch.totalCount };
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

const TAG_FIELDS = "id name __typename assetCount description updatedAt organization { id name }";

/**
 * Tag search can't be scoped to an organization in the API, so tags from
 * `preferOrgId` are listed first and the rest follow.
 */
export async function searchTags(term: string, preferOrgId?: string): Promise<Tag[]> {
  const query = `query FindTags${term ? "($name: String!)" : ""} {
    tagSearch(${term ? "where: { name: { contains: $name } }," : ""} first: 50) { nodes { ${TAG_FIELDS} } }
  }`;
  const { data } = await gql<{ tagSearch: { nodes: Tag[] } }>(query, term ? { name: term } : {});
  const tags = data.tagSearch.nodes.filter(Boolean);
  if (!preferOrgId) return tags.slice(0, PAGE_SIZE);
  const own = tags.filter((t) => t.organization?.id === preferOrgId);
  return [...own, ...tags.filter((t) => t.organization?.id !== preferOrgId)].slice(0, PAGE_SIZE);
}

export async function getTag(id: string): Promise<Tag | null> {
  const { data } = await gql<{ tag: Tag | null }>(
    `query GetTag($id: ID!) { tag(id: $id) { ${TAG_FIELDS} ... on DynamicTag { filter } } }`,
    { id },
  );
  return data.tag;
}

export async function createStaticTag(organizationId: string, name: string, description: string, category = ""): Promise<Tag> {
  const { data } = await gql<{ staticTagCreate: { item: Tag | null; errors: MutationErrors } }>(
    `mutation CreateStaticTag($input: StaticTagCreateMutationInput!) {
      staticTagCreate(input: $input) { item { ${TAG_FIELDS} } ${MUTATION_ERRORS} }
    }`,
    { input: { organizationId, name, ...(description ? { description } : {}), ...(category ? { category } : {}) } },
    { audit: { action: "tag.create", summary: `Created static tag ${name}` } },
  );
  throwIfErrors(data.staticTagCreate.errors, "Could not create the tag");
  return data.staticTagCreate.item!;
}

export async function updateStaticTag(id: string, name: string, description: string): Promise<Tag> {
  const { data } = await gql<{ staticTagUpdate: { item: Tag | null; errors: MutationErrors } }>(
    `mutation UpdateStaticTag($input: StaticTagUpdateMutationInput!) {
      staticTagUpdate(input: $input) { item { ${TAG_FIELDS} } ${MUTATION_ERRORS} }
    }`,
    { input: { id, name, description } },
    { audit: { action: "tag.update", summary: `Updated static tag ${name}` } },
  );
  throwIfErrors(data.staticTagUpdate.errors, "Could not update the tag");
  return data.staticTagUpdate.item!;
}

export async function createDynamicTag(organizationId: string, name: string, description: string, filter: object, category = ""): Promise<Tag> {
  const { data } = await gql<{ assetDynamicTagCreate: { item: Tag | null; errors: MutationErrors } }>(
    `mutation CreateDynamicTag($input: AssetDynamicTagCreateMutationInput!) {
      assetDynamicTagCreate(input: $input) { item { ${TAG_FIELDS} } ${MUTATION_ERRORS} }
    }`,
    { input: { organizationId, name, filter, ...(description ? { description } : {}), ...(category ? { category } : {}) } },
    { audit: { action: "tag.create", summary: `Created dynamic tag ${name}` } },
  );
  throwIfErrors(data.assetDynamicTagCreate.errors, "Could not create the tag");
  return data.assetDynamicTagCreate.item!;
}

export async function updateDynamicTag(id: string, name: string, description: string, filter: object): Promise<Tag> {
  const { data } = await gql<{ assetDynamicTagUpdate: { item: Tag | null; errors: MutationErrors } }>(
    `mutation UpdateDynamicTag($input: AssetDynamicTagUpdateMutationInput!) {
      assetDynamicTagUpdate(input: $input) { item { ${TAG_FIELDS} } ${MUTATION_ERRORS} }
    }`,
    { input: { id, name, description, filter } },
    { audit: { action: "tag.update", summary: `Updated dynamic tag ${name}` } },
  );
  throwIfErrors(data.assetDynamicTagUpdate.errors, "Could not update the tag");
  return data.assetDynamicTagUpdate.item!;
}

export async function assignStaticTags(tagIds: string[], assetIds: string[], tagLabel = plural(tagIds.length, "tag")): Promise<void> {
  const { data } = await gql<{ assetStaticTagsAssign: { errors: MutationErrors } }>(
    `mutation AssignTags($input: AssetStaticTagsAssignMutationInput!) {
      assetStaticTagsAssign(input: $input) { items { asset { id } } ${MUTATION_ERRORS} }
    }`,
    { input: { tagIds, assetIds } },
    { audit: { action: "tag.assign", summary: `Added ${plural(assetIds.length, "device")} to ${tagLabel}` } },
  );
  throwIfErrors(data.assetStaticTagsAssign.errors, "Could not add devices to the tag");
}

export async function unassignStaticTags(tagIds: string[], assetIds: string[], tagLabel = plural(tagIds.length, "tag")): Promise<void> {
  const { data } = await gql<{ assetStaticTagsUnassign: { errors: MutationErrors } }>(
    `mutation UnassignTags($input: AssetStaticTagsUnassignMutationInput!) {
      assetStaticTagsUnassign(input: $input) { items { asset { id } } ${MUTATION_ERRORS} }
    }`,
    { input: { tagIds, assetIds } },
    { audit: { action: "tag.unassign", summary: `Removed ${plural(assetIds.length, "device")} from ${tagLabel}` } },
  );
  throwIfErrors(data.assetStaticTagsUnassign.errors, "Could not remove devices from the tag");
}

export async function deleteTags(ids: string[], label = plural(ids.length, "tag")): Promise<void> {
  const { data } = await gql<{ tagsDelete: { errors: MutationErrors } }>(
    `mutation DeleteTags($input: TagsDeleteMutationInput!) { tagsDelete(input: $input) { ids ${MUTATION_ERRORS} } }`,
    { input: { ids } },
    { audit: { action: "tag.delete", summary: `Deleted ${label}` } },
  );
  throwIfErrors(data.tagsDelete.errors, "Could not delete the tag");
}

// ---------------------------------------------------------------------------
// Script runs
// ---------------------------------------------------------------------------

export interface RunScriptInput {
  scriptId: string;
  /** For the audit log. */
  scriptName: string;
  targetLabel: string;
  assetIds: string[];
  name: string;
  timeoutSeconds: number;
  inputs: { name: string; variable: string; type: InputType; value: InputValue }[];
}

/** Starts a run; the API creates one task per device. */
export async function runScript(input: RunScriptInput): Promise<{ tasks: { taskId: string; assetId: string | null }[]; warnings: string[] }> {
  // Only `id` is selected on items: on a just-created task the non-nullable
  // `name` resolves to null and would error the whole response.
  const query = `mutation RunScript($input: AssetsScriptRunMutationInput!) {
    assetsScriptRun(input: $input) { items { id } ${MUTATION_ERRORS} }
  }`;
  const configuration: Record<string, unknown> = {
    scriptId: input.scriptId,
    name: input.name,
    timeoutSeconds: input.timeoutSeconds,
  };
  if (input.inputs.length) configuration.inputs = input.inputs;

  const { data } = await gql<{ assetsScriptRun: { items: { id: string }[] | null; errors: MutationErrors } }>(
    query,
    { input: { assetIds: input.assetIds, configuration } },
    {
      audit: {
        action: "script.run",
        summary: `Ran ${input.scriptName} on ${plural(input.assetIds.length, "device")} (${input.targetLabel})`,
      },
    },
  );

  const items = data.assetsScriptRun.items ?? [];
  const warnings = (data.assetsScriptRun.errors ?? []).map((e) => e.message ?? e.__typename);
  if (!items.length) throw new ApiError(`The script run was not created. ${warnings.join("; ")}`.trim());
  return { tasks: items.map((i) => ({ taskId: i.id, assetId: assetIdFromTaskId(i.id) })), warnings };
}

// `name` is deliberately not polled: on a just-created task it resolves to
// null despite being non-nullable, which nulls the whole task in the response.
const TASK_FIELDS = `
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
  }`;

export async function getTask(taskId: string): Promise<{ task: TaskSnapshot | null; errors: string[] }> {
  const fetchTask = (fields: string) =>
    gql<{ task: TaskSnapshot | null }>(`query GetTask($id: ID!) { task(id: $id) { ${fields} } }`, { id: taskId }, { tolerateErrors: true });
  const withName = await fetchTask(`name ${TASK_FIELDS}`);
  if (withName.data.task || !withName.errors.length) return { task: withName.data.task, errors: withName.errors };
  // A just-created task's null `name` nulls the whole task: ask again without it.
  const { data, errors } = await fetchTask(TASK_FIELDS);
  return { task: data.task, errors };
}

/** Fetches several tasks in one request using aliases (t0, t1, ...). */
export async function getTasks(taskIds: string[]): Promise<{ tasks: Map<string, TaskSnapshot | null>; errors: string[] }> {
  const vars = taskIds.map((_, i) => `$id${i}: ID!`).join(", ");
  const fields = taskIds.map((_, i) => `t${i}: task(id: $id${i}) { ${TASK_FIELDS} }`).join("\n");
  const variables = Object.fromEntries(taskIds.map((id, i) => [`id${i}`, id]));
  const { data, errors } = await gql<Record<string, TaskSnapshot | null>>(`query GetTasks(${vars}) { ${fields} }`, variables, {
    tolerateErrors: true,
  });
  return { tasks: new Map(taskIds.map((id, i) => [id, data[`t${i}`] ?? null])), errors };
}

/** Script-run task IDs are base64 "<task uuid>:<asset uuid>". */
export function assetIdFromTaskId(taskId: string): string | null {
  try {
    return atob(taskId).split(":")[1] ?? null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Full loads and deltas for the local cache (see sync.ts / collections.ts)
// ---------------------------------------------------------------------------

/** Most items one delta request returns; a full delta means there may be more. */
export const DELTA_LIMIT = 100;

type Page<T> = { totalCount: number; pageInfo: { hasNextPage: boolean; endCursor: string | null }; nodes: T[] };

/** Follows `after` cursors until the connection is exhausted. */
async function fetchAllPages<T>(root: string, args: string, fields: string, variables: Record<string, unknown> = {}, varDefs = ""): Promise<T[]> {
  const items: T[] = [];
  let after: string | null = null;
  do {
    const query = `query FetchAll($after: String${varDefs}) {
      page: ${root}(${args}${args ? ", " : ""}first: 100, after: $after) { totalCount pageInfo { hasNextPage endCursor } nodes { ${fields} } }
    }`;
    const { data }: { data: { page: Page<T> } } = await gql(query, { ...variables, after });
    items.push(...data.page.nodes.filter(Boolean));
    after = data.page.pageInfo.hasNextPage ? data.page.pageInfo.endCursor : null;
  } while (after);
  return items;
}

const SCRIPT_FIELDS = "id name language description timeoutSeconds updatedAt inputs { name variable type }";

export const fetchAllOrganizations = () => fetchAllPages<Organization>("organizationSearch", "", "id name __typename");

export async function countOrganizations(): Promise<number> {
  const { data } = await gql<{ organizationSearch: { totalCount: number } }>(`{ organizationSearch(first: 1) { totalCount } }`);
  return data.organizationSearch.totalCount;
}

export const fetchAllTags = () => fetchAllPages<Tag>("tagSearch", "", TAG_FIELDS);

/** Tags changed after `since`, plus the current total (to spot deletions). */
export async function fetchTagsChangedSince(since: string): Promise<{ changed: Tag[]; total: number }> {
  const { data } = await gql<{ changed: { nodes: Tag[] }; all: { totalCount: number } }>(
    `query TagDelta($since: DateTime!) {
      changed: tagSearch(where: { updatedAt: { gt: $since } }, first: ${DELTA_LIMIT}) { nodes { ${TAG_FIELDS} } }
      all: tagSearch(first: 1) { totalCount }
    }`,
    { since },
  );
  return { changed: data.changed.nodes.filter(Boolean), total: data.all.totalCount };
}

export const fetchAllScripts = (orgId: string) =>
  fetchAllPages<Script>("scriptSearch", "inOrganizations: $orgIds", SCRIPT_FIELDS, { orgIds: [orgId] }, ", $orgIds: [ID!]!");

export async function fetchScriptsChangedSince(orgId: string, since: string): Promise<{ changed: Script[]; total: number }> {
  const { data } = await gql<{ changed: { nodes: Script[] }; all: { totalCount: number } }>(
    `query ScriptDelta($orgIds: [ID!]!, $since: DateTime!) {
      changed: scriptSearch(inOrganizations: $orgIds, where: { updatedAt: { gt: $since } }, first: ${DELTA_LIMIT}) { nodes { ${SCRIPT_FIELDS} } }
      all: scriptSearch(inOrganizations: $orgIds, first: 1) { totalCount }
    }`,
    { orgIds: [orgId], since },
  );
  return { changed: data.changed.nodes.filter(Boolean), total: data.all.totalCount };
}
