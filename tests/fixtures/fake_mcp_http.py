#!/usr/bin/env python3
"""最小 fake MCP server（HTTP streamable）：POST / 收 JSON-RPC，响应可为 JSON 或 SSE。
用 --sse 参数切换 SSE 响应，验证 HttpTransport 的 SSE 解析。
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

SSE = "--sse" in sys.argv

TOOLS = [
    {"name": "ping", "description": "返回 pong（fake）", "inputSchema": {"type": "object"}},
    {"name": "multiply", "description": "两数相乘（fake）", "inputSchema": {"type": "object"}},
]

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        msg = json.loads(self.rfile.read(n))
        method = msg.get("method")
        id_ = msg.get("id")
        if method == "notifications/initialized":
            self.send_response(204); self.end_headers(); return
        if method == "initialize":
            result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                      "serverInfo": {"name": "fake-http", "version": "0.1"}}
        elif method == "tools/list":
            result = {"tools": TOOLS}
        elif method == "tools/call":
            params = msg.get("params", {}); name = params.get("name"); args = params.get("arguments", {})
            if name == "ping":
                text = {"pong": True}
            elif name == "multiply":
                text = {"product": args.get("a", 1) * args.get("b", 1)}
            else:
                text = {"error": f"unknown tool {name}"}
            result = {"content": [{"type": "text", "text": json.dumps(text)}], "isError": False}
        else:
            result = {"error": {"code": -32601, "message": "method not found"}}
        body = json.dumps({"jsonrpc": "2.0", "id": id_, "result": result})
        if SSE:
            payload = ("event: message\ndata: " + body + "\n\n").encode()
            ctype = "text/event-stream"
        else:
            payload = body.encode()
            ctype = "application/json"
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
    def log_message(self, *a):
        pass

port = int(sys.argv[sys.argv.index("--port") + 1]) if "--port" in sys.argv else 8891
HTTPServer(("127.0.0.1", port), H).serve_forever()
