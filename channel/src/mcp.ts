import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { ListToolsRequestSchema, CallToolRequestSchema } from "@modelcontextprotocol/sdk/types.js";

export interface ChannelServerOptions {
  onReply: (text: string) => Promise<void>;
}

export function createChannelServer(options: ChannelServerOptions) {
  const { onReply } = options;

  const server = new Server(
    { name: "cctui", version: "0.1.0" },
    {
      capabilities: {
        experimental: { "claude/channel": {} },
        tools: {},
      },
      instructions: [
        'Messages from the TUI operator arrive as <channel source="cctui" sender="tui">.',
        "These are instructions or questions from the human monitoring your session.",
        "Read them carefully and act on them. Reply using the cctui_reply tool to send a response back to the TUI.",
        "Always acknowledge TUI messages, even if briefly.",
      ].join(" "),
    },
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [
      {
        name: "cctui_reply",
        description: "Send a message back to the TUI operator who is monitoring this session",
        inputSchema: {
          type: "object" as const,
          properties: {
            text: { type: "string", description: "The message to send to the TUI operator" },
          },
          required: ["text"],
        },
      },
    ],
  }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    if (req.params.name === "cctui_reply") {
      const { text } = req.params.arguments as { text: string };
      await onReply(text);
      return { content: [{ type: "text" as const, text: "Message sent to TUI." }] };
    }
    throw new Error(`unknown tool: ${req.params.name}`);
  });

  async function pushMessage(content: string, meta?: Record<string, string>) {
    try {
      await server.notification({
        method: "notifications/claude/channel",
        params: { content, meta: { sender: "tui", ...meta } },
      });
    } catch (err) {
      console.error("[cctui-channel] failed to push notification:", err);
    }
  }

  async function connect() {
    const transport = new StdioServerTransport();
    await server.connect(transport);
  }

  return { server, pushMessage, connect };
}
