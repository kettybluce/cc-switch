import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  ClaudeModelRoutingPanel,
  collectProviderModelCandidates,
} from "@/components/proxy/ClaudeModelRoutingPanel";
import type { Provider } from "@/types";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, opts?: { defaultValue?: string }) =>
      opts?.defaultValue ?? key,
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

const getClaudeModelRouting = vi.fn();
const setClaudeModelRouting = vi.fn();

vi.mock("@/lib/api/settings", () => ({
  settingsApi: {
    getClaudeModelRouting: (...args: unknown[]) =>
      getClaudeModelRouting(...args),
    setClaudeModelRouting: (...args: unknown[]) =>
      setClaudeModelRouting(...args),
  },
}));

vi.mock("@/lib/query/queries", () => ({
  useProvidersQuery: () => ({
    data: {
      providers: {
        prov_deepseek: {
          id: "prov_deepseek",
          name: "DeepSeek",
          settingsConfig: {
            env: {
              ANTHROPIC_MODEL: "deepseek-chat",
              ANTHROPIC_DEFAULT_SONNET_MODEL: "deepseek-chat",
            },
          },
        },
        other: {
          id: "other",
          name: "Other",
          settingsConfig: { env: { ANTHROPIC_MODEL: "claude-sonnet-4" } },
        },
      },
      currentProviderId: "prov_deepseek",
    },
  }),
}));

describe("collectProviderModelCandidates", () => {
  it("collects env model fields and strips [1M]", () => {
    const provider = {
      id: "p",
      name: "P",
      settingsConfig: {
        env: {
          ANTHROPIC_MODEL: "foo[1M]",
          ANTHROPIC_DEFAULT_OPUS_MODEL: "bar",
        },
        modelCatalog: { models: [{ id: "catalog-model" }] },
      },
    } as unknown as Provider;
    const models = collectProviderModelCandidates(provider);
    expect(models).toEqual(
      expect.arrayContaining(["foo", "bar", "catalog-model"]),
    );
  });
});

describe("ClaudeModelRoutingPanel", () => {
  beforeEach(() => {
    getClaudeModelRouting.mockReset();
    setClaudeModelRouting.mockReset();
    getClaudeModelRouting.mockResolvedValue({
      enabled: false,
      replaceBuiltInOptions: false,
      writeAvailableModels: false,
      enableGatewayDiscovery: false,
      entries: [],
    });
    setClaudeModelRouting.mockResolvedValue(true);
  });

  it("defaults to disabled and can add/save a catalog entry", async () => {
    render(<ClaudeModelRoutingPanel />);

    await waitFor(() =>
      expect(
        screen.getByTestId("claude-model-routing-panel"),
      ).toBeInTheDocument(),
    );

    const enabled = screen.getByTestId("claude-model-routing-enabled");
    // Switch is a button role usually
    fireEvent.click(enabled);

    fireEvent.click(screen.getByTestId("claude-model-routing-add"));
    expect(screen.getByTestId("claude-model-routing-entry")).toBeInTheDocument();

    fireEvent.change(screen.getByTestId("claude-model-routing-provider"), {
      target: { value: "prov_deepseek" },
    });
    fireEvent.change(screen.getByTestId("claude-model-routing-upstream"), {
      target: { value: "deepseek-chat" },
    });

    await waitFor(() => {
      expect(screen.getByTestId("claude-model-routing-client")).toHaveValue(
        "prov_deepseek/deepseek-chat",
      );
    });

    fireEvent.click(screen.getByTestId("claude-model-routing-save"));

    await waitFor(() => expect(setClaudeModelRouting).toHaveBeenCalled());
    const payload = setClaudeModelRouting.mock.calls[0][0];
    expect(payload.enabled).toBe(true);
    expect(payload.replaceBuiltInOptions).toBe(false);
    expect(payload.enableGatewayDiscovery).toBe(false);
    expect(payload.entries).toHaveLength(1);
    expect(payload.entries[0]).toMatchObject({
      providerId: "prov_deepseek",
      upstreamModel: "deepseek-chat",
      clientModel: "prov_deepseek/deepseek-chat",
    });
  });

  it("loads existing catalog from settings API", async () => {
    getClaudeModelRouting.mockResolvedValue({
      enabled: true,
      replaceBuiltInOptions: false,
      writeAvailableModels: false,
      enableGatewayDiscovery: false,
      entries: [
        {
          clientModel: "other/claude-sonnet-4",
          providerId: "other",
          upstreamModel: "claude-sonnet-4",
          label: "Other Sonnet",
          description: "",
        },
      ],
    });

    render(<ClaudeModelRoutingPanel />);
    await waitFor(() =>
      expect(screen.getByTestId("claude-model-routing-client")).toHaveValue(
        "other/claude-sonnet-4",
      ),
    );
  });
});
