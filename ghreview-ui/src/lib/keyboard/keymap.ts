export type KeyAction =
  | { type: "nextFile" }
  | { type: "prevFile" }
  | { type: "nextHunk" }
  | { type: "prevHunk" }
  | { type: "gotoDiff" }
  | { type: "closeTab" }
  | { type: "selectTab"; index: number }
  | { type: "openPalette" }
  | { type: "escape" };

export interface KeyEventLike {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  target?: EventTarget | null;
}

export interface KeymapState {
  gPending: boolean;
}

export interface KeymapResult {
  action: KeyAction | null;
  state: KeymapState;
}

function isEditable(target: EventTarget | null | undefined): boolean {
  const el = target as { tagName?: string; isContentEditable?: boolean } | null;
  if (!el?.tagName) return false;
  const tag = el.tagName.toUpperCase();
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || !!el.isContentEditable;
}

export function resolveKey(e: KeyEventLike, state: KeymapState): KeymapResult {
  const mod = e.ctrlKey || e.metaKey;

  if (mod && e.key.toLowerCase() === "w") {
    return { action: { type: "closeTab" }, state: { gPending: false } };
  }
  if (mod && e.key.toLowerCase() === "k") {
    return { action: { type: "openPalette" }, state: { gPending: false } };
  }
  if (mod && /^[1-9]$/.test(e.key)) {
    return {
      action: { type: "selectTab", index: Number(e.key) - 1 },
      state: { gPending: false },
    };
  }

  if (isEditable(e.target)) {
    return { action: null, state: { gPending: false } };
  }

  if (e.key === "Escape") {
    return { action: { type: "escape" }, state: { gPending: false } };
  }

  if (state.gPending) {
    if (e.key === "d") return { action: { type: "gotoDiff" }, state: { gPending: false } };
    return { action: null, state: { gPending: false } };
  }

  if (e.key === "g") {
    return { action: null, state: { gPending: true } };
  }

  switch (e.key) {
    case "j":
      return { action: { type: "nextHunk" }, state: { gPending: false } };
    case "k":
      return { action: { type: "prevHunk" }, state: { gPending: false } };
    case "J":
      return { action: { type: "nextFile" }, state: { gPending: false } };
    case "K":
      return { action: { type: "prevFile" }, state: { gPending: false } };
    default:
      return { action: null, state: { gPending: false } };
  }
}
