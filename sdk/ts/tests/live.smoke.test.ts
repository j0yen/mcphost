// AC8: against a live host, the SDK built from the deployed tag reads host.quickstart with no key
// and sees the contract it was rendered from. Skipped unless MCPHOST_LIVE=1 so CI never hits the network.
//   MCPHOST_LIVE=1 MCPHOST_URL=https://mcphost.dev npm --prefix sdk/ts test -- live.smoke
import { describe, expect, it } from "vitest";

import { Client } from "../src/index.js";

const live = process.env.MCPHOST_LIVE === "1";
const url = process.env.MCPHOST_URL || "https://mcphost.dev";

describe.skipIf(!live)("live smoke", () => {
  it("host.quickstart with no key returns limits.plan and contractSkew() is null", async () => {
    // key: "" is not nullish, so the client does not fall back to MCPHOST_KEY: the call is unauthenticated.
    const c = new Client({ url, key: "", onSkew: "ignore" });
    const res = await c.host.quickstart({});
    const plan = res.limits?.plan;
    expect(plan, "limits.plan").toBeTruthy();
    expect(Object.keys(plan as Record<string, unknown>).length, "limits.plan keys").toBeGreaterThan(0);
    expect(await c.contractSkew()).toBeNull();
  }, 30_000);
});
