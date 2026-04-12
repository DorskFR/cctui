import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { ListToolsRequestSchema, CallToolRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

export interface ChannelServerOptions {
  onReply: (text: string) => Promise<void>;
  onPermissionRequest: (
    requestId: string,
    toolName: string,
    description: string,
    inputPreview: string,
  ) => Promise<void>;
}

const PermissionRequestNotificationSchema = z.object({
  method: z.literal("notifications/claude/channel/permission_request"),
  params: z.object({
    request_id: z.string(),
    tool_name: z.string(),
    description: z.string(),
    input_preview: z.string(),
  }),
});

export function createChannelServer(options: ChannelServerOptions) {
  const { onReply, onPermissionRequest } = options;

  const server = new Server(
    { name: "cctui", version: "0.1.0" },
    {
      capabilities: {
        experimental: {
          "claude/channel": {},
          "claude/channel/permission": {},
        },
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

  // Handle incoming permission_request notifications from Claude Code
  server.setNotificationHandler(PermissionRequestNotificationSchema, async (notification) => {
    const { request_id, tool_name, description, input_preview } = notification.params;
    console.error(
      `[cctui-channel] permission_request received: ${request_id} for tool ${tool_name}`,
    );
    await onPermissionRequest(request_id, tool_name, description, input_preview);
  });

  async function sendPermissionResponse(requestId: string, behavior: "allow" | "deny") {
    try {
      await server.notification({
        method: "notifications/claude/channel/permission",
        params: { request_id: requestId, behavior },
      });
      console.error(
        `[cctui-channel] permission response sent: ${requestId} → ${behavior}`,
      );
    } catch (err) {
      console.error("[cctui-channel] failed to send permission response:", err);
    }
  }

  async function pushMessage(content: string, meta?: Record<string, string>) {
    console.error(`[cctui-channel] pushing message to Claude: ${content.slice(0, 100)}`);
    try {
      await server.notification({
        method: "notifications/claude/channel",
        params: { content, meta: { sender: "tui", ...meta } },
      });
      console.error("[cctui-channel] notification sent successfully");
    } catch (err) {
      console.error("[cctui-channel] failed to push notification:", err);
    }
  }

  async function connect() {
    const transport = new StdioServerTransport();
    await server.connect(transport);
  }

  return { server, pushMessage, sendPermissionResponse, connect };
}
