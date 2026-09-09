import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  Loader2,
  Monitor,
  Radio,
  RefreshCw,
  Terminal,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Label } from "@/components/ui/label";
import { ToggleRow } from "@/components/ui/toggle-row";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import type { PiFeatureFlags, PiRuntimeKind } from "@/lib/api/pi";
import {
  usePiProxyPlan,
  usePiRuntimeStatus,
  usePiWslDistros,
  useSetPiRuntime,
  useTestPiProxy,
} from "@/lib/query/pi";

export type CliRuntimeApp = "claude" | "codex";

/**
 * Settings → 通用 sections for Claude and Codex, parallel to Pi.
 *
 * Live files follow the shared WSL runtime (same distro as Pi). They are read
 * in place from the distribution home (`\\wsl.localhost\…`, like upstream
 * CC Switch) and written through wsl.exe. Location pickers write that shared
 * runtime so the user does not have to hunt in the Claude/Codex app tabs.
 */
export function ClaudeRuntimeSettings() {
  return <CliRuntimeSettings app="claude" />;
}

export function CodexRuntimeSettings() {
  return <CliRuntimeSettings app="codex" />;
}

function CliRuntimeSettings({ app }: { app: CliRuntimeApp }) {
  const { t } = useTranslation();
  const [pickerOpen, setPickerOpen] = useState(false);
  const prefix = `settings.cliRuntime.${app}` as const;

  const { data: status, isLoading, refetch, isFetching } = usePiRuntimeStatus();
  const setRuntime = useSetPiRuntime();
  const testProxy = useTestPiProxy();
  const {
    takeoverStatus,
    setTakeoverForApp,
    isPending: isTakeoverPending,
    isInitialStatusPending,
  } = useProxyStatus();
  const takeoverEnabled = Boolean(takeoverStatus?.[app]);

  const isWsl = status?.target.kind === "wsl";
  const { data: distros, isFetching: isProbing } = usePiWslDistros(
    Boolean(status?.wslAvailable) && (pickerOpen || isWsl),
  );
  const { data: proxyPlan } = usePiProxyPlan(Boolean(status));

  const selectedDistro = status?.settings.distro ?? "";
  const options = useMemo(() => {
    const probed = distros ?? [];
    if (selectedDistro && !probed.some((p) => p.distro === selectedDistro)) {
      return [selectedDistro, ...probed.map((p) => p.distro)];
    }
    return probed.map((probe) => probe.distro);
  }, [distros, selectedDistro]);

  const applyRuntime = (
    kind: PiRuntimeKind,
    distro: string | undefined,
    flags: PiFeatureFlags,
  ) => {
    setRuntime.mutate({ kind, distro, flags });
  };

  if (isLoading || !status) {
    return (
      <section className="space-y-2">
        <header className="space-y-1">
          <h3 className="text-sm font-medium">{t(`${prefix}.title`)}</h3>
          <p className="text-xs text-muted-foreground">
            {t(`${prefix}.description`)}
          </p>
        </header>
        <div className="flex justify-center py-6">
          <Loader2 className="size-4 animate-spin text-muted-foreground" />
        </div>
      </section>
    );
  }

  const showLocationPicker = status.wslAvailable;
  const { settings } = status;
  const statusLines = cliStatusLines(app, status.target);

  return (
    <section className="space-y-3">
      <div className="flex items-start justify-between gap-3">
        <header className="space-y-1">
          <h3 className="text-sm font-medium">{t(`${prefix}.title`)}</h3>
          <p className="text-xs text-muted-foreground">
            {t(`${prefix}.description`)}
          </p>
        </header>
        <Button
          variant="ghost"
          size="icon"
          className="size-7 shrink-0"
          aria-label={t("common.refresh")}
          disabled={isFetching}
          onClick={() => void refetch()}
        >
          <RefreshCw
            className={`size-3.5 ${isFetching ? "animate-spin" : ""}`}
          />
        </Button>
      </div>

      <p className="text-xs text-muted-foreground">
        {t("settings.cliRuntime.sharedRuntime")}
      </p>
      <p className="text-xs text-muted-foreground">
        {t("settings.cliRuntime.wslHome")}
      </p>

      {showLocationPicker && (
        <div className="space-y-3">
          <div className="space-y-1.5">
            <Label className="text-sm font-normal">
              {t(`${prefix}.location`)}
            </Label>
            <Select
              value={settings.kind}
              onValueChange={(value) =>
                applyRuntime(
                  value as PiRuntimeKind,
                  value === "wsl" ? settings.distro : undefined,
                  settings.flags,
                )
              }
              disabled={setRuntime.isPending}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="local">
                  <span className="flex items-center gap-2">
                    <Monitor className="size-3.5" />
                    {t("settings.piRuntime.local")}
                  </span>
                </SelectItem>
                <SelectItem value="wsl">
                  <span className="flex items-center gap-2">
                    <Terminal className="size-3.5" />
                    {t("settings.piRuntime.wsl")}
                  </span>
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          {settings.kind === "wsl" && (
            <div className="space-y-1.5">
              <Label className="text-sm font-normal">
                {t("settings.piRuntime.distro")}
              </Label>
              <Select
                value={selectedDistro}
                onValueChange={(distro) =>
                  applyRuntime("wsl", distro, settings.flags)
                }
                onOpenChange={(open) => open && setPickerOpen(true)}
                disabled={setRuntime.isPending}
              >
                <SelectTrigger>
                  <SelectValue
                    placeholder={t("settings.piRuntime.distroPlaceholder")}
                  />
                </SelectTrigger>
                <SelectContent>
                  {isProbing && options.length === 0 ? (
                    <div className="flex items-center gap-2 px-2 py-1.5 text-xs text-muted-foreground">
                      <Loader2 className="size-3 animate-spin" />
                      {t("settings.piRuntime.probing")}
                    </div>
                  ) : options.length === 0 ? (
                    <div className="px-2 py-1.5 text-xs text-muted-foreground">
                      {t("settings.cliRuntime.noDistros")}
                    </div>
                  ) : (
                    options.map((distro) => (
                      <SelectItem key={distro} value={distro}>
                        {distro}
                      </SelectItem>
                    ))
                  )}
                </SelectContent>
              </Select>
            </div>
          )}
        </div>
      )}

      <div className="space-y-3 rounded-lg border border-border/60 p-3">
        {statusLines.map((line) => (
          <div
            key={line.label}
            className="flex items-center justify-between gap-3 text-xs"
          >
            <span className="text-muted-foreground">{t(line.label)}</span>
            <span
              className="flex min-w-0 items-center gap-1.5 font-mono text-foreground"
              title={line.path}
            >
              <span className="min-w-0 truncate">{line.path}</span>
              <CheckCircle2
                className={`size-3 shrink-0 ${isWsl ? "text-green-500" : "text-muted-foreground"}`}
              />
            </span>
          </div>
        ))}
      </div>

      <div className="space-y-3 rounded-lg border border-border/60 p-3">
        <ToggleRow
          icon={
            <Radio
              className={`h-4 w-4 ${takeoverEnabled ? "text-emerald-500" : "text-muted-foreground"}`}
            />
          }
          title={t(`${prefix}.takeover`)}
          description={t(`${prefix}.takeoverDescription`)}
          checked={takeoverEnabled}
          onCheckedChange={(enabled) =>
            void setTakeoverForApp({ appType: app, enabled })
          }
          disabled={isTakeoverPending || isInitialStatusPending}
        />
        {proxyPlan?.gateway && (
          <p className="text-xs text-muted-foreground">
            {proxyPlan.gateway.reachable
              ? t("settings.piRuntime.proxyHealthOk", {
                  host: proxyPlan.gateway.host ?? "",
                  strategy: proxyPlan.gateway.strategy
                    ? t(
                        `settings.piRuntime.strategy.${proxyPlan.gateway.strategy}`,
                      )
                    : t("settings.piRuntime.localLoopback"),
                })
              : (proxyPlan.gateway.error ?? t("settings.piRuntime.noRoute"))}
          </p>
        )}
        {proxyPlan?.origin && (
          <p
            className="font-mono text-xs text-muted-foreground"
            title={proxyPlan.origin}
          >
            {t("settings.piRuntime.resolvedEndpoint", {
              endpoint: proxyPlan.origin,
            })}
          </p>
        )}
        <Button
          variant="secondary"
          size="sm"
          className="h-7 text-xs"
          disabled={testProxy.isPending}
          onClick={() => testProxy.mutate()}
        >
          {testProxy.isPending && (
            <Loader2 className="mr-1.5 size-3 animate-spin" />
          )}
          {t("settings.piRuntime.testProxy")}
        </Button>
      </div>
    </section>
  );
}

/**
 * Path the user can paste into Explorer: the upstream-style
 * `\\wsl.localhost\<distro>\home\<user>\.claude\settings.json`.
 */
function displayLinuxPath(
  target: { kind: "local" } | { kind: "wsl"; distro: string; home: string },
  relative: string,
): string {
  if (target.kind !== "wsl") {
    return `~/${relative}`;
  }
  const home = target.home.replace(/\/$/, "").replace(/^\//, "");
  return `\\\\wsl.localhost\\${target.distro}\\${home}\\${relative}`.replace(
    /\//g,
    "\\",
  );
}

function cliStatusLines(
  app: CliRuntimeApp,
  target: { kind: "local" } | { kind: "wsl"; distro: string; home: string },
): { label: string; path: string }[] {
  if (app === "claude") {
    return [
      {
        label: "settings.cliRuntime.claude.settingsPath",
        path: displayLinuxPath(target, ".claude/settings.json"),
      },
    ];
  }
  return [
    {
      label: "settings.cliRuntime.codex.settingsPath",
      path: displayLinuxPath(target, ".codex/config.toml"),
    },
    {
      label: "settings.cliRuntime.codex.authPath",
      path: displayLinuxPath(target, ".codex/auth.json"),
    },
  ];
}
