import { describe, expect, it, vi } from "vitest";
import { RampageClient } from "./index";

describe("RampageClient", () => {
  it("refuses non-loopback controller URLs", () => {
    expect(() => new RampageClient("https://control.example.com")).toThrow(/loopback/);
    expect(new RampageClient().baseUrl).toBe("http://127.0.0.1:47831");
  });

  it("encodes artifact bytes without losing binary values", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      schema: "rampage.artifact-ref.v1",
      digest: "sha256:test",
      size_bytes: 3,
      media_type: "application/octet-stream",
      storage_class: "cache",
      encrypted: true,
    }), { status: 201, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    await client.putArtifact(Uint8Array.from([0, 1, 255]));
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(JSON.parse(init.body as string).data_base64).toBe("AAH/");
    expect(new Headers(init.headers).get("x-rampage-token")).toBe("local-token");
    vi.unstubAllGlobals();
  });

  it("exposes signed topology observations through the token-protected API", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify([]), {
      status: 200,
      headers: { "content-type": "application/json" },
    }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    expect(await client.topology()).toEqual([]);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:47831/v1/offers");
    expect(new Headers(init.headers).get("x-rampage-token")).toBe("local-token");
    vi.unstubAllGlobals();
  });

  it("uses the bounded shard-set planning and status routes", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({
        schema: "rampage.shard-set-plan.v1",
        set_id: "set-1",
        admissible: true,
        all_or_nothing: true,
        placements: [],
        mutated: false,
      }), { status: 200, headers: { "content-type": "application/json" } }))
      .mockResolvedValueOnce(new Response(JSON.stringify({
        schema: "rampage.shard-set-status.v1",
        set_id: "set-1",
        status: "running",
        total: 1,
        succeeded: 0,
        failed: 0,
        terminal: 0,
        minimum_successes: 1,
        threshold_met: false,
        threshold_still_possible: true,
        members: [],
      }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    const set = {
      schema: "rampage.shard-set.v1" as const,
      set_id: "set-1",
      project_id: "project-1",
      submitted_by: "user-1",
      shards: [{}],
      minimum_successes: 1,
      deadline: "2026-08-01T00:00:00Z",
      idempotency_key: "idempotent-1",
    };
    expect((await client.planShardSet(set)).admissible).toBe(true);
    expect((await client.shardSetStatus("set-1")).status).toBe("running");
    expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:47831/v1/shard-sets/plan");
    expect(fetchMock.mock.calls[1]?.[0]).toBe("http://127.0.0.1:47831/v1/shard-sets/set-1");
    vi.unstubAllGlobals();
  });
});
