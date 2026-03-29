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

  test("handles empty content array", () => {
    const line = JSON.stringify({ type: "assistant", message: { role: "assistant", content: [] } });
    expect(parseLine(line)).toEqual([]);
  });

  test("handles assistant string content", () => {
    const line = JSON.stringify({ type: "assistant", message: { role: "assistant", content: "direct string" } });
    expect(parseLine(line)).toEqual([{ type: "assistant_message", content: "direct string" }]);
  });

  test("handles empty string content", () => {
    const line = JSON.stringify({ type: "human", message: { role: "user", content: "" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("handles missing message field", () => {
    const line = JSON.stringify({ type: "human" });
    expect(parseLine(line)).toEqual([]);
  });

  test("handles missing role field", () => {
    const line = JSON.stringify({ type: "human", message: { content: "no role" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips queue-operation type", () => {
    const line = JSON.stringify({ type: "queue-operation", message: { role: "user", content: "queued" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("handles unknown content part types gracefully", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: { role: "assistant", content: [
        { type: "thinking", text: "internal thought" },
        { type: "text", text: "visible response" },
      ]},
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0]).toEqual({ type: "assistant_message", content: "visible response" });
  });

  test("handles interleaved tool_results in user message", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: [
        { type: "tool_result", tool_use_id: "t1", content: "output 1" },
        { type: "text", text: "follow up question" },
        { type: "tool_result", tool_use_id: "t2", content: "output 2" },
      ]},
    });
    const results = parseLine(line);
    expect(results).toHaveLength(3);
    expect(results[0].type).toBe("tool_result");
    expect(results[1].type).toBe("user_message");
    expect(results[2].type).toBe("tool_result");
  });

  test("handles tool_use with empty input", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "tool_use", name: "Read", input: {} }] },
    });
    expect(parseLine(line)).toEqual([{ type: "tool_call", tool: "Read", input: {} }]);
  });

  test("handles tool_use with missing name", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "tool_use", input: { x: 1 } }] },
    });
    expect(parseLine(line)).toEqual([{ type: "tool_call", tool: "", input: { x: 1 } }]);
  });

  test("handles tool_result with null content", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: null }] },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content).toBe("");
  });

  test("handles tool_result with array of content blocks (MCP style)", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: [
        { type: "text", text: "first block" },
        { type: "text", text: "second block" },
      ] }] },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content).toBe("first block\nsecond block");
  });

  test("handles tool_result with object content (not string)", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: { key: "value" } }] },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content).toBe('{"key":"value"}');
  });

  test("skips command type", () => {
    const line = JSON.stringify({ type: "command", message: { role: "user", content: "command output" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips progress type", () => {
    const line = JSON.stringify({ type: "progress", message: { role: "user", content: "progress update" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips metadata type", () => {
    const line = JSON.stringify({ type: "metadata", message: { role: "user", content: "metadata" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips config_change type", () => {
    const line = JSON.stringify({ type: "config_change", message: { role: "user", content: "config change" } });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips user message with command artifacts", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: "some text </command-name> more text" },
    });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips user message with command tag artifacts", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: "<command-model>claude-opus</command-model>" },
    });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips tool_result with command artifacts in content", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "t1", content: "output </command-name>" }],
      },
    });
    expect(parseLine(line)).toEqual([]);
  });

  test("skips text part with command artifacts in content array", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          { type: "text", text: "normal text" },
          { type: "text", text: "</command-name>" },
          { type: "text", text: "more text" },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(2);
    expect(results[0].content).toBe("normal text");
    expect(results[1].content).toBe("more text");
  });

  test("extracts title from nested WebSearch-like results", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "search1",
            content: [
              { title: "First Result", link: "https://example.com/1" },
              { title: "Second Result", link: "https://example.com/2" },
            ],
          },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content).toContain("First Result");
    expect(results[0].content).toContain("https://example.com/1");
  });

  test("handles tool_result with object containing title field", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "search1",
            content: { title: "Search Result Title", description: "Some description" },
          },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content).toBe("Search Result Title");
  });

  test("handles tool_result with object containing text field", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "id1",
            content: { text: "Extracted text content", meta: "ignored" },
          },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content).toBe("Extracted text content");
  });

  test("handles tool_result with nested content blocks", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "t1",
            content: [
              { type: "text", text: "First block" },
              {
                type: "nested",
                content: [{ type: "text", text: "Nested text" }],
              },
              { type: "text", text: "Last block" },
            ],
          },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    expect(results[0].content).toContain("First block");
    expect(results[0].content).toContain("Last block");
  });

  test("handles array of search results with title and link", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "ws1",
            content: [
              { title: "Python Docs", link: "https://python.org", snippet: "..." },
              { title: "Stack Overflow", link: "https://stackoverflow.com", snippet: "..." },
            ],
          },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    const content = results[0].content;
    expect(content).toContain("Python Docs");
    expect(content).toContain("python.org");
    expect(content).toContain("Stack Overflow");
    expect(content).toContain("stackoverflow.com");
  });

  test("handles tool_result with deeply nested object structure", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "api1",
            content: {
              status: "success",
              data: {
                message: "Operation completed",
                details: { count: 42 },
              },
            },
          },
        ],
      },
    });
    const results = parseLine(line);
    expect(results).toHaveLength(1);
    // Should extract or stringify the object sensibly
    expect(results[0].content).toBeTruthy();
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
