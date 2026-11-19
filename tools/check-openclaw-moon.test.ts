import { scheduledState } from "./check-openclaw-moon.js";

Deno.test("routine learning distinguishes no eligible work, queued evidence and stopped retries", () => {
  const cases = [
    [[], false, "empty_no_receipt"],
    [[], true, "eligible_pending"],
    [[{ batch: 0, failed: 1 }], false, "empty_no_receipt"],
    [[{ batch: 0, failed: 2 }], true, "retry_pending"],
    [[{ batch: 0, failed: 3 }], false, "exhausted"],
    [[{ batch: 0, failed: 2, expired: 1 }], true, "exhausted"],
    [[{ batch: 0, failed: 2, leased: 1 }], true, "in_progress"],
    [[{ batch: 0, failed: 2, committed: 1 }], false, "committed"],
    [[{ batch: 0, committed: 1 }, { batch: 1, failed: 3 }], true, "exhausted"],
    [
      [{ batch: 0, committed: 1 }, { batch: 1, committed: 1 }],
      true,
      "daily_limit_reached",
    ],
  ] as const;
  for (const [batches, eligible, expected] of cases) {
    const actual = scheduledState(batches, 2, eligible);
    if (actual !== expected) {
      throw new Error(`expected ${expected}, got ${actual}`);
    }
  }
});
