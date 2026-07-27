// Provider-id helpers shared by the accounts surfaces. Pure, no
// Svelte state.

/** Display name for an account provider id (anthropic → Claude, openai → Codex,
 *  plus the compatible-endpoint variants). */
export const providerLabel = (p: string) =>
  p === "anthropic"
    ? "Claude"
    : p === "openai"
      ? "Codex"
      : p === "anthropic-compatible"
        ? "Anthropic-compatible"
        : p === "openai-compatible"
          ? "OpenAI-compatible"
          : p;

/** Provider family (mirrors the server's generated `family` column): an
 *  account may hold at most one provider per family. */
export const providerFamily = (p: string): "anthropic" | "openai" =>
  p.startsWith("openai") ? "openai" : "anthropic";

/** The selectable provider kinds, in the order the pickers list them. */
export const PROVIDER_KINDS = [
  { value: "anthropic", label: "Claude (anthropic)" },
  { value: "openai", label: "Codex (openai)" },
  { value: "anthropic-compatible", label: "Anthropic-compatible endpoint" },
  { value: "openai-compatible", label: "OpenAI-compatible endpoint" },
] as const;

export type ProviderKind = (typeof PROVIDER_KINDS)[number]["value"];
