// Deterministic OpenClaw responses for adapter integration tests. The tests
// control semantic decisions independently of the real binary's search/ACLs.
export const hookModel = { modelProviderId: "vllm", modelId: "fixture-chat" };
export const assembleModel = {
  model: { provider: "vllm", resolved: "fixture-chat" },
};
export const requestId = "0123456789abcdef0123456789abcdef";

export function savedEnvelope(text: string | null, publicOnly = true) {
  const candidates = text
    ? [{
      document_id: 7,
      content: text,
      matched_queries: [0],
      temporal: false,
      observed_at_ms: null,
      verified_sender: null,
      previous: [],
      citations: [],
    }]
    : [];
  return {
    request_id: requestId,
    candidates,
    public_only: publicOnly,
    memory_count: candidates.length,
    reference_count: 0,
    packet_chars: Array.from(JSON.stringify(candidates)).length,
    truncated: false,
  };
}

export type JudgementInput = {
  currentText: string;
  context: string[];
  candidates?: { id: string; text: string }[];
};
export function fixtureJudgement(input: JudgementInput) {
  if (
    input.currentText ===
      "Show the original messages across all my authorised chats" &&
    !input.candidates
  ) {
    return { intent: "history", queries: [], thisConversationOnly: false };
  }
  if (input.candidates) return { ids: input.candidates.map((c) => c.id) };
  if (["hello", "Hi lilac"].includes(input.currentText)) {
    return { intent: "none", queries: [] };
  }
  if (
    [
      "Show our chat history",
      "What did we say in our chat history?",
      "Show our chat history for the synthetic apricot workflow.",
    ].includes(input.currentText)
  ) {
    return { intent: "history", queries: [] };
  }
  return {
    intent: "facts",
    queries: [{ spans: [input.currentText], includeTimeline: false }],
  };
}
export function modelRunner(
  judge: (input: JudgementInput) => unknown = fixtureJudgement,
) {
  return (params: Record<string, unknown>) => {
    const input = JSON.parse(String(params.prompt).split("\n\n").at(-1)!);
    return {
      meta: { agentMeta: { provider: params.provider, model: params.model } },
      payloads: [{ text: JSON.stringify(judge(input)) }],
    };
  };
}
