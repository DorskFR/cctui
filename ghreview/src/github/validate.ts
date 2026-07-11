import { createOctokit, type OctokitRequest } from "./client.ts";

export interface ValidateResult {
  ok: boolean;
  login?: string;
  status: number;
}

export type PatValidator = (token: string) => Promise<ValidateResult>;

export async function validatePat(
  token: string,
  makeClient: (token: string) => OctokitRequest = createOctokit,
): Promise<ValidateResult> {
  const client = makeClient(token);
  try {
    const res = await client.request("GET /user");
    const login = (res.data as { login?: string } | null)?.login;
    if (res.status >= 200 && res.status < 300 && typeof login === "string") {
      return { ok: true, login, status: res.status };
    }
    return { ok: false, status: res.status };
  } catch (err) {
    const status = (err as { status?: number }).status ?? 0;
    return { ok: false, status };
  }
}
