import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { checkConnection, createProject, deleteProject, getProject, listProjects, listServers, removeServer, saveServer, setServerEcoverse, testServer } from "./api";

// Migration data isn't in the persisted key list in lib/queryClient.ts, so none of it is written to disk by the
// query cache (projects live in the core's encrypted store).
export const projectsQuery = { queryKey: ["migrate", "projects"], queryFn: listProjects, staleTime: 5_000 } as const;
export const serversQuery = { queryKey: ["migrate", "servers"], queryFn: listServers, staleTime: 30_000 } as const;
export const projectQuery = (id: string) => ({ queryKey: ["migrate", "project", id], queryFn: () => getProject(id), staleTime: 5_000 }) as const;
/** The Connect checklist reads the servers, so it is read each time the step opens and never kept. */
export const connectQuery = (id: string) =>
  ({ queryKey: ["migrate", "connect", id], queryFn: () => checkConnection(id), staleTime: 0, gcTime: 0, retry: false, refetchOnWindowFocus: false }) as const;

export const useProjects = () => useQuery(projectsQuery);
export const useServers = () => useQuery(serversQuery);
export const useProject = (id: string) => useQuery(projectQuery(id));
export const useConnectCheck = (id: string) => useQuery(connectQuery(id));

export function useServerActions() {
  const qc = useQueryClient();
  const refresh = () => qc.invalidateQueries({ queryKey: ["migrate", "servers"] });
  return {
    save: useMutation({ mutationFn: saveServer, onSuccess: refresh }),
    ecoverse: useMutation({ mutationFn: setServerEcoverse, onSuccess: refresh }),
    remove: useMutation({ mutationFn: removeServer, onSuccess: refresh }),
    test: useMutation({ mutationFn: testServer }),
  };
}

export function useProjectActions() {
  const qc = useQueryClient();
  const refresh = () => qc.invalidateQueries({ queryKey: ["migrate", "projects"] });
  return {
    create: useMutation({ mutationFn: createProject, onSuccess: refresh }),
    remove: useMutation({ mutationFn: deleteProject, onSuccess: refresh }),
  };
}
