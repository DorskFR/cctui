import { createApp } from "./app.ts";
import { loadConfig } from "./config.ts";
import { createDb } from "./db/client.ts";
import { runMigrations } from "./db/migrate.ts";
import type { AppDeps } from "./deps.ts";
import { EventBus } from "./events/bus.ts";
import { createAccount } from "./github/account.ts";
import { Poller } from "./sync/poller.ts";

const config = loadConfig();
const deps: AppDeps = { webhookSecret: config.webhookSecret };
let poller: Poller | null = null;

if (config.databaseUrl) {
  const db = createDb(config.databaseUrl, config.schema);
  const ran = await runMigrations(db);
  if (ran.length > 0) console.log(`ghreview: applied migrations ${ran.join(", ")}`);
  const bus = new EventBus();
  await bus.startListening(db);
  deps.db = db;
  deps.bus = bus;

  if (config.githubAccount) {
    const account = createAccount({
      login: config.githubAccount,
      token: config.githubToken,
      budget: { limit: config.rateLimitPerHour, ceilingFraction: config.budgetCeilingFraction },
    });
    poller = new Poller({ db, account, bus, intervalMs: config.pollIntervalMs });
    poller.start();
    deps.syncSnapshot = () => ({
      last_run: poller?.lastRun ?? null,
      accounts: [account.login],
    });
  }
} else {
  console.log("ghreview: DATABASE_URL unset — running contract-only (no sync, empty store)");
}

const app = createApp(deps);

export default {
  port: config.port,
  fetch: app.fetch,
};
