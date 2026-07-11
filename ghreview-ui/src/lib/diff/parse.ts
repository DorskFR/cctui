import type { GithubFile } from "../api/types";

export interface HunkHeader {
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
  section: string;
}

export type DiffRowKind = "file" | "hunk" | "context" | "add" | "del";

export interface DiffRow {
  kind: DiffRowKind;
  content: string;
  oldLine: number | null;
  newLine: number | null;
  fileIndex: number;
  hunkIndex: number;
}

export interface DiffHunk {
  header: HunkHeader;
  rowStart: number;
  rowEnd: number;
}

export interface DiffFile {
  filename: string;
  previousFilename?: string;
  status: GithubFile["status"];
  additions: number;
  deletions: number;
  binary: boolean;
  hunks: DiffHunk[];
  fileRowIndex: number;
  rowStart: number;
  rowEnd: number;
}

export interface DiffModel {
  files: DiffFile[];
  rows: DiffRow[];
}

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/;

export function parseHunkHeader(line: string): HunkHeader | null {
  const m = HUNK_RE.exec(line);
  if (!m) return null;
  return {
    oldStart: Number(m[1]),
    oldCount: m[2] === undefined ? 1 : Number(m[2]),
    newStart: Number(m[3]),
    newCount: m[4] === undefined ? 1 : Number(m[4]),
    section: (m[5] ?? "").trim(),
  };
}

export function buildDiffModel(files: GithubFile[]): DiffModel {
  const rows: DiffRow[] = [];
  const outFiles: DiffFile[] = [];

  files.forEach((file, fileIndex) => {
    const fileRowIndex = rows.length;
    rows.push({
      kind: "file",
      content: file.filename,
      oldLine: null,
      newLine: null,
      fileIndex,
      hunkIndex: -1,
    });

    const hunks: DiffHunk[] = [];
    const patch = file.patch ?? "";
    const binary = patch.length === 0;
    let oldLine = 0;
    let newLine = 0;
    let hunkIndex = -1;

    if (!binary) {
      for (const line of patch.split("\n")) {
        const header = parseHunkHeader(line);
        if (header) {
          if (hunkIndex >= 0) hunks[hunkIndex].rowEnd = rows.length;
          hunkIndex += 1;
          oldLine = header.oldStart;
          newLine = header.newStart;
          const rowStart = rows.length;
          rows.push({
            kind: "hunk",
            content: line,
            oldLine: null,
            newLine: null,
            fileIndex,
            hunkIndex,
          });
          hunks.push({ header, rowStart, rowEnd: rows.length });
          continue;
        }
        if (hunkIndex < 0) continue;
        const marker = line[0];
        const content = line.slice(1);
        if (marker === "\\") continue;
        if (marker === "+") {
          rows.push({
            kind: "add",
            content,
            oldLine: null,
            newLine,
            fileIndex,
            hunkIndex,
          });
          newLine += 1;
        } else if (marker === "-") {
          rows.push({
            kind: "del",
            content,
            oldLine,
            newLine: null,
            fileIndex,
            hunkIndex,
          });
          oldLine += 1;
        } else {
          rows.push({
            kind: "context",
            content,
            oldLine,
            newLine,
            fileIndex,
            hunkIndex,
          });
          oldLine += 1;
          newLine += 1;
        }
      }
      if (hunkIndex >= 0) hunks[hunkIndex].rowEnd = rows.length;
    }

    outFiles.push({
      filename: file.filename,
      previousFilename: file.previous_filename,
      status: file.status,
      additions: file.additions,
      deletions: file.deletions,
      binary,
      hunks,
      fileRowIndex,
      rowStart: fileRowIndex,
      rowEnd: rows.length,
    });
  });

  return { files: outFiles, rows };
}
