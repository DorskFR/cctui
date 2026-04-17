import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ServerBridge } from "./bridge";

export interface SkillIndexEntry {
  name: string;
  version: string;
  sha256: string;
  size_bytes: number;
  uploaded_by_machine: string | null;
  uploaded_at: string;
  content_type: string;
}

const VERSION_FILE = ".cctui-version";

function skillsRoot(): string {
  return process.env.CCTUI_SKILLS_DIR ?? join(homedir(), ".claude", "skills");
}

function readLocalVersion(root: string, name: string): string | null {
  try {
    return readFileSync(join(root, name, VERSION_FILE), "utf8").trim();
  } catch {
    return null;
  }
}

function writeLocalVersion(root: string, name: string, sha256: string): void {
  try {
    writeFileSync(join(root, name, VERSION_FILE), `${sha256}\n`);
  } catch (err) {
    console.error(`[cctui-channel] write ${VERSION_FILE} for ${name} failed:`, err);
  }
}

/**
 * Fetch `/skills/index` and pull any skill whose server sha256 differs from
 * the local `.cctui-version`. Best-effort: logs and continues on any error.
 */
export async function syncSkills(bridge: ServerBridge): Promise<void> {
  const root = skillsRoot();
  try {
    mkdirSync(root, { recursive: true });
  } catch (err) {
    console.error(`[cctui-channel] mkdir ${root} failed:`, err);
    return;
  }

  let index: SkillIndexEntry[];
  try {
    index = await bridge.getSkillIndex();
  } catch (err) {
    console.error("[cctui-channel] skill index fetch failed:", err);
    return;
  }

  for (const entry of index) {
    const local = readLocalVersion(root, entry.name);
    if (local === entry.sha256) continue;
    try {
      await pullOne(bridge, root, entry);
      writeLocalVersion(root, entry.name, entry.sha256);
      console.error(
        `[cctui-channel] skill synced: ${entry.name} @ ${entry.sha256.slice(0, 12)}`,
      );
    } catch (err) {
      console.error(`[cctui-channel] skill pull failed for ${entry.name}:`, err);
    }
  }
}

async function pullOne(
  bridge: ServerBridge,
  root: string,
  entry: SkillIndexEntry,
): Promise<void> {
  const bytes = await bridge.getSkillBundle(entry.name);
  const tmp = join(
    root,
    `.cctui-skill-${entry.name}-${process.pid}-${Date.now()}.tar.zst`,
  );
  writeFileSync(tmp, bytes);

  // Replace destination atomically: move existing aside, extract, then drop.
  const dest = join(root, entry.name);
  const backup = existsSync(dest)
    ? `${dest}.cctui-old-${process.pid}-${Date.now()}`
    : null;
  try {
    if (backup) {
      await Bun.$`mv ${dest} ${backup}`.quiet();
    }
    const res = await Bun.spawn(["tar", "--zstd", "-C", root, "-xf", tmp], {
      stdout: "pipe",
      stderr: "pipe",
    }).exited;
    if (res !== 0) {
      throw new Error(`tar --zstd -x exited ${res}`);
    }
    if (backup) {
      await Bun.$`rm -rf ${backup}`.quiet();
    }
  } catch (err) {
    // Roll back if possible.
    if (backup && existsSync(backup)) {
      try {
        if (existsSync(dest)) await Bun.$`rm -rf ${dest}`.quiet();
        await Bun.$`mv ${backup} ${dest}`.quiet();
      } catch {}
    }
    throw err;
  } finally {
    try {
      await Bun.$`rm -f ${tmp}`.quiet();
    } catch {}
  }
}
