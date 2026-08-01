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

  it("previews model strategy without implying execution authority", async () => {
    const payload = {
      schema: "rampage.model-session-plan.v1",
      session_id: "session-1",
      strategy: "speed_boost",
      state: "qualification_required",
      distributed: true,
      required_bytes: 1,
      observed_fabric_bytes: 2,
      maximum_supported_bytes: 2,
      predicted_speedup_milli: 1300,
      placements: [],
      blockers: ["qualification required"],
      warnings: [],
      proposed_local_endpoint: null,
      execution_authority: "none_preview_only",
      reason: "preview",
    };
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(payload), {
      status: 200,
      headers: { "content-type": "application/json" },
    }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    const request = {
      schema: "rampage.model-session-request.v1" as const,
      session_id: "session-1",
      model_id: "local/model",
      estimated_weight_bytes: 1,
      kv_cache_bytes: 0,
      context_tokens: 4096,
      strategy: "speed_boost" as const,
      max_nodes: 4,
      deadline: "2026-08-02T00:00:00Z",
      idempotency_key: "session-1",
    };
    expect((await client.planModelSession(request)).execution_authority).toBe("none_preview_only");
    expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:47831/v1/model-sessions/plan");
    vi.unstubAllGlobals();
  });

  it("exposes the OpenAI-compatible gateway with bearer auth", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      object: "list",
      data: [{ id: "gemma3:4b", object: "model", created: 1, owned_by: "rampage-fabric" }],
    }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    expect(client.openAiConfig()).toEqual({
      baseURL: "http://127.0.0.1:47831/v1",
      apiKey: "local-token",
    });
    expect((await client.models()).data[0]?.id).toBe("gemma3:4b");
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:47831/v1/models");
    expect(new Headers(init.headers).get("authorization")).toBe("Bearer local-token");
    vi.unstubAllGlobals();
  });

  it("exposes operation-exact capabilities and deterministic self-diagnostics", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({
        schema: "rampage.workload-capability-inventory.v1",
        authority: "exact_adapter_operation_from_verified_signed_offer",
        candidate_authority: false,
        nodes: [],
      }), { status: 200, headers: { "content-type": "application/json" } }))
      .mockResolvedValueOnce(new Response(JSON.stringify({
        schema: "rampage.fabric-diagnostic-report.v1",
        status: "healthy",
        health_score: 100,
        evidence_digest: `sha256:${"a".repeat(64)}`,
        autonomy: { per_change_approval_required: false },
        findings: [],
      }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    expect((await client.workloadCapabilities()).candidate_authority).toBe(false);
    expect((await client.selfScan()).autonomy.per_change_approval_required).toBe(false);
    expect(fetchMock.mock.calls.map((call) => call[0])).toEqual([
      "http://127.0.0.1:47831/v1/workload-capabilities",
      "http://127.0.0.1:47831/v1/diagnostics/self-scan",
    ]);
    vi.unstubAllGlobals();
  });

  it("exports the short-lived signed owner-relay allowlist", async () => {
    const payload = {
      schema: "rampage.relay-access-manifest.v1",
      fabric_id: `sha256:${"a".repeat(64)}`,
      generation: 3,
      allowed_endpoint_ids: ["b".repeat(64)],
      issued_at: "2026-08-01T00:00:00Z",
      expires_at: "2026-08-01T00:10:00Z",
      signature: "signed",
    };
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(payload), {
      status: 200,
      headers: { "content-type": "application/json" },
    }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    expect((await client.relayAccessManifest()).generation).toBe(3);
    expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:47831/v1/mesh/relay-access");
    vi.unstubAllGlobals();
  });

  it("requests signed canary authority with the complete evidence candidate", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      schema: "rampage.promotion-canary-lease.v1",
      canary_id: "canary-1",
      max_traffic_basis_points: 500,
      signature: "signed",
    }), { status: 201, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    const now = new Date().toISOString();
    const candidate = {
      schema: "rampage.promotion-candidate.v1" as const,
      proposal_id: "proposal-1",
      project_id: "project-1",
      base_revision: "abc123",
      candidate_digest: `sha256:${"a".repeat(64)}`,
      changed_paths: ["routing/cache.toml"],
      risk: "r0_configuration" as const,
      gates: [],
      requested_at: now,
      expires_at: now,
    };
    expect((await client.authorizePromotionCanary(candidate)).signature).toBe("signed");
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:47831/v1/improvements/canary");
    expect(new Headers(init.headers).get("x-rampage-token")).toBe("local-token");
    expect(JSON.parse(init.body as string).candidate_digest).toBe(candidate.candidate_digest);
    vi.unstubAllGlobals();
  });

  it("exposes Anthropic and OpenRouter compatibility without changing authority", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      id: "msg_1",
      type: "message",
      role: "assistant",
      content: [{ type: "text", text: "hello" }],
      model: "gemma3:4b",
      stop_reason: "end_turn",
      stop_sequence: null,
      usage: { input_tokens: 1, output_tokens: 1 },
    }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    const client = new RampageClient(undefined, "local-token");
    expect(client.anthropicConfig()).toEqual({
      baseURL: "http://127.0.0.1:47831",
      apiKey: "local-token",
    });
    expect(client.openRouterConfig().baseURL).toBe("http://127.0.0.1:47831/api/v1");
    const response = await client.anthropicMessage({
      model: "gemma3:4b",
      max_tokens: 16,
      messages: [{ role: "user", content: "hello" }],
    });
    expect(response.content[0]?.text).toBe("hello");
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:47831/v1/messages");
    expect(new Headers(init.headers).get("authorization")).toBe("Bearer local-token");
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
