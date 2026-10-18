// A recording in-memory host speaking just enough MCP over a fake `fetch`.
import { CONTRACT_SHA } from "../src/index.js";

type Reply = Record<string, unknown>;
export type Handler = (tool: string, args: Record<string, unknown>) => Reply;

export const ok = (structured: Record<string, unknown>): Reply => ({
  result: { structuredContent: structured, isError: false },
});

export const rpcError = (data: Record<string, unknown>, message = "refused", code = -32602): Reply => ({
  error: { code, message, data },
});

export class FakeHost {
  readonly requests: { method: string; url: string; body?: any }[] = [];
  etag: string | null;
  onCall?: Handler;

  constructor(opts: { etag?: string | null; onCall?: Handler } = {}) {
    this.etag = opts.etag === undefined ? `"${CONTRACT_SHA}"` : opts.etag;
    this.onCall = opts.onCall;
  }

  readonly fetch = async (url: string, init?: RequestInit): Promise<Response> => {
    const method = init?.method ?? "GET";
    if (method === "HEAD") {
      this.requests.push({ method, url });
      return new Response(null, { status: 200, headers: this.etag === null ? {} : { ETag: this.etag } });
    }
    const body = JSON.parse(String(init?.body));
    this.requests.push({ method, url, body });
    if (body.method === "initialize") {
      return json({ jsonrpc: "2.0", id: body.id, result: { capabilities: {} } });
    }
    if (this.onCall === undefined) throw new Error("test host got a tools/call it has no handler for");
    return json({ jsonrpc: "2.0", id: body.id, ...this.onCall(body.params.name, body.params.arguments) });
  };

  posts(): any[] {
    return this.requests.filter((r) => r.method === "POST").map((r) => r.body);
  }
}

function json(value: unknown): Response {
  return new Response(JSON.stringify(value), { headers: { "content-type": "application/json" } });
}
