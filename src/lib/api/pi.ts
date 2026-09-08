import { invoke } from "@tauri-apps/api/core";
import type { UsageScript } from "@/types";

export interface PiCurrentState {
  enabledProviderIds: string[];
  defaultProviderId: string | null;
}

export type PiSessionDiscovery =
  | {
      status: "available";
    }
  | {
      status: "requires_project_context";
      configuredPath: string;
    }
  | {
      status: "unavailable";
      reason: string;
    };

/** Which runtime hosts Pi. `local` means the machine CC Switch runs on. */
export type PiRuntimeKind = "local" | "wsl";

/** Gradual-rollout switches for the Pi runtime. */
export interface PiFeatureFlags {
  wsl: boolean;
  /** Advanced opt-out. Defaults on so Pi can follow local-proxy takeover. */
  wslProxy: boolean;
  session: boolean;
  sessionIncremental: boolean;
  usage: boolean;
}

export interface PiRuntimeSettings {
  kind: PiRuntimeKind;
  distro?: string;
  flags: PiFeatureFlags;
}

/** What a discovered runtime target can actually do. Probed, not versioned. */
export interface PiCapabilities {
  models: boolean;
  sessions: boolean;
  usage: boolean;
  duration: boolean;
}

export interface WslPiProbe {
  distro: string;
  home: string;
  agentDir: string;
  piPath?: string | null;
  piVersion?: string | null;
  hasModels: boolean;
  hasSessions: boolean;
  sessionCount: number;
  capabilities: PiCapabilities;
}

export type PiRuntimeTarget =
  | { kind: "local" }
  | { kind: "wsl"; distro: string; home: string; agentDir: string };

export interface PiRuntimeError {
  code: string;
  message: string;
}

export interface PiRuntimeStatus {
  settings: PiRuntimeSettings;
  target: PiRuntimeTarget;
  probe?: WslPiProbe;
  /** A WSL selection that could not be honoured, without breaking the Pi UI. */
  error?: PiRuntimeError;
  wslAvailable: boolean;
}

export interface SessionSyncOutcome {
  fetched: number;
  unchanged: number;
  removed: number;
  total: number;
  bytes: number;
  truncated: boolean;
  errors: string[];
}

/** How the WSL-reachable address for the Windows host was found. */
export type PiProxyHostStrategy =
  | "mirroredLoopback"
  | "defaultGateway"
  | "resolvConf";

export interface PiProxyHealth {
  reachable: boolean;
  endpoint?: string;
  host?: string;
  strategy?: PiProxyHostStrategy;
  latencyMs?: number;
  protocol?: "http" | "https" | "socks5" | "socks5h";
  error?: string;
}

export interface PiProxyPlan {
  enabled: boolean;
  /** Live models.json currently points at the local proxy. */
  projected: boolean;
  gateway: PiProxyHealth;
  /** `http://host:port` used as the origin of rewritten baseUrls. */
  origin?: string;
  /** Local proxy listen address (`127.0.0.1` or `0.0.0.0`). */
  listenAddress?: string;
  /** Unused: Option B does not inject process-level proxy env vars. */
  forwardProxy?: string;
  forwardProxyHealth?: PiProxyHealth;
  environment: Record<string, string>;
}

export const piApi = {
  async getCurrentState(): Promise<PiCurrentState> {
    return await invoke("get_pi_current_state");
  },

  async getRuntimeStatus(): Promise<PiRuntimeStatus> {
    return await invoke("get_pi_runtime_status");
  },

  async listWslDistros(): Promise<WslPiProbe[]> {
    return await invoke("list_pi_wsl_distros");
  },

  async setRuntime(runtime: PiRuntimeSettings): Promise<PiRuntimeStatus> {
    return await invoke("set_pi_runtime", { runtime });
  },

  async syncWslSessions(): Promise<SessionSyncOutcome> {
    return await invoke("sync_pi_wsl_sessions");
  },

  async getProxyPlan(): Promise<PiProxyPlan> {
    return await invoke("get_pi_proxy_plan");
  },

  async testProxy(): Promise<PiProxyHealth> {
    return await invoke("test_pi_proxy");
  },

  async updateProviderUsageScript(
    id: string,
    usageScript: UsageScript,
  ): Promise<boolean> {
    return await invoke("update_pi_provider_usage_script", {
      id,
      usageScript,
    });
  },

  async getSessionDiscovery(): Promise<PiSessionDiscovery> {
    return await invoke("get_pi_session_discovery");
  },
};
