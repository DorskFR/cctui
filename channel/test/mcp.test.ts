import { describe, expect, test, mock } from "bun:test";
import { createChannelServer } from "../src/mcp";

describe("createChannelServer", () => {
  test("creates server with channel capability", () => {
    const { server } = createChannelServer({ onReply: mock(async () => {}) });
    expect(server).toBeDefined();
  });

  test("pushMessage is a function", async () => {
    const { pushMessage } = createChannelServer({ onReply: mock(async () => {}) });
    expect(typeof pushMessage).toBe("function");
  });
});
