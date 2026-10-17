import { describe, expect, it } from "vitest";
import {
  configFromDraft,
  draftFromConfig,
  type DshDraft,
} from "@/components/providers/forms/DshProviderForm";

/**
 * DSH 表单的数据转换测试。
 *
 * 这两个纯函数是表单与 DSH 配置之间的唯一桥梁，也是阶段一最容易出错的地方：
 * - `draftFromConfig`：把已存卡片还原成可编辑草稿；
 * - `configFromDraft`：把草稿变成写进 `cordis.patch.yml` 的 provider 对象。
 *
 * 重点验证「只写用户填了的字段」——DSH 对缺省字段有自己的默认值
 * （defaultContextWindow 等），凭空补数字会覆盖那些默认。
 */

function emptyDraft(): DshDraft {
  return {
    displayName: "",
    apiKeyEnv: "",
    api: "",
    baseURL: "",
    models: [],
  };
}

describe("draftFromConfig", () => {
  it("returns an empty draft when config is missing or malformed", () => {
    expect(draftFromConfig(undefined)).toEqual(emptyDraft());
    expect(draftFromConfig({})).toEqual(emptyDraft());
    // models 不是数组、或条目没有 id 时不应崩
    expect(draftFromConfig({ models: "nope" }).models).toEqual([]);
    expect(draftFromConfig({ models: [{ name: "no id" }] }).models).toEqual([]);
    expect(draftFromConfig({ models: [null, 42, "x"] }).models).toEqual([]);
  });

  it("restores every supported field from a saved card", () => {
    const draft = draftFromConfig({
      displayName: "My CPA",
      apiKeyEnv: "K_CPA",
      api: "openai-responses",
      baseURL: "https://api.example.com/v1",
      models: [
        {
          id: "deepseek-v4.1-flash",
          name: "DeepSeek V4.1",
          contextWindow: 1000000,
          maxTokens: 32000,
        },
      ],
    });

    expect(draft.displayName).toBe("My CPA");
    expect(draft.apiKeyEnv).toBe("K_CPA");
    expect(draft.api).toBe("openai-responses");
    expect(draft.baseURL).toBe("https://api.example.com/v1");
    expect(draft.models).toEqual([
      {
        id: "deepseek-v4.1-flash",
        name: "DeepSeek V4.1",
        contextWindow: "1000000",
        maxTokens: "32000",
      },
    ]);
  });

  it("falls back to empty api when the stored value is not a known format", () => {
    // DSH 将来可能加新的 api 类型；不认识的值不能让表单卡住
    expect(draftFromConfig({ api: "some-future-format" }).api).toBe("");
    expect(draftFromConfig({ api: "openai-completions" }).api).toBe(
      "openai-completions",
    );
    expect(draftFromConfig({ api: "anthropic-messages" }).api).toBe(
      "anthropic-messages",
    );
  });

  it("keeps models whose id exists but leaves optional fields blank", () => {
    const draft = draftFromConfig({
      models: [{ id: "only-id" }, { id: "named", name: "Named" }],
    });
    expect(draft.models).toEqual([
      { id: "only-id", name: "", contextWindow: "", maxTokens: "" },
      { id: "named", name: "Named", contextWindow: "", maxTokens: "" },
    ]);
  });

  it("drops models with a non-string contextWindow instead of writing NaN", () => {
    const draft = draftFromConfig({
      models: [{ id: "m", contextWindow: "1M", maxTokens: null }],
    });
    expect(draft.models[0].contextWindow).toBe("");
    expect(draft.models[0].maxTokens).toBe("");
  });
});

describe("configFromDraft", () => {
  it("writes nothing for an untouched draft", () => {
    expect(configFromDraft(emptyDraft(), "")).toEqual({});
  });

  it("falls back to the provider key when displayName is blank", () => {
    const out = configFromDraft(emptyDraft(), "my-cpa");
    expect(out.displayName).toBe("my-cpa");
  });

  it("trims whitespace-only values away", () => {
    const out = configFromDraft(
      { ...emptyDraft(), displayName: "   ", apiKeyEnv: "  ", baseURL: "  " },
      "fallback",
    );
    expect(out.displayName).toBe("fallback");
    expect(out).not.toHaveProperty("apiKeyEnv");
    expect(out).not.toHaveProperty("baseURL");
  });

  it("omits api when none is selected", () => {
    expect(configFromDraft(emptyDraft(), "k")).not.toHaveProperty("api");
    expect(
      configFromDraft({ ...emptyDraft(), api: "openai-completions" }, "k").api,
    ).toBe("openai-completions");
  });

  it("omits the models key entirely when there are no rows", () => {
    // 关键：不写 models 让 DSH 沿用自带目录，而不是塞一个空数组把它覆盖掉
    const out = configFromDraft(emptyDraft(), "k");
    expect(out).not.toHaveProperty("models");
  });

  it("keeps only the id for a bare model row", () => {
    const out = configFromDraft(
      {
        ...emptyDraft(),
        models: [{ id: " m-1 ", name: "  ", contextWindow: "", maxTokens: "" }],
      },
      "k",
    );
    expect(out.models).toEqual([{ id: "m-1" }]);
  });

  it("converts numeric strings to numbers for contextWindow and maxTokens", () => {
    const out = configFromDraft(
      {
        ...emptyDraft(),
        models: [
          {
            id: "m",
            name: "M",
            contextWindow: "1000000",
            maxTokens: "32000",
          },
        ],
      },
      "k",
    );
    const models = out.models as Array<Record<string, unknown>>;
    expect(models[0].contextWindow).toBe(1000000);
    expect(models[0].maxTokens).toBe(32000);
    expect(typeof models[0].contextWindow).toBe("number");
  });

  it("ignores non-numeric numbers rather than writing NaN", () => {
    const out = configFromDraft(
      {
        ...emptyDraft(),
        models: [
          { id: "m", name: "", contextWindow: "abc", maxTokens: "12x" },
        ],
      },
      "k",
    );
    const models = out.models as Array<Record<string, unknown>>;
    expect(models[0]).toEqual({ id: "m" });
  });

  it("round-trips a realistic card without losing data", () => {
    const original = {
      displayName: "CPA",
      apiKeyEnv: "CLINE_API_KEY",
      api: "openai-completions",
      baseURL: "http://127.0.0.1:8317/v1",
      models: [
        { id: "deepseek-v4.1-flash", name: "ds", contextWindow: 1000000, maxTokens: 32000 },
        { id: "bare" },
      ],
    };
    const restored = configFromDraft(draftFromConfig(original), "cpa");
    expect(restored).toEqual(original);
  });
});
