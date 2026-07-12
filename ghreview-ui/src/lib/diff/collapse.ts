import type { DiffFile, DiffModel, DiffRow } from "./parse";

export interface CollapseOptions {
  viewed: ReadonlySet<string>;
  expanded?: ReadonlySet<string>;
}

export function collapseViewedFiles(model: DiffModel, opts: CollapseOptions): DiffModel {
  const expanded = opts.expanded ?? new Set<string>();
  const rows: DiffRow[] = [];
  const files: DiffFile[] = [];

  for (const file of model.files) {
    const collapse = opts.viewed.has(file.filename) && !expanded.has(file.filename);
    const fileRowIndex = rows.length;
    const original = model.rows[file.fileRowIndex];
    const bodyRows = model.rows.slice(file.fileRowIndex + 1, file.rowEnd);
    const hiddenLines = bodyRows.length;

    if (collapse) {
      rows.push({
        ...original,
        content: `${file.filename}  ·  viewed — ${hiddenLines} lines hidden`,
        collapsed: true,
        hiddenLines,
      });
      files.push({
        ...file,
        fileRowIndex,
        rowStart: fileRowIndex,
        rowEnd: rows.length,
        hunks: [],
        collapsed: true,
        hiddenLines,
      });
      continue;
    }

    rows.push({ ...original });
    const rowStart = fileRowIndex;
    const hunkBase = rows.length;
    for (const r of bodyRows) rows.push({ ...r });
    const shift = hunkBase - (file.fileRowIndex + 1);
    files.push({
      ...file,
      fileRowIndex,
      rowStart,
      rowEnd: rows.length,
      hunks: file.hunks.map((h) => ({
        ...h,
        rowStart: h.rowStart + shift,
        rowEnd: h.rowEnd + shift,
      })),
      collapsed: false,
    });
  }

  return { files, rows };
}
