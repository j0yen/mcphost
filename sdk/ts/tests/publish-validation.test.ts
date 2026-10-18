// AC3: a wrong `kind` is a compile error, and a wrong runtime value is a ZodError before any request.
import { describe, expect, it, vi } from "vitest";
import { ZodError } from "zod";

import { Client } from "../src/index.js";

describe("host.tool.publish argument validation", () => {
  it("rejects a bad kind at compile time (tsc --strict --noEmit checks the directive)", () => {
    const fetchSpy = vi.fn();
    const c = new Client({ url: "http://host.test", onSkew: "ignore", fetch: fetchSpy });
    // @ts-expect-error "nope" is not one of the registered kinds
    const bad = c.host.tool.publish({ name: "x", kind: "nope", spec: {} });
    bad.catch(() => undefined);
  });

  it("throws a ZodError before any fetch when the same value is cast to any", async () => {
    const fetchSpy = vi.fn();
    const c = new Client({ url: "http://host.test", onSkew: "ignore", fetch: fetchSpy });
    const args: any = { name: "x", kind: "nope", spec: {} };
    const err = await c.host.tool.publish(args).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ZodError);
    expect((err as ZodError).issues.some((i) => i.path[0] === "kind")).toBe(true);
    expect(fetchSpy).toHaveBeenCalledTimes(0);
  });

  it("also records zero calls through the global fetch", async () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    try {
      const c = new Client({ url: "http://host.test", onSkew: "ignore" });
      await expect(c.host.tool.publish({ name: "x", kind: "nope", spec: {} } as any)).rejects.toBeInstanceOf(ZodError);
      expect(fetchSpy).toHaveBeenCalledTimes(0);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("accepts a valid kind and sends it", async () => {
    const calls: { url: string; body: any }[] = [];
    const fake = async (url: string, init?: RequestInit): Promise<Response> => {
      const body = JSON.parse(String(init?.body));
      calls.push({ url, body });
      if (body.method === "initialize") {
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, result: { capabilities: {} } }));
      }
      return new Response(
        JSON.stringify({ jsonrpc: "2.0", id: body.id, result: { structuredContent: { ok: true }, isError: false } }),
      );
    };
    const c = new Client({ url: "http://host.test", onSkew: "ignore", fetch: fake });
    await c.host.tool.publish({ name: "x", kind: "echo", spec: { schema: {} } }).catch(() => undefined);
    expect(calls.map((x) => x.body.method)).toEqual(["initialize", "tools/call"]);
    expect(calls[1]?.body.params.arguments.kind).toBe("echo");
  });
});
