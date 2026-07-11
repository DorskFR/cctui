import type { AuthResolver } from "./auth/resolver.ts";
import type { DbHandle } from "./db/client.ts";
import type { EventBus } from "./events/bus.ts";
import type { Account } from "./github/account.ts";
import type { OctokitRequest } from "./github/client.ts";
import type { PatValidator } from "./github/validate.ts";

export interface SyncSnapshot {
  last_run: string | null;
  accounts: string[];
}

export interface AppDeps {
  db?: DbHandle;
  bus?: EventBus;
  webhookSecret?: string;
  syncSnapshot?: () => SyncSnapshot;
  accountFor?: (account: string) => Account | undefined;
  auth?: AuthResolver;
  sealer?: { seal: (plaintext: string) => string };
  validatePat?: PatValidator;
  octokitForPat?: (token: string) => OctokitRequest;
}
