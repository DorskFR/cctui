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
  forceSync?: (account: string) => Promise<"ok" | "busy" | "unknown">;
  accountFor?: (account: string) => Account | undefined;
  auth?: AuthResolver;
  authDisabled?: boolean;
  sealer?: { seal: (plaintext: string) => string };
  validatePat?: PatValidator;
  octokitForPat?: (token: string) => OctokitRequest;
  limits?: AccountLimits;
}

export interface AccountLimits {
  rateLimitPerHour: number;
  pollIntervalMs: number;
}
