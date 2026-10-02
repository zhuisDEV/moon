// Reduced official-OpenAI route fixture from the installed OpenClaw 2026.9.7
// policy, inspected 2026-10-03. It has no credentials, provider calls or config
// reads. This models only the regression boundary, not the complete host:
//
// src/agents/embedded-agent-runner/run/runtime-resolution.ts:
//   any nonempty streamParams -> requestTransportOverrides = "present"
// extensions/openai/provider-policy-api.ts, codexCanReproduceRoute:
//   requestTransportOverrides === "present" -> false
// src/agents/provider-model-route-auth.ts, selectProviderModelRouteAuth:
//   no host credential + incompatible native owner -> configured-auth rejection
//
// The same two cases were also checked against the actual installed policy
// functions, with an empty synthetic source plan and env, before this fix.
export function officialOpenAiPolicy(
  streamParams: Record<string, unknown> | undefined,
) {
  const requestTransportOverrides = streamParams &&
      Object.keys(streamParams).length > 0
    ? "present"
    : "none";
  const nativeCompatible = requestTransportOverrides !== "present";
  return {
    runtime: nativeCompatible ? "codex" : "openclaw",
    auth: nativeCompatible ? "deferred" : "rejected",
  };
}

export function nativeOnlyOpenAiRunner(params: Record<string, unknown>) {
  const policy = officialOpenAiPolicy(
    params.streamParams as Record<string, unknown> | undefined,
  );
  if (policy.auth === "rejected") {
    throw new Error(
      "No route-compatible authentication source is configured for openai.",
    );
  }
  return { payloads: [{ text: "READY" }] };
}
