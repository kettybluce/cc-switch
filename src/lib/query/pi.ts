import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { piApi, type PiRuntimeSettings } from "@/lib/api/pi";
import { extractErrorMessage } from "@/utils/errorUtils";

export const piKeys = {
  all: ["pi"] as const,
  currentState: ["pi", "currentState"] as const,
  sessionDiscovery: ["pi", "sessionDiscovery"] as const,
  runtimeStatus: ["pi", "runtimeStatus"] as const,
  wslDistros: ["pi", "wslDistros"] as const,
  proxyPlan: ["pi", "proxyPlan"] as const,
};

export const invalidatePiProviderCaches = async (queryClient: QueryClient) => {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: piKeys.currentState }),
    queryClient.invalidateQueries({ queryKey: ["providers", "pi"] }),
  ]);
};

export const invalidatePiDirectoryCaches = async (queryClient: QueryClient) => {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: piKeys.all }),
    queryClient.invalidateQueries({ queryKey: ["providers", "pi"] }),
    queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
    queryClient.invalidateQueries({ queryKey: ["sessions"] }),
  ]);
};

export function usePiCurrentState(enabled = true) {
  return useQuery({
    queryKey: piKeys.currentState,
    queryFn: () => piApi.getCurrentState(),
    enabled,
  });
}

export function usePiRuntimeStatus() {
  return useQuery({
    queryKey: piKeys.runtimeStatus,
    queryFn: () => piApi.getRuntimeStatus(),
    staleTime: 30 * 1000,
    refetchOnWindowFocus: false,
  });
}

/**
 * Probe WSL distributions on demand.
 *
 * Each probe starts a login shell inside a distribution, so this is never
 * fetched automatically — only when the user opens the picker.
 */
export function usePiWslDistros(enabled: boolean) {
  return useQuery({
    queryKey: piKeys.wslDistros,
    queryFn: () => piApi.listWslDistros(),
    enabled,
    staleTime: 60 * 1000,
  });
}

export function usePiProxyPlan(enabled: boolean) {
  return useQuery({
    queryKey: piKeys.proxyPlan,
    queryFn: () => piApi.getProxyPlan(),
    enabled,
    // Snapshot only (no WSL curl). Live probe is 「检测代理」.
    staleTime: 60 * 1000,
    refetchOnWindowFocus: false,
  });
}

export function useSetPiRuntime() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: (runtime: PiRuntimeSettings) => piApi.setRuntime(runtime),
    onSuccess: async () => {
      toast.success(t("settings.piRuntime.saved"));
      // Providers, skills and sessions are all resolved through the runtime,
      // so every Pi-derived view has to be re-read.
      await invalidatePiDirectoryCaches(queryClient);
      await queryClient.invalidateQueries({ queryKey: piKeys.proxyPlan });
    },
    onError: (error: unknown) => {
      toast.error(
        t("settings.piRuntime.switchFailed", {
          error: extractErrorMessage(error),
        }),
      );
    },
  });
}

export function useSyncPiWslSessions() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: () => piApi.syncWslSessions(),
    onSuccess: async (outcome) => {
      if (outcome.errors.length > 0) {
        toast.error(
          t("settings.piRuntime.syncPartial", { error: outcome.errors[0] }),
        );
      } else {
        toast.success(
          t("settings.piRuntime.synced", {
            fetched: outcome.fetched,
            total: outcome.total,
          }),
        );
      }
      await queryClient.invalidateQueries({ queryKey: ["sessions"] });
      await queryClient.invalidateQueries({
        queryKey: piKeys.sessionDiscovery,
      });
      await queryClient.invalidateQueries({ queryKey: piKeys.runtimeStatus });
    },
    onError: (error: unknown) => {
      toast.error(
        t("settings.piRuntime.syncFailed", {
          error: extractErrorMessage(error),
        }),
      );
    },
  });
}

export function isProxyOffDetail(detail: string): boolean {
  const lower = detail.toLowerCase();
  return (
    lower.includes("please start the local proxy") ||
    lower.includes("local proxy is not running") ||
    lower.includes("connection refused") ||
    lower.includes("/ping") ||
    detail.includes("请先开启本地代理") ||
    detail.includes("請先開啟本機代理")
  );
}

export function useTestPiProxy() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: () => piApi.testProxy(),
    onSuccess: (health) => {
      if (health.reachable) {
        toast.success(
          t("settings.piRuntime.proxyReachable", {
            latency: health.latencyMs ?? 0,
          }),
        );
        void queryClient.invalidateQueries({ queryKey: piKeys.proxyPlan });
        return;
      }
      const raw = health.error ?? "";
      toast.error(
        t("settings.piRuntime.proxyUnreachable", {
          error: isProxyOffDetail(raw)
            ? t("settings.piRuntime.proxyStartFirst")
            : raw || t("settings.piRuntime.proxyStartFirst"),
        }),
      );
    },
    onError: (error: unknown) => {
      const raw = extractErrorMessage(error);
      toast.error(
        t("settings.piRuntime.proxyTestFailed", {
          error: isProxyOffDetail(raw)
            ? t("settings.piRuntime.proxyStartFirst")
            : raw,
        }),
      );
    },
  });
}
