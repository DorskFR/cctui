import type { StreamerEvent } from "./types";

interface ParsedEvent {
  type: string;
  content?: string;
  tool?: string;
  input?: Record<string, unknown>;
  tool_use_id?: string;
}

const SKIP_TYPES = new Set(["file-history-snapshot", "queue-operation", "system"]);

/**
 * Parse a single JSONL transcript line into one or more events.
 * A single line can contain multiple content blocks (e.g. text + tool_use + text),
 * so we return ALL of them.
 */
export function parseLine(line: string): ParsedEvent[] {
  let d: Record<string, unknown>;
  try {
    d = JSON.parse(line);
  } catch {
    return [];
  }

  const msgType = (d.type as string) ?? "";
  if (SKIP_TYPES.has(msgType)) return [];

  const msg = (d.message as Record<string, unknown>) ?? {};
  const role = (msg.role as string) ?? "";
  const content = msg.content;

  if (role === "system") return [];

  if (role === "user") {
    if (typeof content === "string" && content) {
      return [{ type: "user_message", content }];
    }
    if (Array.isArray(content)) {
      const events: ParsedEvent[] = [];
      for (const part of content) {
        if (part.type === "tool_result") {
          const raw = String(part.content ?? "");
          events.push({ type: "tool_result", tool_use_id: part.tool_use_id ?? "", content: raw.slice(0, 500) });
        } else if (part.type === "text") {
          events.push({ type: "user_message", content: part.text ?? "" });
        }
      }
      return events;
    }
    return [];
  }

  if (role === "assistant") {
    if (Array.isArray(content)) {
      const events: ParsedEvent[] = [];
      for (const part of content) {
        if (part.type === "text") {
          events.push({ type: "assistant_message", content: part.text ?? "" });
        } else if (part.type === "tool_use") {
          events.push({ type: "tool_call", tool: part.name ?? "", input: part.input ?? {} });
        }
      }
      return events;
    } else if (typeof content === "string" && content) {
      return [{ type: "assistant_message", content }];
    }
    return [];
  }

  return [];
}

interface UsageData {
  tokens_in: number;
  tokens_out: number;
  cost_usd: number;
}

export function parseUsage(line: string): UsageData | null {
  let d: Record<string, unknown>;
  try {
    d = JSON.parse(line);
  } catch {
    return null;
  }

  const msg = (d.message as Record<string, unknown>) ?? {};
  const usage = (msg.usage as Record<string, number>) ?? (d.usage as Record<string, number>);
  if (!usage) return null;

  const tokensIn = (usage.input_tokens ?? 0) + (usage.cache_creation_input_tokens ?? 0) + (usage.cache_read_input_tokens ?? 0);
  const tokensOut = usage.output_tokens ?? 0;
  const costUsd = (tokensIn / 1_000_000) * 3.0 + (tokensOut / 1_000_000) * 15.0;

  return { tokens_in: tokensIn, tokens_out: tokensOut, cost_usd: costUsd };
}

export type EventCallback = (event: StreamerEvent) => void;

export async function tailTranscript(
  sessionId: string,
  transcriptPath: string,
  onEvent: EventCallback,
  signal?: AbortSignal,
): Promise<void> {
  for (let i = 0; i < 60; i++) {
    if (signal?.aborted) return;
    const exists = await Bun.file(transcriptPath).exists();
    if (exists) break;
    if (i === 59) return;
    await Bun.sleep(500);
  }

  let offset = 0;

  const processNewContent = async () => {
    const file = Bun.file(transcriptPath);
    const size = file.size;
    if (size <= offset) return;

    const content = await file.text();
    const newContent = content.slice(offset);
    offset = content.length;

    for (const line of newContent.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;

      const ts = Math.floor(Date.now() / 1000);

      const events = parseLine(trimmed);
      for (const event of events) {
        onEvent({ session_id: sessionId, type: event.type, content: event.content, tool: event.tool, input: event.input, tool_use_id: event.tool_use_id, ts });
      }

      const usage = parseUsage(trimmed);
      if (usage) {
        onEvent({ session_id: sessionId, type: "usage", ts, tokens_in: usage.tokens_in, tokens_out: usage.tokens_out, cost_usd: usage.cost_usd });
      }
    }
  };

  await processNewContent();

  while (!signal?.aborted) {
    await Bun.sleep(300);
    await processNewContent();
  }
}
