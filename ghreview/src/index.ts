import { createApp } from "./app.ts";
import { createCctuiResolver, createStaticResolver, parseStaticTokens } from "./auth/resolver.ts";
import { loadConfig } from "./config.ts";
import { createSealer } from "./crypto/seal.ts";
import { createGhAccount } from "./db/accounts.ts";
import { createDb } from "./db/client.ts";
import { runMigrations } from "./db/migrate.ts";
import type { AppDeps } from "./deps.ts";
import { EventBus } from "./events/bus.ts";
import { AccountManager } from "./sync/manager.ts";

const config = loadConfig();
const deps: AppDeps = {
  webhookSecret: config.webhookSecret,
  limits: { rateLimitPerHour: config.rateLimitPerHour, pollIntervalMs: config.pollIntervalMs },
};
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
        syncViewedFromGithub: config.syncViewedFromGithub,
      },
      open: (sealed) => sealer.open(sealed),
    });
    await manager.start();
    deps.accountFor = (login) => manager?.accountFor(login);
    deps.syncSnapshot = () => manager?.snapshot() ?? { last_run: null, accounts: [] };
    deps.forceSync = (login) => manager?.forceSync(login) ?? Promise.resolve("unknown");
  } else {
    console.log("ghreview: GHREVIEW_SEAL_KEY unset — accounts/poller disabled (store + auth only)");
  }
} else {
  console.log("ghreview: DATABASE_URL unset — running store-less (no sync, empty store)");
}

let authLabel: string;
let hostname: string | undefined;
if (config.authMode === "none") {
  if (!config.unsafeAllowAnonymous) {
    console.error(
      "ghreview: GHREVIEW_AUTH_MODE=none refuses to boot without GHREVIEW_UNSAFE_ALLOW_ANONYMOUS=true; " +
        "use GHREVIEW_AUTH_MODE=static for local development instead",
    );
    process.exit(1);
  }
  deps.authDisabled = true;
  hostname = "127.0.0.1";
  authLabel = "none — ANONYMOUS ACCESS, LOOPBACK ONLY";
  console.warn(
    "ghreview: GHREVIEW_AUTH_MODE=none — authentication disabled for every /v1 route; binding to 127.0.0.1 only",
  );
} else if (config.authMode === "static") {
  deps.auth = createStaticResolver(parseStaticTokens(config.authTokens));
  authLabel = "static";
} else if (deps.db) {
  deps.auth = createCctuiResolver(deps.db, config.cctuiSchema);
  authLabel = "cctui";
} else {
  authLabel =
    "deny-all (cctui mode needs DATABASE_URL; set GHREVIEW_AUTH_MODE=static to serve authenticated)";
}
console.log(`ghreview: auth mode ${authLabel}`);

const app = createApp(deps);

export default {
  port: config.port,
  hostname,
  fetch: app.fetch,
};
