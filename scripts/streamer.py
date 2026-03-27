#!/usr/bin/env python3
"""Tail a Claude transcript JSONL file and POST events to the cctui server."""
import json
import os
import sys
import time
import urllib.request


def post_event(api_url, token, event):
    data = json.dumps(event).encode()
    req = urllib.request.Request(
        api_url,
        data=data,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {token}",
        },
    )
    try:
        urllib.request.urlopen(req, timeout=5)
    except Exception:
        pass


def parse_line(line):
    try:
        d = json.loads(line)
    except json.JSONDecodeError:
        return None

    msg = d.get("message", {})
    role = msg.get("role", "")
    content = msg.get("content", "")
    msg_type = d.get("type", "")

    if msg_type in ("file-history-snapshot", "queue-operation", "system"):
        return None
    if role == "system":
        return None

    if role == "user":
        if isinstance(content, str) and content:
            return {"type": "user_message", "content": content}
        if isinstance(content, list):
            for part in content:
                if part.get("type") == "tool_result":
                    return {
                        "type": "tool_result",
                        "tool_use_id": part.get("tool_use_id", ""),
                        "content": str(part.get("content", ""))[:500],
                    }
                if part.get("type") == "text":
                    return {"type": "user_message", "content": part.get("text", "")}
        return None

    if role == "assistant":
        if isinstance(content, list):
            for part in content:
                if part.get("type") == "text":
                    return {"type": "assistant_message", "content": part.get("text", "")}
                if part.get("type") == "tool_use":
                    return {
                        "type": "tool_call",
                        "tool": part.get("name", ""),
                        "input": part.get("input", {}),
                    }
        elif isinstance(content, str) and content:
            return {"type": "assistant_message", "content": content}
        return None

    return None


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <session_id> <transcript_path>", file=sys.stderr)
        sys.exit(1)

    session_id = sys.argv[1]
    transcript = sys.argv[2]
    server_url = os.environ.get("CCTUI_URL", "http://localhost:8700")
    token = os.environ.get("CCTUI_TOKEN", "dev-agent")
    api_url = f"{server_url}/api/v1/events/{session_id}"

    # Wait for transcript file to appear (created after first message)
    for _ in range(60):  # wait up to 30s
        if os.path.exists(transcript):
            break
        time.sleep(0.5)
    else:
        sys.exit(0)  # file never appeared, session probably ended

    with open(transcript) as f:
        # First: send existing content
        for line in f:
            event = parse_line(line.strip())
            if event:
                event["session_id"] = session_id
                event["ts"] = int(time.time())
                post_event(api_url, token, event)

        # Then: tail for new content
        while True:
            line = f.readline()
            if line:
                event = parse_line(line.strip())
                if event:
                    event["session_id"] = session_id
                    event["ts"] = int(time.time())
                    post_event(api_url, token, event)
            else:
                # Check if the Claude session is still alive
                try:
                    pid_file = os.path.expanduser("~/.cctui/session_id")
                    if os.path.exists(pid_file):
                        with open(pid_file) as pf:
                            current_sid = pf.read().strip()
                        if current_sid != session_id:
                            break  # new session started, exit
                except Exception:
                    pass
                time.sleep(0.3)


if __name__ == "__main__":
    main()
