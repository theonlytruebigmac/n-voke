import { queryOptions, useQuery } from "@tanstack/react-query";
import {
  DELTA_LIMIT,
  countOrganizations,
  fetchAllOrganizations,
  fetchAllScripts,
  fetchAllTags,
  fetchScriptsChangedSince,
  fetchTagsChangedSince,
  findAllAssets,
  getTag,
  searchAssets,
} from "./api";
import { queryClient } from "./queryClient";
import { byName, deltaSince, matches, maxUpdatedAt, mergeDelta, type Synced } from "./sync";
import type { Asset, Organization, Script, Tag } from "./types";

/*
 * Cached collections. Small reference lists (organizations, tags, scripts)
 * are held in full and searched locally; tags and scripts refresh by delta
 * (only what changed since the last sync). Device lists and tag members are
 * cached per query and refreshed in the background (stale-while-revalidate).
 */

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
/** Even with deltas, do a full reload this often to correct any drift. */
const FULL_RELOAD_EVERY = 6 * HOUR;
const RESULTS = 50;

export const keys = {
  orgs: ["orgs"] as const,
  tags: ["tags"] as const,
  scripts: (orgId: string) => ["scripts", orgId] as const,
  assets: (orgId: string, term: string) => ["assets", orgId, term] as const,
  tag: (id: string) => ["tag", id] as const,
  members: (orgId: string, tagId: string) => ["members", orgId, tagId] as const,
};

/** Full load first time (or when stale/out of sync), otherwise merge a delta. */
async function syncCollection<T extends { id: string; name: string; updatedAt?: string | null }>(
  key: readonly unknown[],
  fetchAll: () => Promise<T[]>,
  fetchDelta: (since: string) => Promise<{ changed: T[]; total: number }>,
): Promise<Synced<T>> {
  const prev = queryClient.getQueryData<Synced<T>>(key);
  const full = async (): Promise<Synced<T>> => {
    const items = (await fetchAll()).sort(byName);
    return { items, cursor: maxUpdatedAt(items), fullAt: Date.now() };
  };
  if (!prev || !prev.cursor || Date.now() - prev.fullAt > FULL_RELOAD_EVERY) return full();

  const { changed, total } = await fetchDelta(deltaSince(prev.cursor));
  // A full page may be cut short, and moving the cursor past it would skip the rest for good.
  if (changed.length >= DELTA_LIMIT) return full();
  const items = mergeDelta(prev.items, changed);
  // Deltas can't report deletions; a count mismatch means something went away.
  if (items.length !== total) return full();
  if (items === prev.items) return prev; // nothing changed: keep the same object (no re-render)
  return { items, cursor: maxUpdatedAt(items) ?? prev.cursor, fullAt: prev.fullAt };
}

export const tagsQuery = queryOptions({
  queryKey: keys.tags,
  queryFn: () => syncCollection<Tag>(keys.tags, fetchAllTags, fetchTagsChangedSince),
  staleTime: 30_000,
  refetchInterval: 2 * MINUTE,
});

export const scriptsQuery = (orgId: string) =>
  queryOptions({
    queryKey: keys.scripts(orgId),
    queryFn: () =>
      syncCollection<Script>(
        keys.scripts(orgId),
        () => fetchAllScripts(orgId),
        (since) => fetchScriptsChangedSince(orgId, since),
      ),
    staleTime: 2 * MINUTE,
  });

interface OrgDirectory {
  items: Organization[];
  fullAt: number;
}

/**
 * Organizations have no `updatedAt` filter: a cheap count check decides
 * whether the full list needs reloading.
 */
export const orgsQuery = queryOptions({
  queryKey: keys.orgs,
  queryFn: async (): Promise<OrgDirectory> => {
    const prev = queryClient.getQueryData<OrgDirectory>(keys.orgs);
    if (prev && Date.now() - prev.fullAt < FULL_RELOAD_EVERY && (await countOrganizations()) === prev.items.length) return prev;
    return { items: (await fetchAllOrganizations()).sort(byName), fullAt: Date.now() };
  },
  staleTime: 10 * MINUTE,
});

export const membersQuery = (orgId: string, tagId: string) =>
  queryOptions({
    queryKey: keys.members(orgId, tagId),
    queryFn: () => findAllAssets(orgId, { tags: { id: { in: [tagId] } } }),
    staleTime: MINUTE,
  });

export const tagQuery = (id: string) => queryOptions({ queryKey: keys.tag(id), queryFn: () => getTag(id), staleTime: MINUTE });

export const useTags = () => useQuery(tagsQuery);

/*
 * ensureQueryData with revalidateIfStale: returns cached data immediately when
 * there is any (refreshing in the background if it's stale), else waits for it.
 */

// ---------------------------------------------------------------------------
// Searches used by the pickers: instant, local where the data is local.
// ---------------------------------------------------------------------------

export async function searchOrganizationsLocal(term: string): Promise<Organization[]> {
  const { items } = await queryClient.ensureQueryData({ ...orgsQuery, revalidateIfStale: true });
  return items.filter((o) => matches(term, o.name)).slice(0, RESULTS);
}

/** Name or description substring match (better than the API's whole-word search). */
export async function searchScriptsLocal(orgId: string, term: string): Promise<Script[]> {
  const { items } = await queryClient.ensureQueryData({ ...scriptsQuery(orgId), revalidateIfStale: true });
  return items.filter((s) => matches(term, s.name, s.description)).slice(0, RESULTS);
}

/** Tags from `preferOrgId` first, then the rest. */
export async function searchTagsLocal(term: string, preferOrgId?: string): Promise<Tag[]> {
  const { items } = await queryClient.ensureQueryData({ ...tagsQuery, revalidateIfStale: true });
  const hits = items.filter((t) => matches(term, t.name));
  if (!preferOrgId) return hits.slice(0, RESULTS);
  return [...hits.filter((t) => t.organization?.id === preferOrgId), ...hits.filter((t) => t.organization?.id !== preferOrgId)].slice(0, RESULTS);
}

/** Devices can number in the thousands, so they're searched on the server, with each search cached. */
export function searchAssetsCached(orgId: string, term: string): Promise<Asset[]> {
  return queryClient.ensureQueryData({
    queryKey: keys.assets(orgId, term.trim()),
    queryFn: () => searchAssets(orgId, term.trim()),
    staleTime: MINUTE,
    revalidateIfStale: true,
  });
}

export async function tagMembersCached(orgId: string, tagId: string) {
  return queryClient.ensureQueryData({ ...membersQuery(orgId, tagId), revalidateIfStale: true });
}

// ---------------------------------------------------------------------------
// Keeping the cache right after this app changes something.
// ---------------------------------------------------------------------------

function patchTags(fn: (items: Tag[]) => Tag[]) {
  queryClient.setQueryData<Synced<Tag>>(keys.tags, (prev) => (prev ? { ...prev, items: fn(prev.items).sort(byName) } : prev));
}

/** A tag was created or edited: put the fresh copy in the list straight away. */
export function cacheTag(tag: Tag) {
  patchTags((items) => [...items.filter((t) => t.id !== tag.id), tag]);
  queryClient.setQueryData(keys.tag(tag.id), tag);
}

export function uncacheTag(tagId: string) {
  patchTags((items) => items.filter((t) => t.id !== tagId));
  queryClient.removeQueries({ queryKey: keys.tag(tagId) });
  queryClient.removeQueries({ queryKey: ["members"], predicate: (q) => q.queryKey[2] === tagId });
}

/** Membership changed: refetch the tag (for its device count) and its member list. */
export async function refreshTagMembership(tagId: string) {
  await queryClient.invalidateQueries({ queryKey: ["members"], predicate: (q) => q.queryKey[2] === tagId });
  const fresh = await getTag(tagId).catch(() => null);
  if (fresh) cacheTag(fresh);
}
