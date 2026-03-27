import { describe, expect, test } from "bun:test";
import { parseLine, parseUsage } from "../src/transcript";

describe("parseLine", () => {
  test("parses user text message", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: "hello world" },
    });
    expect(parseLine(line)).toEqual([{ type: "user_message", content: "hello world" }]);
  });

  test("parses user message with content list containing text", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: [{ type: "text", text: "some input" }] },
    });
    expect(parseLine(line)).toEqual([{ type: "user_message", content: "some input" }]);
  });

  test("parses user message with tool_result", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "abc123", content: "result text" }] },
    });
    expect(parseLine(line)).toEqual([{ type: "tool_result", tool_use_id: "abc123", content: "result text" }]);
  });

  test("parses assistant text message", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "text", text: "I will help" }] },
    });
    expect(parseLine(line)).toEqual([{ type: "assistant_message", content: "I will help" }]);
  });

  test("parses assistant tool_use", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "tool_use", name: "Bash", input: { command: "ls" } }] },
    });
    expect(parseLine(line)).toEqual([{ type: "tool_call", tool: "Bash", input: { command: "ls" } }]);
  });

  test("returns empty array for system messages", () => {
    const line = JSON.stringify({ type: "system", message: { role: "system", content: "system prompt" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("returns empty array for file-history-snapshot", () => {
    const line = JSON.stringify({ type: "file-history-snapshot" });
    expect(parseLine(line)).toEqual([]);
  });

  test("returns empty array for invalid JSON", () => {
    expect(parseLine("not json")).toEqual([]);
  });

  test("truncates long tool_result content to 500 chars", () => {
    const longContent = "x".repeat(600);
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "id1", content: longContent }] },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content?.length).toBe(500);
  });

  test("parses ALL content blocks from assistant message", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: {
        role: "assistant",
        content: [
          { type: "text", text: "Let me check that." },
          { type: "tool_use", name: "Bash", input: { command: "ls" } },
          { type: "text", text: "Now reading the file." },
          { type: "tool_use", name: "Read", input: { file_path: "/tmp/foo" } },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(4);
    expect(results[0]).toEqual({ type: "assistant_message", content: "Let me check that." });
    expect(results[1]).toEqual({ type: "tool_call", tool: "Bash", input: { command: "ls" } });
    expect(results[2]).toEqual({ type: "assistant_message", content: "Now reading the file." });
    expect(results[3]).toEqual({ type: "tool_call", tool: "Read", input: { file_path: "/tmp/foo" } });
  });

  test("parses ALL content blocks from user message", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "t1", content: "result 1" },
          { type: "tool_result", tool_use_id: "t2", content: "result 2" },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(2);
    expect(results[0]).toEqual({ type: "tool_result", tool_use_id: "t1", content: "result 1" });
    expect(results[1]).toEqual({ type: "tool_result", tool_use_id: "t2", content: "result 2" });
  });
});

describe("parseUsage", () => {
  test("parses usage from message.usage", () => {
    const line = JSON.stringify({ message: { role: "assistant", usage: { input_tokens: 1000, output_tokens: 500 } } });
    const result = parseUsage(line);
    expect(result).not.toBeNull();
    expect(result!.tokens_in).toBe(1000);
    expect(result!.tokens_out).toBe(500);
    expect(result!.cost_usd).toBeCloseTo(0.0105);
  });

  test("includes cache tokens in input count", () => {
    const line = JSON.stringify({
      message: { role: "assistant", usage: { input_tokens: 100, cache_creation_input_tokens: 200, cache_read_input_tokens: 300, output_tokens: 50 } },
    });
    const result = parseUsage(line);
    expect(result!.tokens_in).toBe(600);
    expect(result!.tokens_out).toBe(50);
  });

  test("returns null when no usage data", () => {
    const line = JSON.stringify({ message: { role: "user", content: "hi" } });
    expect(parseUsage(line)).toBeNull();
  });
});
