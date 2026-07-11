import { createApp } from "./app.ts";
import {
  type AuthResolver,
  createCctuiResolver,
  createStaticResolver,
  parseStaticTokens,
} from "./auth/resolver.ts";
import { loadConfig } from "./config.ts";
import { createSealer } from "./crypto/seal.ts";
import { createGhAccount } from "./db/accounts.ts";
import { createDb } from "./db/client.ts";
import { runMigrations } from "./db/migrate.ts";
import type { AppDeps } from "./deps.ts";
import { EventBus } from "./events/bus.ts";
import { AccountManager } from "./sync/manager.ts";

const config = loadConfig();
const deps: AppDeps = { webhookSecret: config.webhookSecret };
let manager: AccountManager | null = null;

const sealer = config.sealKey ? createSealer(config.sealKey) : undefined;
if (sealer) deps.sealer = sealer;

if (config.databaseUrl) {
  const db = createDb(config.databaseUrl, config.schema);
  const ran = await runMigrations(db);
  if (ran.length > 0) console.log(`ghreview: applied migrations ${ran.join(", ")}`);
  const bus = new EventBus();
  await bus.startListening(db);
  deps.db = db;
  deps.bus = bus;

  let resolver: AuthResolver;
  if (config.authMode === "static") {
    resolver = createStaticResolver(parseStaticTokens(config.authTokens));
  } else {
    resolver = createCctuiResolver(db, config.cctuiSchema);
  }
  deps.auth = resolver;

  if (config.githubAccount && config.githubToken && sealer) {
    await createGhAccount(db, {
      userId: "env",
      login: config.githubAccount,
      encryptedPat: sealer.seal(config.githubToken),
    }).catch((e) => console.warn(`ghreview: env account bootstrap skipped: ${e.message}`));
  }

  if (sealer) {
    manager = new AccountManager({
      db,
      bus,
      defaults: {
        pollIntervalMs: config.pollIntervalMs,
        budgetCeilingFraction: config.budgetCeilingFraction,
        rateLimitPerHour: config.rateLimitPerHour,
      },
      open: (sealed) => sealer.open(sealed),
    });
    await manager.start();
    deps.accountFor = (login) => manager?.accountFor(login);
    deps.syncSnapshot = () => manager?.snapshot() ?? { last_run: null, accounts: [] };
  } else {
    console.log("ghreview: GHREVIEW_SEAL_KEY unset — accounts/poller disabled (store + auth only)");
  }
} else {
  console.log("ghreview: DATABASE_URL unset — running contract-only (no sync, empty store)");
}

const app = createApp(deps);

export default {
  port: config.port,
  fetch: app.fetch,
};
