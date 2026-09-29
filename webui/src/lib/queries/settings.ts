import { qk } from "./keys";
import { useQueryClient } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import type { PrivacyScanJob } from "@bindings/PrivacyScanJob";
import type { RescrubRequest } from "@bindings/RescrubRequest";

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
