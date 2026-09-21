import { describe, expect, it } from "vitest";
import {
  DEFAULT_QUERY_STALE_TIME_MS,
  queryClient,
} from "@/lib/query/queryClient";

describe("queryClient defaults", () => {
  it("uses a 30s staleTime to avoid window-focus IPC storms", () => {
    expect(DEFAULT_QUERY_STALE_TIME_MS).toBe(30_000);
    expect(queryClient.getDefaultOptions().queries?.staleTime).toBe(
      DEFAULT_QUERY_STALE_TIME_MS,
    );
  });
});
