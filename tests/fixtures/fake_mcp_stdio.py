#!/usr/bin/env python3
"""最小 fake MCP server（stdio）：换行分隔 JSON-RPC 2.0，用于端到端验证协议栈。
实现 initialize / notifications/initialized / tools/list / tools/call。
故意往 stderr 写噪音，验证 StdioTransport 不把 stderr 混进 stdout。
"""
import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "回显输入（fake）",
        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
    },
    {
        "name": "add",
        "description": "两数相加（fake）",
        "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}},
    },
]

def respond(id_, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": id_, "result": result}) + "\n")
    sys.stdout.flush()

def main():
    sys.stderr.write("fake mcp stdio server ready\n")
    sys.stderr.flush()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        method = msg.get("method")
        id_ = msg.get("id")
        if method == "notifications/initialized":
            continue  # notification 无响应
        if id_ is None:
            continue
        if method == "initialize":
            respond(id_, {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-stdio", "version": "0.1"},
            })
        elif method == "tools/list":
            respond(id_, {"tools": TOOLS})
        elif method == "tools/call":
            params = msg.get("params", {})
            name = params.get("name")
            args = params.get("arguments", {})
            if name == "echo":
                text = {"text": args.get("text", "")}
            elif name == "add":
                text = {"sum": args.get("a", 0) + args.get("b", 0)}
            else:
                text = {"error": f"unknown tool {name}"}
            respond(id_, {"content": [{"type": "text", "text": json.dumps(text)}], "isError": False})
        else:
            respond(id_, {"error": {"code": -32601, "message": "method not found"}})

if __name__ == "__main__":
    main()
