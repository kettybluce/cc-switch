import { QueryClient } from "@tanstack/react-query";

/** Default cache window for queries that do not set their own `staleTime`. */
export const DEFAULT_QUERY_STALE_TIME_MS = 30_000;

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: true,
      // Avoid a full IPC refetch storm (providers JSON, settings, proxy status)
      // every time the desktop window regains focus. Mutations still
      // invalidate; per-query `refetchInterval` continues to poll.
      staleTime: DEFAULT_QUERY_STALE_TIME_MS,
    },
    mutations: {
      retry: false,
    },
  },
});
