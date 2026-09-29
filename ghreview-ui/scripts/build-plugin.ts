#!/usr/bin/env bun
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const distDir = join(root, "dist", "plugin");
const skillsDir = join(root, "skills");

// The server refuses an archive above this, so a bundle that outgrows it is a
// build failure here rather than an install failure on someone's instance.
const MAX_ARCHIVE_BYTES = 5 * 1024 * 1024;

interface PluginManifest {
  id: string;
  name: string;
  description: string;
  version: string;
  cctuiApi: 1;
  icon: string;
  web: string;
  page: { title: string; icon: string };
  backend: { upstreamSetting: string };
  styles?: string[];
  instanceSettings: { key: string; label: string; type: "string" | "url"; secret?: boolean }[];
  skills?: string[];
}

async function run(cmd: string[]): Promise<void> {
  const proc = Bun.spawn(cmd, { cwd: root, stdout: "inherit", stderr: "inherit" });
  const code = await proc.exited;
  if (code !== 0) throw new Error(`${cmd.join(" ")} exited ${code}`);
}

async function exists(path: string): Promise<boolean> {
  return await Bun.file(path).exists();
}

const pkg = JSON.parse(await readFile(join(root, "package.json"), "utf8")) as { version: string };
const version = process.env.GHREVIEW_VERSION ?? pkg.version;

await rm(distDir, { recursive: true, force: true });
await mkdir(distDir, { recursive: true });

await run(["bunx", "vite", "build", "--config", "vite.plugin.config.ts"]);

const bundle = join(distDir, "web", "index.js");
if (!(await exists(bundle))) throw new Error(`vite produced no bundle at ${bundle}`);

// The page is unreadable without the token/diff/markdown sheets, so a build that
// lost them is a failure rather than a silently unstyled install.
const stylesheet = join(distDir, "web", "index.css");
if (!(await exists(stylesheet))) throw new Error(`vite produced no stylesheet at ${stylesheet}`);

const hasSkill = await exists(join(skillsDir, "gh-review", "SKILL.md"));
if (hasSkill) {
  await mkdir(join(distDir, "skills"), { recursive: true });
  await run(["cp", "-R", join(skillsDir, "gh-review"), join(distDir, "skills", "gh-review")]);
}

const manifest: PluginManifest = {
  id: "ghreview",
  name: "Review",
  description:
    "GitHub review centre: pull requests, diffs, inline review drafts and the notification inbox, served by a separately deployed ghreview backend.",
  version,
  cctuiApi: 1,
  icon: "pull-request",
  web: "web",
  page: { title: "Review", icon: "pull-request" },
  backend: { upstreamSetting: "backendUrl" },
  styles: ["web/index.css"],
  instanceSettings: [
    { key: "backendUrl", label: "Backend URL", type: "url" },
    { key: "backendSecret", label: "Backend shared secret", type: "string", secret: true },
  ],
  ...(hasSkill ? { skills: ["gh-review"] } : {}),
};

await writeFile(join(distDir, "plugin.json"), `${JSON.stringify(manifest, null, 2)}\n`);

const tgz = join(root, "dist", `ghreview-${version}.tgz`);
await rm(tgz, { force: true });
await run(["tar", "czf", tgz, "-C", join(root, "dist"), "plugin"]);

const size = Bun.file(tgz).size;
console.log(`plugin.json + web/index.js -> ${tgz} (${(size / 1024).toFixed(1)} KiB)`);
if (size > MAX_ARCHIVE_BYTES) {
  throw new Error(
    `${tgz} is ${(size / 1024 / 1024).toFixed(2)} MB, over the server's 5 MB archive limit`,
  );
}
