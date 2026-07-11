import type { OctokitRequest } from "./client.ts";
import { parseRateHeaders, type RateHeaders } from "./ratelimit.ts";

export interface MarkReadResult {
  status: number;
  ok: boolean;
  rate: RateHeaders;
}

interface OctokitLikeError {
  status?: number;
  response?: { headers?: Record<string, string | undefined> };
}

export async function markThreadRead(
  client: OctokitRequest,
  threadId: string,
): Promise<MarkReadResult> {
  try {
    const res = await client.request("PATCH /notifications/threads/{thread_id}", {
      thread_id: threadId,
    });
    return {
      status: res.status,
      ok: (res.status >= 200 && res.status < 300) || res.status === 404,
      rate: parseRateHeaders(res.headers),
    };
  } catch (err) {
    const e = err as OctokitLikeError;
    const status = e.status ?? 0;
    const headers = e.response?.headers ?? {};
    return { status, ok: status === 404, rate: parseRateHeaders(headers) };
  }
}
