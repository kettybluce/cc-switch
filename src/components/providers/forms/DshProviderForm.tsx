import { useEffect, useMemo, useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";
import { toast } from "@/lib/toast";
import { Button } from "@/components/ui/button";
import { Form, FormField, FormItem, FormMessage } from "@/components/ui/form";
import { ImeSafeInput } from "@/components/ui/ime-safe-input";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import JsonEditor from "@/components/JsonEditor";
import { useDarkMode } from "@/hooks/useDarkMode";
import { useProvidersQuery } from "@/lib/query/queries";
import type { ProviderFormData } from "@/lib/schemas/provider";
import type { ProviderFormProps, ProviderFormValues } from "./ProviderForm";
import { BasicFormFields } from "./BasicFormFields";

/**
 * DSH（DeepSeek Harness）供应商表单。
 *
 * 与其它应用的差异来自 DSH 自身的配置契约：
 *
 * - 供应商的标识就是 `cordis.patch.yml` 里 `providers` 的映射键，**写入后不能改名**
 *   （后端 `dsh::update` 直接拒绝 rename），故这里像 MCode 一样只在新增时可编辑。
 * - 凭据是 `apiKeyEnv`，存的是环境变量名而不是密钥（方案 A）。表单因此不收 API Key，
 *   只收变量名，并明确提示它要在 WSL 的环境变量里存在。
 * - `models` 是**整体替换**语义：填了就替代 pi-ai 自带目录，不填则沿用目录默认。
 *
 * 因此这里没有沿用通用表单的「基础字段 + JSON 混排」，而是把 DSH 真正需要的几个字段
 * 摆成结构化表单，底部保留完整 JSON 作为逃生舱。
 */

const API_FORMATS = [
  "openai-completions",
  "openai-responses",
  "anthropic-messages",
] as const;

type DshApi = (typeof API_FORMATS)[number];

interface DshModelRow {
  id: string;
  name: string;
  contextWindow: string;
  maxTokens: string;
}

interface DshDraft {
  displayName: string;
  apiKeyEnv: string;
  api: DshApi | "";
  baseURL: string;
  models: DshModelRow[];
}

/** 与后端 `validate_provider_json` 同一口径：只校验「能否安全写入」的下限。 */
const KEY_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

const normalizeKey = (value: string) =>
  value
    .toLowerCase()
    .replace(/[^a-z0-9-]/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");

/** 从已有卡片（数据库里存的就是 DSH 的 provider 对象）还原草稿。 */
function draftFromConfig(config: Record<string, unknown> | undefined): DshDraft {
  const models = Array.isArray(config?.models) ? config.models : [];
  return {
    displayName: String(config?.displayName ?? ""),
    apiKeyEnv: String(config?.apiKeyEnv ?? ""),
    api: (API_FORMATS as readonly string[]).includes(String(config?.api))
      ? (config?.api as DshApi)
      : "",
    baseURL: String(config?.baseURL ?? ""),
    models: models.flatMap((raw) => {
      if (!raw || typeof raw !== "object") return [];
      const model = raw as Record<string, unknown>;
      const id = typeof model.id === "string" ? model.id : "";
      if (!id) return [];
      return [
        {
          id,
          name: typeof model.name === "string" ? model.name : "",
          contextWindow:
            typeof model.contextWindow === "number"
              ? String(model.contextWindow)
              : "",
          maxTokens:
            typeof model.maxTokens === "number" ? String(model.maxTokens) : "",
        },
      ];
    }),
  };
}

/**
 * 草稿 → 写进 patch 的 provider 对象。
 *
 * 只写用户填了的字段：DSH 侧对缺省字段有自己的默认值（`defaultContextWindow`
 * 等），凭空补一个数字反而会覆盖那些默认。
 */
function configFromDraft(
  draft: DshDraft,
  displayNameFallback: string,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const display = draft.displayName.trim() || displayNameFallback.trim();
  if (display) out.displayName = display;
  if (draft.apiKeyEnv.trim()) out.apiKeyEnv = draft.apiKeyEnv.trim();
  if (draft.api) out.api = draft.api;
  if (draft.baseURL.trim()) out.baseURL = draft.baseURL.trim();
  if (draft.models.length > 0) {
    out.models = draft.models.map((model) => {
      const entry: Record<string, unknown> = { id: model.id.trim() };
      if (model.name.trim()) entry.name = model.name.trim();
      const contextWindow = Number(model.contextWindow);
      if (model.contextWindow.trim() && Number.isFinite(contextWindow)) {
        entry.contextWindow = contextWindow;
      }
      const maxTokens = Number(model.maxTokens);
      if (model.maxTokens.trim() && Number.isFinite(maxTokens)) {
        entry.maxTokens = maxTokens;
      }
      return entry;
    });
  }
  return out;
}

export function DshProviderForm({
  providerId,
  initialData,
  onSubmit,
  onCancel,
  submitLabel,
  showButtons = true,
  onSubmittingChange,
  onSubmitReadyChange,
}: ProviderFormProps) {
  const { t } = useTranslation();
  const isDarkMode = useDarkMode();
  const isEdit = Boolean(initialData);
  const [providerKey, setProviderKey] = useState(providerId ?? "");
  const [draft, setDraft] = useState<DshDraft>(() =>
    draftFromConfig(initialData?.settingsConfig),
  );
  const [jsonText, setJsonText] = useState(() =>
    JSON.stringify(initialData?.settingsConfig ?? {}, null, 2),
  );
  const [jsonValid, setJsonValid] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const { data: existing } = useProvidersQuery("dsh");
  const existingKeys = useMemo(
    () => new Set(Object.keys(existing?.providers ?? {})),
    [existing],
  );
  const keyTaken = !isEdit && providerKey !== "" && existingKeys.has(providerKey);
  const keyInvalid = !isEdit && providerKey !== "" && !KEY_PATTERN.test(providerKey);

  const form = useForm<ProviderFormData>({
    defaultValues: {
      name: initialData?.name ?? "",
      notes: initialData?.notes ?? "",
      websiteUrl: initialData?.websiteUrl ?? "",
      icon: initialData?.icon ?? "",
      iconColor: initialData?.iconColor ?? "",
      settingsConfig: jsonText,
    },
  });
  const name = form.watch("name");

  /**
   * DSH 侧对「目录里没有的 route」要求 `api` + `baseURL` + 非空 `models` 三者齐全，
   * 否则整条 provider 在 DSH 启动时被拒载（见 dsh-llm-pi-ai 的 Failures 一节）。
   * 与其存进去等 DSH 报错，不如在这里拦住。
   */
  const routeIncomplete =
    (draft.api !== "" || draft.baseURL.trim() !== "" || draft.models.length > 0) &&
    (draft.api === "" || draft.baseURL.trim() === "" || draft.models.length === 0);

  const ready = Boolean(
    jsonValid &&
      name.trim() &&
      (isEdit || (providerKey && !keyInvalid && !keyTaken)) &&
      !routeIncomplete,
  );

  useEffect(() => {
    onSubmitReadyChange?.(ready);
  }, [ready, onSubmitReadyChange]);

  // 结构化字段改动 → 同步底部 JSON。JSON 是最终写进 patch 的内容，
  // 所以这里必须**合并**到现有对象上，不能整体替换，否则用户在 JSON 里
  // 手填的、表单不认识的字段（compat / retryPolicy / headers 等）会被抹掉。
  const patchDraft = (next: Partial<DshDraft>) => {
    const merged = { ...draft, ...next };
    setDraft(merged);
    let current: Record<string, unknown> = {};
    try {
      const parsed = JSON.parse(jsonText);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        current = parsed as Record<string, unknown>;
      }
    } catch {
      // JSON 当前不合法：以结构化字段为准重建，用户正在改的就是它
    }
    setJsonText(
      JSON.stringify(
        {
          ...current,
          ...configFromDraft(merged, String(current.displayName ?? "")),
        },
        null,
        2,
      ),
    );
  };

  const updateModel = (index: number, next: Partial<DshModelRow>) => {
    patchDraft({
      models: draft.models.map((model, i) =>
        i === index ? { ...model, ...next } : model,
      ),
    });
  };

  return (
    <Form {...form}>
      <form
        id="provider-form"
        className="space-y-6"
        onSubmit={form.handleSubmit(async (identity) => {
          if (!ready || busy) return;
          setBusy(true);
          onSubmittingChange?.(true);
          setError("");
          try {
            let settingsConfig: Record<string, unknown>;
            try {
              settingsConfig = JSON.parse(jsonText) as Record<string, unknown>;
            } catch {
              settingsConfig = configFromDraft(draft, identity.name);
            }
            const payload: ProviderFormValues = {
              ...identity,
              name: identity.name.trim(),
              providerKey: isEdit ? providerId : providerKey,
              // 与 MCode 同口径：分类走 presetCategory，由 AddProviderDialog 落到 category
              presetCategory: initialData?.category ?? "custom",
              meta: initialData?.meta,
              settingsConfig: JSON.stringify(settingsConfig),
            };
            await onSubmit(payload);
          } catch (submitError) {
            const message =
              typeof submitError === "string"
                ? submitError
                : submitError instanceof Error
                  ? submitError.message
                  : JSON.stringify(submitError);
            setError(message);
            toast.error(message);
          } finally {
            setBusy(false);
            onSubmittingChange?.(false);
          }
        })}
      >
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}

        <fieldset disabled={busy} className="min-w-0 space-y-6 border-0 p-0">
          <BasicFormFields
            form={form}
            beforeNameSlot={
              <div className="space-y-2">
                <Label htmlFor="dsh-provider-key">
                  {t("dsh.form.providerKey", {
                    defaultValue: "供应商标识",
                  })}
                  <span aria-hidden="true" className="text-destructive ml-1">
                    *
                  </span>
                </Label>
                <Input
                  id="dsh-provider-key"
                  value={providerKey}
                  onChange={(event) =>
                    setProviderKey(normalizeKey(event.target.value))
                  }
                  disabled={isEdit}
                  placeholder={t("dsh.form.providerKeyPlaceholder", {
                    defaultValue: "例如 deepseek-official",
                  })}
                  autoComplete="off"
                  className={keyTaken || keyInvalid ? "border-destructive" : ""}
                />
                <p
                  className={
                    keyTaken || keyInvalid
                      ? "text-xs text-destructive"
                      : "text-xs text-fg-2"
                  }
                >
                  {keyTaken
                    ? t("opencode.providerKeyDuplicate")
                    : keyInvalid
                      ? t("opencode.providerKeyInvalid")
                      : isEdit
                        ? t("dsh.form.providerKeyLocked", {
                            defaultValue:
                              "已写入 cordis.patch.yml，供应商标识不可修改",
                          })
                        : t("dsh.form.providerKeyHint", {
                            defaultValue:
                              "小写字母、数字与连字符；写入后不可改名",
                          })}
                </p>
              </div>
            }
          />

          <div className="space-y-2">
            <Label htmlFor="dsh-apikey-env">
              {t("dsh.form.apiKeyEnv", { defaultValue: "API Key 环境变量名" })}
            </Label>
            <ImeSafeInput
              id="dsh-apikey-env"
              value={draft.apiKeyEnv}
              onValueChange={(value) => patchDraft({ apiKeyEnv: value })}
              placeholder="DEEPSEEK_API_KEY"
              className="font-mono"
            />
            <p className="text-xs text-fg-2">
              {t("dsh.form.apiKeyEnvHint", {
                defaultValue:
                  "DSH 只存变量名，不存密钥。请确认该变量已在 WSL 的 shell 环境里设置。",
              })}
            </p>
          </div>

          <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_160px]">
            <div className="space-y-2">
              <Label htmlFor="dsh-base-url">Base URL</Label>
              <ImeSafeInput
                id="dsh-base-url"
                value={draft.baseURL}
                onValueChange={(value) => patchDraft({ baseURL: value })}
                placeholder="https://api.deepseek.com/v1"
                className="font-mono"
              />
            </div>
            <div className="space-y-2">
              <Label>{t("dsh.form.api", { defaultValue: "协议" })}</Label>
              <Select
                value={draft.api || "none"}
                onValueChange={(value) =>
                  patchDraft({
                    api: value === "none" ? "" : (value as DshApi),
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">
                    {t("dsh.form.apiCatalogDefault", {
                      defaultValue: "目录默认",
                    })}
                  </SelectItem>
                  {API_FORMATS.map((api) => (
                    <SelectItem key={api} value={api}>
                      {api}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <Label>{t("dsh.form.models", { defaultValue: "模型" })}</Label>
              <Button
                type="button"
                variant="quiet"
                size="compact"
                onClick={() =>
                  patchDraft({
                    models: [
                      ...draft.models,
                      { id: "", name: "", contextWindow: "", maxTokens: "" },
                    ],
                  })
                }
              >
                <Plus className="h-3.5 w-3.5" />
                {t("dsh.form.addModel", { defaultValue: "添加模型" })}
              </Button>
            </div>
            {draft.models.length === 0 ? (
              <p className="text-xs text-fg-2">
                {t("dsh.form.modelsEmptyHint", {
                  defaultValue:
                    "留空则沿用 pi-ai 目录里这家 route 的模型清单；填了就会整体替换该清单。",
                })}
              </p>
            ) : (
              <div className="space-y-2">
                {draft.models.map((model, index) => (
                  <div
                    key={index}
                    className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_120px_120px_auto]"
                  >
                    <Input
                      value={model.id}
                      onChange={(event) =>
                        updateModel(index, { id: event.target.value })
                      }
                      placeholder={t("dsh.form.modelId", {
                        defaultValue: "模型 ID",
                      })}
                      className="font-mono"
                    />
                    <Input
                      value={model.name}
                      onChange={(event) =>
                        updateModel(index, { name: event.target.value })
                      }
                      placeholder={t("dsh.form.modelName", {
                        defaultValue: "显示名（可选）",
                      })}
                    />
                    <Input
                      value={model.contextWindow}
                      onChange={(event) =>
                        updateModel(index, { contextWindow: event.target.value })
                      }
                      placeholder="context"
                      inputMode="numeric"
                      className="font-mono"
                    />
                    <Input
                      value={model.maxTokens}
                      onChange={(event) =>
                        updateModel(index, { maxTokens: event.target.value })
                      }
                      placeholder="maxTokens"
                      inputMode="numeric"
                      className="font-mono"
                    />
                    <Button
                      type="button"
                      variant="quiet"
                      size="icon-compact"
                      aria-label={t("dsh.form.removeModel", {
                        defaultValue: "删除模型",
                      })}
                      onClick={() =>
                        patchDraft({
                          models: draft.models.filter((_, i) => i !== index),
                        })
                      }
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                ))}
              </div>
            )}
            {routeIncomplete && (
              <p className="text-xs text-destructive">
                {t("dsh.form.incompleteRoute", {
                  defaultValue:
                    "自定义 route 必须同时填协议、Base URL 和至少一个模型；否则 DSH 会拒绝加载这条配置。",
                })}
              </p>
            )}
          </div>
        </fieldset>

        <div className="space-y-2">
          <Label htmlFor="dsh-settings-config">
            {t("dsh.form.rawConfig", {
              defaultValue: "完整配置（写入 cordis.patch.yml 的内容）",
            })}
          </Label>
          <JsonEditor
            id="dsh-settings-config"
            ariaLabel={t("dsh.form.rawConfig", {
              defaultValue: "完整配置（写入 cordis.patch.yml 的内容）",
            })}
            value={jsonText}
            darkMode={isDarkMode}
            language="json"
            showValidation
            height={240}
            onChange={(text) => {
              setJsonText(text);
              try {
                const next = JSON.parse(text) as Record<string, unknown>;
                if (!next || typeof next !== "object" || Array.isArray(next)) {
                  setJsonValid(false);
                  return;
                }
                setJsonValid(true);
                setDraft(draftFromConfig(next));
              } catch {
                setJsonValid(false);
              }
            }}
          />
          <p className="text-xs text-fg-2">
            {t("dsh.form.rawConfigHint", {
              defaultValue:
                "这里直接决定写进 DSH 的 provider 对象；未知字段会原样保留。",
            })}
          </p>
          <FormField
            control={form.control}
            name="settingsConfig"
            render={() => (
              <FormItem className="space-y-0">
                <FormMessage />
              </FormItem>
            )}
          />
        </div>

        {showButtons && (
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              onClick={onCancel}
              disabled={busy}
            >
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={busy || !ready}>
              {submitLabel}
            </Button>
          </div>
        )}
      </form>
    </Form>
  );
}
