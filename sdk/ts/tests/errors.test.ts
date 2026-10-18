// AC4: host error codes become typed errors.
import { describe, expect, it } from "vitest";

import { Client, MCPHostError, errors } from "../src/index.js";
import { FakeHost, rpcError } from "./host.js";

function clientFor(data: Record<string, unknown>): Client {
  const host = new FakeHost({ onCall: () => rpcError(data, "spec is wrong") });
  return new Client({ url: "http://host.test", onSkew: "ignore", fetch: host.fetch });
}

const publish = { name: "x", kind: "echo", spec: { schema: {} } } as const;

describe("error mapping", () => {
  it("args_invalid is a typed error with data.argument", async () => {
    const c = clientFor({ error_code: "args_invalid", argument: "spec", remedy: "pass an object" });
    const e = await c.host.tool.publish(publish).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(errors.ArgsInvalid);
    expect(e).toBeInstanceOf(MCPHostError);
    const err = e as errors.ArgsInvalid;
    expect(err.code).toBe("args_invalid");
    expect(err.data.argument).toBe("spec");
    expect(err.remedy).toBe("pass an object");
    expect(err.detail).toBe("spec is wrong");
  });

  it("an unknown code throws the base class with the code preserved", async () => {
    const c = clientFor({ error_code: "brand_new_code_from_a_newer_host" });
    const e = await c.host.tool.publish(publish).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(MCPHostError);
    expect((e as object).constructor).toBe(MCPHostError);
    expect((e as MCPHostError).code).toBe("brand_new_code_from_a_newer_host");
  });

  it("every registered code maps to its own subclass", () => {
    const entries = Object.entries(errors.ERRORS_BY_CODE);
    expect(entries.length).toBeGreaterThanOrEqual(30);
    for (const [code, Cls] of entries) {
      const e = new Cls("m");
      expect(e).toBeInstanceOf(MCPHostError);
      expect(e.code).toBe(code);
    }
  });

  it("a tool-level isError body maps the same way", async () => {
    const host = new FakeHost({
      onCall: () => ({ result: { isError: true, structuredContent: { error: "no such tool", error_code: "args_invalid" } } }),
    });
    const c = new Client({ url: "http://host.test", onSkew: "ignore", fetch: host.fetch });
    const e = await c.host.tool.publish(publish).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(errors.ArgsInvalid);
  });
});
