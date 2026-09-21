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

  it("still refetches on window focus after the stale window, with a single retry", () => {
    const queries = queryClient.getDefaultOptions().queries;
    expect(queries?.refetchOnWindowFocus).toBe(true);
    expect(queries?.retry).toBe(1);
  });

  it("does not retry mutations so failed writes surface immediately", () => {
    expect(queryClient.getDefaultOptions().mutations?.retry).toBe(false);
  });

  it("exposes the staleTime constant as the only default cache window", () => {
    const queries = queryClient.getDefaultOptions().queries;
    expect(queries?.staleTime).toBe(30_000);
    expect(queries?.gcTime).toBeUndefined();
  });
});
