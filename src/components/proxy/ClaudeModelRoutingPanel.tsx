import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Save, Loader2, Info } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  settingsApi,
  type ClaudeModelRoutingConfig,
  type ClaudeModelRoutingEntry,
} from "@/lib/api/settings";
import { useProvidersQuery } from "@/lib/query/queries";
import type { Provider } from "@/types";

function emptyEntry(): ClaudeModelRoutingEntry {
  return {
    clientModel: "",
    providerId: "",
    upstreamModel: "",
    label: "",
    description: "",
  };
}

function defaultClientModel(providerId: string, upstreamModel: string): string {
  const pid = providerId.trim();
  const model = upstreamModel.trim();
  if (!pid || !model) return "";
  return `${pid}/${model}`;
}

/** Collect candidate upstream model ids from a Claude provider's env / catalog. */
export function collectProviderModelCandidates(provider: Provider): string[] {
  const models = new Set<string>();
  const env = (provider.settingsConfig as { env?: Record<string, unknown> })
    ?.env;
  if (env && typeof env === "object") {
    for (const key of [
      "ANTHROPIC_MODEL",
      "ANTHROPIC_DEFAULT_HAIKU_MODEL",
      "ANTHROPIC_DEFAULT_SONNET_MODEL",
      "ANTHROPIC_DEFAULT_OPUS_MODEL",
      "ANTHROPIC_DEFAULT_FABLE_MODEL",
      "CLAUDE_CODE_SUBAGENT_MODEL",
    ]) {
      const value = env[key];
      if (typeof value === "string" && value.trim()) {
        models.add(value.trim().replace(/\[1[mM]\]$/i, ""));
      }
    }
  }
  const catalog = (
    provider.settingsConfig as {
      modelCatalog?: { models?: Array<{ id?: string; slug?: string }> };
    }
  )?.modelCatalog?.models;
  if (Array.isArray(catalog)) {
    for (const item of catalog) {
      const id = item?.id ?? item?.slug;
      if (typeof id === "string" && id.trim()) models.add(id.trim());
    }
  }
  return Array.from(models);
}

export function ClaudeModelRoutingPanel() {
  const { t } = useTranslation();
  const { data: providersData } = useProvidersQuery("claude");
  const providers = useMemo(
    () => Object.values(providersData?.providers ?? {}),
    [providersData],
  );

  const [config, setConfig] = useState<ClaudeModelRoutingConfig>({
    enabled: false,
    replaceBuiltInOptions: false,
    writeAvailableModels: false,
    enableGatewayDiscovery: false,
    entries: [],
  });
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    settingsApi
      .getClaudeModelRouting()
      .then(setConfig)
      .catch((e) => {
        console.error("Failed to load Claude model routing:", e);
        toast.error(String(e));
      })
      .finally(() => setIsLoading(false));
  }, []);

  const updateEntry = (
    index: number,
    patch: Partial<ClaudeModelRoutingEntry>,
  ) => {
    setConfig((prev) => {
      const entries = prev.entries.map((entry, i) => {
        if (i !== index) return entry;
        const next = { ...entry, ...patch };
        // Keep clientModel in sync with the stable default when provider/model change,
        // unless the user has customized it away from the previous default.
        const prevDefault = defaultClientModel(
          entry.providerId,
          entry.upstreamModel,
        );
        const providerChanged =
          patch.providerId !== undefined || patch.upstreamModel !== undefined;
        if (
          providerChanged &&
          (!entry.clientModel || entry.clientModel === prevDefault)
        ) {
          next.clientModel = defaultClientModel(
            next.providerId,
            next.upstreamModel,
          );
        }
        if (
          providerChanged &&
          (!entry.label ||
            entry.label === entry.upstreamModel ||
            entry.label === prevDefault)
        ) {
          const providerName =
            providers.find((p) => p.id === next.providerId)?.name ??
            next.providerId;
          next.label = next.upstreamModel
            ? `${providerName} / ${next.upstreamModel}`
            : providerName;
        }
        return next;
      });
      return { ...prev, entries };
    });
  };

  const handleSave = async () => {
    setIsSaving(true);
    try {
      const normalized: ClaudeModelRoutingConfig = {
        ...config,
        // Phase 1: never enable unimplemented gateway discovery from UI.
        enableGatewayDiscovery: false,
        entries: config.entries
          .map((e) => ({
            ...e,
            clientModel:
              e.clientModel.trim() ||
              defaultClientModel(e.providerId, e.upstreamModel),
            providerId: e.providerId.trim(),
            upstreamModel: e.upstreamModel.trim(),
            label: e.label.trim(),
            description: e.description.trim(),
          }))
          .filter((e) => e.providerId && e.upstreamModel && e.clientModel),
      };
      await settingsApi.setClaudeModelRouting(normalized);
      setConfig(normalized);
      toast.success(
        t("proxy.claudeModelRouting.saved", {
          defaultValue: "跨供应商模型目录已保存",
        }),
      );
    } catch (e) {
      console.error(e);
      toast.error(String(e));
    } finally {
      setIsSaving(false);
    }
  };

  if (isLoading) {
    return (
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("common.loading", { defaultValue: "加载中…" })}
      </div>
    );
  }

  return (
    <div className="space-y-4" data-testid="claude-model-routing-panel">
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-1">
          <h4 className="text-sm font-semibold">
            {t("proxy.claudeModelRouting.title", {
              defaultValue: "Claude 跨供应商模型目录",
            })}
          </h4>
          <p className="text-xs text-muted-foreground">
            {t("proxy.claudeModelRouting.description", {
              defaultValue:
                "接管开启且本功能启用时，将目录投影到 Claude Code settings.json 的 modelPicker，并按 clientModel 路由到对应供应商。需要 Claude Code ≥ 2.1.242。",
            })}
          </p>
        </div>
        <Switch
          checked={config.enabled}
          onCheckedChange={(checked) =>
            setConfig((prev) => ({ ...prev, enabled: checked }))
          }
          data-testid="claude-model-routing-enabled"
        />
      </div>

      <Alert>
        <Info className="h-4 w-4" />
        <AlertDescription className="text-xs">
          {t("proxy.claudeModelRouting.hint", {
            defaultValue:
              "clientModel 默认格式为「供应商ID/上游模型」（碰撞安全）。目录命中时不会热切换首页当前供应商，也不会走全局故障转移。保存后请重启 Claude Code 会话以重载 modelPicker。",
          })}
        </AlertDescription>
      </Alert>

      <div className="flex items-center justify-between">
        <Label className="text-xs text-muted-foreground">
          {t("proxy.claudeModelRouting.replaceBuiltIn", {
            defaultValue: "替换内置 sonnet/opus/…（默认关闭，保留内置）",
          })}
        </Label>
        <Switch
          checked={config.replaceBuiltInOptions}
          onCheckedChange={(checked) =>
            setConfig((prev) => ({
              ...prev,
              replaceBuiltInOptions: checked,
            }))
          }
          disabled={!config.enabled}
        />
      </div>

      <div className="space-y-3">
        {config.entries.map((entry, index) => {
          const provider = providers.find((p) => p.id === entry.providerId);
          const candidates = provider
            ? collectProviderModelCandidates(provider)
            : [];
          return (
            <div
              key={index}
              className="grid gap-2 rounded-lg border border-border/60 p-3"
              data-testid="claude-model-routing-entry"
            >
              <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
                <div className="space-y-1">
                  <Label className="text-xs">
                    {t("proxy.claudeModelRouting.provider", {
                      defaultValue: "供应商",
                    })}
                  </Label>
                  <select
                    className="flex h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
                    value={entry.providerId}
                    onChange={(e) =>
                      updateEntry(index, { providerId: e.target.value })
                    }
                    disabled={!config.enabled}
                    data-testid="claude-model-routing-provider"
                  >
                    <option value="">
                      {t("proxy.claudeModelRouting.selectProvider", {
                        defaultValue: "选择供应商…",
                      })}
                    </option>
                    {providers.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name} ({p.id})
                      </option>
                    ))}
                  </select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs">
                    {t("proxy.claudeModelRouting.upstreamModel", {
                      defaultValue: "上游模型",
                    })}
                  </Label>
                  <Input
                    list={`claude-model-routing-models-${index}`}
                    value={entry.upstreamModel}
                    onChange={(e) =>
                      updateEntry(index, { upstreamModel: e.target.value })
                    }
                    disabled={!config.enabled}
                    placeholder="deepseek-chat"
                    data-testid="claude-model-routing-upstream"
                  />
                  <datalist id={`claude-model-routing-models-${index}`}>
                    {candidates.map((m) => (
                      <option key={m} value={m} />
                    ))}
                  </datalist>
                </div>
              </div>
              <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
                <div className="space-y-1">
                  <Label className="text-xs">
                    {t("proxy.claudeModelRouting.clientModel", {
                      defaultValue: "clientModel（/model 可见）",
                    })}
                  </Label>
                  <Input
                    value={
                      entry.clientModel ||
                      defaultClientModel(entry.providerId, entry.upstreamModel)
                    }
                    onChange={(e) =>
                      updateEntry(index, { clientModel: e.target.value })
                    }
                    disabled={!config.enabled}
                    data-testid="claude-model-routing-client"
                  />
                </div>
                <div className="space-y-1">
                  <Label className="text-xs">
                    {t("proxy.claudeModelRouting.label", {
                      defaultValue: "显示名",
                    })}
                  </Label>
                  <Input
                    value={entry.label}
                    onChange={(e) =>
                      updateEntry(index, { label: e.target.value })
                    }
                    disabled={!config.enabled}
                  />
                </div>
              </div>
              <div className="flex items-end gap-2">
                <div className="flex-1 space-y-1">
                  <Label className="text-xs">
                    {t("proxy.claudeModelRouting.descriptionField", {
                      defaultValue: "描述",
                    })}
                  </Label>
                  <Input
                    value={entry.description}
                    onChange={(e) =>
                      updateEntry(index, { description: e.target.value })
                    }
                    disabled={!config.enabled}
                  />
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  disabled={!config.enabled}
                  onClick={() =>
                    setConfig((prev) => ({
                      ...prev,
                      entries: prev.entries.filter((_, i) => i !== index),
                    }))
                  }
                  aria-label={t("common.delete", { defaultValue: "删除" })}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
            </div>
          );
        })}
      </div>

      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!config.enabled}
          onClick={() =>
            setConfig((prev) => ({
              ...prev,
              entries: [...prev.entries, emptyEntry()],
            }))
          }
          data-testid="claude-model-routing-add"
        >
          <Plus className="h-4 w-4 mr-1" />
          {t("proxy.claudeModelRouting.add", { defaultValue: "添加模型" })}
        </Button>
        <Button
          type="button"
          size="sm"
          onClick={() => void handleSave()}
          disabled={isSaving}
          data-testid="claude-model-routing-save"
        >
          {isSaving ? (
            <Loader2 className="h-4 w-4 mr-1 animate-spin" />
          ) : (
            <Save className="h-4 w-4 mr-1" />
          )}
          {t("common.save", { defaultValue: "保存" })}
        </Button>
      </div>
    </div>
  );
}
