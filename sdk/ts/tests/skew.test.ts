// AC5: the client notices when the host's contract is not the one it was rendered from.
import { afterEach, describe, expect, it, vi } from "vitest";

import { CONTRACT_VERSION, Client, ContractSkew } from "../src/index.js";
import { FakeHost } from "./host.js";

const HOST_SHA = "b71e" + "0".repeat(60);
const HOST_VERSION = `1.${HOST_SHA.slice(0, 12)}`;

afterEach(() => vi.restoreAllMocks());

describe("contract skew", () => {
  it("warns exactly once, naming both versions", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const host = new FakeHost({ etag: `"${HOST_SHA}"` });
    const c = new Client({ url: "http://host.test", fetch: host.fetch });
    await c.ready;
    expect(warn).toHaveBeenCalledTimes(1);
    const text = String(warn.mock.calls[0]?.[0]);
    expect(text).toContain(CONTRACT_VERSION);
    expect(text).toContain(HOST_VERSION);
  });

  it("onSkew: throw rejects with ContractSkew", async () => {
    const host = new FakeHost({ etag: `"${HOST_SHA}"` });
    const c = new Client({ url: "http://host.test", onSkew: "throw", fetch: host.fetch });
    const e = await c.ready.catch((x: unknown) => x);
    expect(e).toBeInstanceOf(ContractSkew);
    expect((e as ContractSkew).local).toBe(CONTRACT_VERSION);
    expect((e as ContractSkew).host).toBe(HOST_VERSION);
  });

  it("a call on a throwing client rejects with ContractSkew before sending anything", async () => {
    const host = new FakeHost({ etag: `"${HOST_SHA}"` });
    const c = new Client({ url: "http://host.test", onSkew: "throw", fetch: host.fetch });
    await expect(c.host.quickstart({})).rejects.toBeInstanceOf(ContractSkew);
    expect(host.posts()).toEqual([]);
  });

  it("a matching host is silent and contractSkew() is null", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const host = new FakeHost();
    const c = new Client({ url: "http://host.test", onSkew: "throw", fetch: host.fetch });
    await c.ready;
    expect(await c.contractSkew()).toBeNull();
    expect(warn).not.toHaveBeenCalled();
    expect(host.requests.map((r) => r.method)).toEqual(["HEAD", "HEAD"]);
  });

  it("contractSkew() reports both versions, weak ETags included", async () => {
    const c = new Client({ url: "http://host.test", onSkew: "ignore", fetch: new FakeHost({ etag: `W/"${HOST_SHA}"` }).fetch });
    expect(await c.contractSkew()).toEqual({ local: CONTRACT_VERSION, host: HOST_VERSION });
  });

  it("ignore makes no request", async () => {
    const host = new FakeHost({ etag: `"${HOST_SHA}"` });
    const c = new Client({ url: "http://host.test", onSkew: "ignore", fetch: host.fetch });
    await c.ready;
    expect(host.requests).toEqual([]);
  });

  it("an unknown onSkew value is refused", () => {
    expect(() => new Client({ url: "http://host.test", onSkew: "shout" as never })).toThrow(TypeError);
  });
});
