import { Octokit } from "@octokit/rest";
import { parseRateHeaders, type RateHeaders } from "./ratelimit.ts";

export interface ConditionalResult<T> {
  status: number;
  etag: string | null;
  lastModified: string | null;
  pollInterval: number | null;
  retryAfter: number | null;
  secondaryLimit: boolean;
  rate: RateHeaders;
  data: T | null;
}

export interface OctokitRequest {
  request: (route: string, params?: Record<string, unknown>) => Promise<OctokitResponse>;
}

export interface OctokitResponse {
  status: number;
  headers: Record<string, string | undefined>;
  data: unknown;
}

interface OctokitLikeError {
  status?: number;
  response?: { headers?: Record<string, string | undefined>; data?: unknown };
}

function toResult<T>(res: OctokitResponse): ConditionalResult<T> {
  const headers = res.headers;
  const pollRaw = headers["x-poll-interval"];
  const retryRaw = headers["retry-after"];
  return {
    status: res.status,
    etag: headers.etag ?? null,
    lastModified: headers["last-modified"] ?? null,
    pollInterval: pollRaw ? Number(pollRaw) : null,
    retryAfter: retryRaw ? Number(retryRaw) : null,
    secondaryLimit: false,
    rate: parseRateHeaders(headers),
    data: res.data as T,
  };
}

function isSecondaryLimit(headers: Record<string, string | undefined>, status: number): boolean {
  if (status !== 403 && status !== 429) return false;
  const remaining = headers["x-ratelimit-remaining"];
  if (headers["retry-after"] !== undefined) return true;
  return remaining !== undefined && Number(remaining) > 0;
}

export async function conditionalRequest<T>(
  client: OctokitRequest,
  route: string,
  params: Record<string, unknown>,
  cache: { etag?: string | null; lastModified?: string | null } = {},
): Promise<ConditionalResult<T>> {
  const headers: Record<string, string> = {};
  if (cache.etag) headers["if-none-match"] = cache.etag;
  if (cache.lastModified) headers["if-modified-since"] = cache.lastModified;
  try {
    const res = await client.request(route, { ...params, headers });
    return toResult<T>(res);
  } catch (err) {
    const e = err as OctokitLikeError;
    const status = e.status ?? 0;
    const resHeaders = e.response?.headers ?? {};
    if (status === 304) {
      return {
        status: 304,
        etag: resHeaders.etag ?? cache.etag ?? null,
        lastModified: resHeaders["last-modified"] ?? cache.lastModified ?? null,
        pollInterval: resHeaders["x-poll-interval"] ? Number(resHeaders["x-poll-interval"]) : null,
        retryAfter: null,
        secondaryLimit: false,
        rate: parseRateHeaders(resHeaders),
        data: null,
      };
    }
    const retryRaw = resHeaders["retry-after"];
    return {
      status,
      etag: null,
      lastModified: null,
      pollInterval: null,
      retryAfter: retryRaw ? Number(retryRaw) : null,
      secondaryLimit: isSecondaryLimit(resHeaders, status),
      rate: parseRateHeaders(resHeaders),
      data: null,
    };
  }
}

export function createOctokit(token: string | undefined): OctokitRequest {
  return new Octokit({ auth: token }) as unknown as OctokitRequest;
}
