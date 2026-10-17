"""A recording in-memory host: an `httpx.MockTransport` speaking just enough MCP."""

from __future__ import annotations

import json
from typing import Any, Callable, Optional

import httpx

import mcphost_sdk

# (tool name, arguments) -> a CallToolResult dict, or a JSON-RPC error member
Handler = Callable[[str, dict[str, Any]], dict[str, Any]]


def ok(structured: dict[str, Any]) -> dict[str, Any]:
    return {"result": {"structuredContent": structured, "isError": False}}


def rpc_error(data: dict[str, Any], message: str = "refused", code: int = -32602) -> dict[str, Any]:
    return {"error": {"code": code, "message": message, "data": data}}


class FakeHost:
    def __init__(self, etag: Optional[str] = None, on_call: Optional[Handler] = None) -> None:
        self.etag = etag if etag is not None else f'"{mcphost_sdk.CONTRACT_SHA}"'
        self.on_call = on_call
        self.requests: list[httpx.Request] = []

    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self._handle)

    def posts(self) -> list[dict[str, Any]]:
        return [json.loads(r.content) for r in self.requests if r.method == "POST"]

    def _handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        if request.method == "HEAD" and request.url.path == "/contract.json":
            return httpx.Response(200, headers={"ETag": self.etag})
        body = json.loads(request.content)
        if body["method"] == "initialize":
            return httpx.Response(200, json={"jsonrpc": "2.0", "id": body["id"], "result": {"capabilities": {}}})
        assert body["method"] == "tools/call", body
        assert self.on_call is not None, "test host got a tools/call it has no handler for"
        reply = self.on_call(body["params"]["name"], body["params"]["arguments"])
        return httpx.Response(200, json={"jsonrpc": "2.0", "id": body["id"], **reply})
