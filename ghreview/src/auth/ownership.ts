import type { Context } from "hono";
import { accountOwnedBy } from "../db/notificationState.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import { getUserId } from "./middleware.ts";

type OwnershipError = {
  ok: false;
  status: 403 | 404;
  body: { error: { code: string; message: string } };
};

export type OwnedAccountResult = { ok: true; acct: Account } | OwnershipError;

function forbidden(message: string): OwnershipError {
  return { ok: false, status: 403, body: { error: { code: "forbidden", message } } };
}

function notFound(message: string): OwnershipError {
  return { ok: false, status: 404, body: { error: { code: "not_found", message } } };
}

export async function requireOwnedAccount(
  deps: AppDeps,
  c: Context,
  account: string,
): Promise<OwnedAccountResult> {
  const uid = getUserId(c);
  if (!uid) return forbidden("Authentication required");
  if (deps.db && !(await accountOwnedBy(deps.db, account, uid))) {
    return forbidden(`Account ${account} is not accessible`);
  }
  const acct = deps.accountFor?.(account);
  if (!acct) return notFound(`Account ${account} is not managed`);
  return { ok: true, acct };
}
