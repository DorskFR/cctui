import type { DiffModel } from "./parse";

export interface NavTarget {
  rowIndex: number;
  fileIndex: number;
  hunkIndex: number;
  kind: "file" | "hunk";
}

export interface NavIndex {
  files: NavTarget[];
  hunks: NavTarget[];
}

export function buildNavIndex(model: DiffModel): NavIndex {
  const files: NavTarget[] = [];
  const hunks: NavTarget[] = [];
  for (const file of model.files) {
    files.push({
      rowIndex: file.fileRowIndex,
      fileIndex: model.files.indexOf(file),
      hunkIndex: -1,
      kind: "file",
    });
    file.hunks.forEach((hunk, hunkIndex) => {
      hunks.push({
        rowIndex: hunk.rowStart,
        fileIndex: model.files.indexOf(file),
        hunkIndex,
        kind: "hunk",
      });
    });
  }
  return { files, hunks };
}

function stepSorted(targets: NavTarget[], currentRow: number, dir: 1 | -1): NavTarget | null {
  if (targets.length === 0) return null;
  if (dir === 1) {
    for (const t of targets) if (t.rowIndex > currentRow) return t;
    return null;
  }
  for (let i = targets.length - 1; i >= 0; i--) {
    if (targets[i].rowIndex < currentRow) return targets[i];
  }
  return null;
}

export function nextFile(nav: NavIndex, currentRow: number): NavTarget | null {
  return stepSorted(nav.files, currentRow, 1);
}

export function prevFile(nav: NavIndex, currentRow: number): NavTarget | null {
  return stepSorted(nav.files, currentRow, -1);
}

export function nextHunk(nav: NavIndex, currentRow: number): NavTarget | null {
  return stepSorted(nav.hunks, currentRow, 1);
}

export function prevHunk(nav: NavIndex, currentRow: number): NavTarget | null {
  return stepSorted(nav.hunks, currentRow, -1);
}
