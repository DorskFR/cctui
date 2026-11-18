import { qk } from "./keys";
import { useQueryClient } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { api } from "../api";
import { path } from "@bindings/routes";
import type { PrivacyScanJob } from "@bindings/PrivacyScanJob";
import type { RescrubRequest } from "@bindings/RescrubRequest";
import type { EventRetentionInfo } from "@bindings/EventRetentionInfo";

/** Start a scan and hand back its job. A real (non-dry-run) pass rewrote rows
 *  the open conversation is already rendering, so its cache has to go once the
 *  job finishes — otherwise the sweep looks like it did nothing until a manual
 *  reload. */
export function useRescrub() {
  const qc = useQueryClient();
  return {
    start: (req: RescrubRequest) => endpoints.startRescrub(req),
    poll: () => endpoints.privacyScanJob(),
    cancel: () => endpoints.cancelRescrub(),
    settled: (job: PrivacyScanJob) => {
      if (!job.dry_run && job.rows_changed > 0) {
        qc.invalidateQueries({ queryKey: qk.conversationAll });
      }
    },
  };
}

/** Revoke the credential this browser signed in with (`DELETE /me/key`). */
export const revokeCurrentKey = () => api.del<void>(path("delete_me_key"));

export const eventRetention = () =>
  api.get<EventRetentionInfo>(path("get_admin_instance_event_retention"));

/** `null` restores the default; `0` keeps events forever. */
export const setEventRetention = (days: number | null) =>
  api.put<EventRetentionInfo>(path("put_admin_instance_event_retention"), { days });
