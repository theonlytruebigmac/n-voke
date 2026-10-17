import { QueryClient } from "@tanstack/react-query";
import { createAsyncStoragePersister } from "@tanstack/query-async-storage-persister";
import { storeGet, storeRemove, storeSet, takeLegacy } from "./secureStore";

const DAY_MS = 24 * 60 * 60 * 1000;

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 60_000,
      // Must outlive the persisted cache, or restored entries get dropped.
      gcTime: DAY_MS,
      retry: 1,
      refetchOnWindowFocus: true,
    },
  },
});

// The cache used to live in localStorage in plain text; it refills on its own, so just remove it.
takeLegacy("query-cache.v1");

/** Reference data cached (encrypted) on disk so the app starts with lists already filled. */
const PERSISTED_KEYS = new Set(["orgs", "tags", "scripts", "assets", "members", "tag"]);

export const persistOptions = {
  persister: createAsyncStoragePersister({
    storage: { getItem: storeGet, setItem: storeSet, removeItem: storeRemove },
    key: "query-cache",
    throttleTime: 3000,
  }),
  maxAge: DAY_MS,
  // Bump to discard every user's cached data after a breaking shape change.
  buster: "1",
  dehydrateOptions: {
    shouldDehydrateQuery: (q: { queryKey: readonly unknown[]; state: { status: string } }) =>
      q.state.status === "success" && PERSISTED_KEYS.has(String(q.queryKey[0])),
  },
};
