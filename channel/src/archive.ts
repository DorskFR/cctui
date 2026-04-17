import { createHash } from "node:crypto";
import { createReadStream, existsSync, readdirSync, statSync } from "node:fs";
import { basename, join } from "node:path";
import type { ServerBridge } from "./bridge";

export interface ProjectFile {
  absPath: string;
  projectDir: string;
  sessionId: string;
}

export type UploadOutcome = "skipped" | "uploaded" | "failed";

/** Walks <root>/<projectDir>/*.jsonl. Never throws. */
export function walkProjectDirs(root: string): ProjectFile[] {
  if (!existsSync(root)) return [];
  const out: ProjectFile[] = [];
  let projects: string[] = [];
  try {
    projects = readdirSync(root);
  } catch {
    return [];
  }
  for (const projectDir of projects) {
    const projPath = join(root, projectDir);
    try {
      if (!statSync(projPath).isDirectory()) continue;
    } catch {
      continue;
    }
    let entries: string[] = [];
    try {
      entries = readdirSync(projPath);
    } catch {
      continue;
    }
    for (const name of entries) {
      if (!name.endsWith(".jsonl")) continue;
      const absPath = join(projPath, name);
      try {
        if (!statSync(absPath).isFile()) continue;
      } catch {
        continue;
      }
      out.push({ absPath, projectDir, sessionId: basename(name, ".jsonl") });
    }
  }
  return out;
}

export function computeFileSha256(absPath: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const h = createHash("sha256");
    const s = createReadStream(absPath);
    s.on("error", reject);
    s.on("data", (c) => h.update(c));
    s.on("end", () => resolve(h.digest("hex")));
  });
}

const uploadedHash = new Map<string, string>();

export async function uploadIfChanged(
  bridge: ServerBridge,
  file: ProjectFile,
): Promise<UploadOutcome> {
  let sha: string;
  try {
    sha = await computeFileSha256(file.absPath);
  } catch (err) {
    console.error(`[cctui-channel] hash failed ${file.absPath}:`, err);
    return "failed";
  }
  if (uploadedHash.get(file.absPath) === sha) return "skipped";

  let state: "present" | "absent";
  try {
    state = await bridge.headArchive(file.projectDir, file.sessionId, sha);
  } catch (err) {
    console.error(`[cctui-channel] HEAD archive failed:`, err);
    return "failed";
  }
  if (state === "present") {
    uploadedHash.set(file.absPath, sha);
    return "skipped";
  }

  try {
    await bridge.putArchive(file.projectDir, file.sessionId, file.absPath, sha);
    uploadedHash.set(file.absPath, sha);
    return "uploaded";
  } catch (err) {
    console.error(`[cctui-channel] PUT archive failed for ${file.sessionId}:`, err);
    return "failed";
  }
}

export function __resetArchiveCacheForTests(): void {
  uploadedHash.clear();
}
