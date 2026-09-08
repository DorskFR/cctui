import { useQueryClient } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import type { RescrubRequest } from "@bindings/RescrubRequest";

/** Re-run the detector set over stored transcripts. A real (non-dry-run) pass
 *  rewrote rows the open conversation is already rendering, so its cache has to
 *  go — otherwise the sweep looks like it did nothing until a manual reload. */
export function useRescrub() {
  const qc = useQueryClient();
  return async (req: RescrubRequest) => {
    const report = await endpoints.rescrubSettings(req);
    if (!req.dry_run && report.rows_changed > 0) {
      qc.invalidateQueries({ queryKey: ["conversation"] });
    }
    return report;
  };
}
