import { useQueryClient } from "@tanstack/svelte-query";
import type { GitInfo } from "@bindings/GitInfo";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

/** Cached (machine, path) git lookup for the spawn dialog branch badge. */
export const useGitInfo = () => {
  const qc = useQueryClient();
  return (machineId: string, path: string): Promise<GitInfo> =>
    qc.fetchQuery({
      queryKey: qk.gitInfo(machineId, path),
      queryFn: () => endpoints.machineGitInfo(machineId, path),
      staleTime: 30_000,
      retry: false,
    });
};
