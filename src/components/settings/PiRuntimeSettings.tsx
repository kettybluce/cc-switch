import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertCircle,
  CheckCircle2,
  Loader2,
  Monitor,
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
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import type {
  PiFeatureFlags,
  PiProxyHealth,
  PiRuntimeKind,
  WslPiProbe,
} from "@/lib/api/pi";
import {
  usePiProxyPlan,
  usePiRuntimeStatus,
  usePiWslDistros,
  useSetPiRuntime,
  useSyncPiWslSessions,
  useTestPiProxy,
} from "@/lib/query/pi";

/**
 * Runtime selection for Pi.
 *
 * Pi can be installed for the CC Switch user or inside a WSL2 distribution.
 * The two are genuinely different installations with their own `models.json`
 * and their own sessions, so this is a choice CC Switch has to be told about
 * rather than one it can merge.
 */
export function PiRuntimeSettings() {
  const { t } = useTranslation();
  const [pickerOpen, setPickerOpen] = useState(false);

  const { data: status, isLoading, refetch, isFetching } = usePiRuntimeStatus();
  const setRuntime = useSetPiRuntime();
  const syncSessions = useSyncPiWslSessions();
  const testProxy = useTestPiProxy();

  const isWsl = status?.target.kind === "wsl";
  // Probing every distribution starts a login shell in each, so the list is
  // only fetched once the user opens the picker or already uses WSL.
  const { data: distros, isFetching: isProbing } = usePiWslDistros(
    Boolean(status?.wslAvailable) && (pickerOpen || isWsl),
  );
  const { data: proxyPlan } = usePiProxyPlan(isWsl);

  const selectedDistro = status?.settings.distro ?? "";
  const options = useMemo(() => {
    const probed = distros ?? [];
    // Keep the saved distribution selectable even while probing, so the
    // control never flashes back to an empty value on refresh.
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
        <RuntimeHeader />
        <div className="flex justify-center py-6">
          <Loader2 className="size-4 animate-spin text-muted-foreground" />
        </div>
      </section>
    );
  }

  // WSL only exists on Windows, so elsewhere there is no choice to present.
  // The backend is the authority here rather than the user agent string.
  if (!status.wslAvailable) {
    return null;
  }

  const { settings, probe } = status;

  return (
    <section className="space-y-3">
      <div className="flex items-start justify-between gap-3">
        <RuntimeHeader />
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

      <div className="space-y-3">
        <div className="space-y-1.5">
          <Label className="text-sm font-normal">
            {t("settings.piRuntime.location")}
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
                    {t("settings.piRuntime.noDistros")}
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

      {status.error && (
        <p
          role="alert"
          className="flex items-start gap-1.5 text-xs text-yellow-600 dark:text-yellow-400"
        >
          <AlertCircle className="mt-0.5 size-3 shrink-0" />
          <span>{status.error.message}</span>
        </p>
      )}

      {isWsl && probe && (
        <WslRuntimeDetails
          probe={probe}
          proxyEnabled={settings.flags.wslProxy}
          proxyHealth={proxyPlan?.gateway}
          forwardProxy={proxyPlan?.forwardProxy}
          syncing={syncSessions.isPending}
          testing={testProxy.isPending}
          savingFlags={setRuntime.isPending}
          onSync={() => syncSessions.mutate()}
          onTestProxy={() => testProxy.mutate()}
          onToggleProxy={(wslProxy) =>
            applyRuntime("wsl", settings.distro, {
              ...settings.flags,
              wslProxy,
            })
          }
        />
      )}
    </section>
  );
}

function RuntimeHeader() {
  const { t } = useTranslation();
  return (
    <header className="space-y-1">
      <h3 className="text-sm font-medium">{t("settings.piRuntime.title")}</h3>
      <p className="text-xs text-muted-foreground">
        {t("settings.piRuntime.description")}
      </p>
    </header>
  );
}

interface WslRuntimeDetailsProps {
  probe: WslPiProbe;
  proxyEnabled: boolean;
  proxyHealth?: PiProxyHealth;
  forwardProxy?: string;
  syncing: boolean;
  testing: boolean;
  savingFlags: boolean;
  onSync: () => void;
  onTestProxy: () => void;
  onToggleProxy: (enabled: boolean) => void;
}

function WslRuntimeDetails({
  probe,
  proxyEnabled,
  proxyHealth,
  forwardProxy,
  syncing,
  testing,
  savingFlags,
  onSync,
  onTestProxy,
  onToggleProxy,
}: WslRuntimeDetailsProps) {
  const { t } = useTranslation();

  return (
    <div className="space-y-3 rounded-lg border border-border/60 p-3">
      <dl className="space-y-1.5 text-xs">
        <DetailRow
          label={t("settings.piRuntime.piVersion")}
          value={probe.piVersion ?? t("settings.piRuntime.piMissing")}
          ok={Boolean(probe.piPath)}
        />
        <DetailRow
          label={t("settings.piRuntime.agentDir")}
          value={probe.agentDir}
          ok={probe.hasModels}
        />
        <DetailRow
          label={t("settings.piRuntime.sessions")}
          value={t("settings.piRuntime.sessionCount", {
            count: probe.sessionCount,
          })}
          ok={probe.capabilities.sessions}
        />
      </dl>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="secondary"
          size="sm"
          className="h-7 text-xs"
          disabled={syncing}
          onClick={onSync}
        >
          {syncing ? (
            <Loader2 className="mr-1.5 size-3 animate-spin" />
          ) : (
            <RefreshCw className="mr-1.5 size-3" />
          )}
          {t("settings.piRuntime.syncSessions")}
        </Button>
        <Button
          variant="secondary"
          size="sm"
          className="h-7 text-xs"
          disabled={testing}
          onClick={onTestProxy}
        >
          {testing && <Loader2 className="mr-1.5 size-3 animate-spin" />}
          {t("settings.piRuntime.testProxy")}
        </Button>
      </div>

      <div className="space-y-1.5 border-t border-border/60 pt-3">
        <div className="flex items-center justify-between gap-3">
          <Label
            htmlFor="pi-wsl-proxy"
            className="text-sm font-normal leading-snug"
          >
            {t("settings.piRuntime.useProxy")}
          </Label>
          <Switch
            id="pi-wsl-proxy"
            checked={proxyEnabled}
            disabled={savingFlags}
            onCheckedChange={onToggleProxy}
          />
        </div>
        <p className="text-xs text-muted-foreground">
          {t("settings.piRuntime.useProxyDescription")}
        </p>
        {proxyHealth && (
          <p className="text-xs text-muted-foreground">
            {proxyHealth.reachable
              ? t("settings.piRuntime.routeVia", {
                  host: proxyHealth.host ?? "",
                  strategy: t(
                    `settings.piRuntime.strategy.${proxyHealth.strategy ?? "mirroredLoopback"}`,
                  ),
                })
              : (proxyHealth.error ?? t("settings.piRuntime.noRoute"))}
          </p>
        )}
        {forwardProxy && (
          <p className="text-xs text-muted-foreground">
            {t("settings.piRuntime.forwardProxy", { url: forwardProxy })}
          </p>
        )}
      </div>
    </div>
  );
}

function DetailRow({
  label,
  value,
  ok,
}: {
  label: string;
  value: string;
  ok: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="flex min-w-0 items-center gap-1.5">
        <span
          className="min-w-0 truncate font-mono text-foreground"
          title={value}
        >
          {value}
        </span>
        {ok ? (
          <CheckCircle2 className="size-3 shrink-0 text-green-500" />
        ) : (
          <AlertCircle className="size-3 shrink-0 text-yellow-500" />
        )}
      </dd>
    </div>
  );
}
