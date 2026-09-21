import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { http, HttpResponse } from "msw";

import { PiProviderForm } from "@/components/providers/forms/PiProviderForm";
import { server } from "../msw/server";

const TAURI_ENDPOINT = "http://tauri.local";

const toastInfoMock = vi.fn();
const toastSuccessMock = vi.fn();
const toastErrorMock = vi.fn();

vi.mock("sonner", () => ({
  toast: {
    info: (...args: unknown[]) => toastInfoMock(...args),
    success: (...args: unknown[]) => toastSuccessMock(...args),
    error: (...args: unknown[]) => toastErrorMock(...args),
  },
}));

vi.mock("@/components/JsonEditor", () => ({
  default: ({
    id,
    value,
    onChange,
    readOnly,
  }: {
    id?: string;
    value: string;
    onChange: (value: string) => void;
    readOnly?: boolean;
  }) => (
    <textarea
      id={id}
      value={value}
      readOnly={readOnly}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

function renderCustomForm(
  overrides: Partial<ComponentProps<typeof PiProviderForm>> = {},
) {
  const onSubmit = vi.fn().mockResolvedValue(undefined);
  const result = render(
    <PiProviderForm
      appId="pi"
      submitLabel="Save Pi provider"
      onSubmit={onSubmit}
      onCancel={() => {}}
      {...overrides}
    />,
  );
  if (!overrides.providerId) {
    fireEvent.click(
      screen.getByRole("button", { name: "providerPreset.custom" }),
    );
  }
  return { ...result, onSubmit };
}

describe("PiProviderForm openai-completions compat and fetch-models", () => {
  beforeEach(() => {
    Element.prototype.scrollIntoView = vi.fn();
    toastInfoMock.mockReset();
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
  });

  it("defaults a custom provider to openai-completions without inventing supportsDeveloperRole", () => {
    renderCustomForm();

    expect(document.querySelector("#pi-provider-api-select")).toHaveTextContent(
      "OpenAI Chat Completions",
    );
    const preview = JSON.parse(
      (screen.getByLabelText("provider.configJson") as HTMLTextAreaElement)
        .value,
    );
    expect(preview.api).toBe("openai-completions");
    expect(preview).not.toHaveProperty("compat");
    expect(preview).not.toHaveProperty("supportsDeveloperRole");
  });

  it("hints supportsDeveloperRole false as the openai-completions compatibility placeholders", async () => {
    renderCustomForm();

    fireEvent.click(
      screen.getByRole("button", { name: "pi.form.addCompatibilityOption" }),
    );

    expect(
      screen.getByPlaceholderText("supportsDeveloperRole"),
    ).toBeInTheDocument();
    expect(screen.getByPlaceholderText("false")).toBeInTheDocument();
    expect(
      JSON.parse(
        (screen.getByLabelText("provider.configJson") as HTMLTextAreaElement)
          .value,
      ),
    ).not.toHaveProperty("compat");
  });

  it("does not pin supportsDeveloperRole when the user switches to openai-completions", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(
      <PiProviderForm
        appId="pi"
        providerId="anthropic-card"
        submitLabel="Save switched API"
        onSubmit={onSubmit}
        onCancel={() => {}}
        initialData={{
          name: "Anthropic card",
          settingsConfig: {
            name: "Anthropic card",
            api: "anthropic-messages",
            baseUrl: "https://api.example.com",
            models: [
              {
                id: "model",
                name: "Model",
                reasoning: false,
                input: ["text"],
                contextWindow: 128_000,
                maxTokens: 16_384,
              },
            ],
          },
        }}
      />,
    );

    await user.click(document.querySelector("#pi-provider-api-select")!);
    await user.click(
      await screen.findByRole("option", { name: "OpenAI Chat Completions" }),
    );

    const preview = JSON.parse(
      (screen.getByLabelText("provider.configJson") as HTMLTextAreaElement)
        .value,
    );
    expect(preview.api).toBe("openai-completions");
    expect(preview).not.toHaveProperty("compat");

    fireEvent.click(screen.getByRole("button", { name: "Save switched API" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(JSON.parse(onSubmit.mock.calls[0][0].settingsConfig)).not.toHaveProperty(
      "compat",
    );
  }, 15_000);

  it("preserves an explicit openai-completions supportsDeveloperRole true instead of forcing false", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const input = {
      name: "Explicit true",
      api: "openai-completions",
      baseUrl: "https://api.example.com/v1",
      compat: { supportsDeveloperRole: true },
      models: [
        {
          id: "model",
          name: "Model",
          reasoning: false,
          input: ["text"],
          contextWindow: 128_000,
          maxTokens: 16_384,
        },
      ],
    };

    render(
      <PiProviderForm
        appId="pi"
        providerId="explicit-true"
        submitLabel="Save explicit true"
        onSubmit={onSubmit}
        onCancel={() => {}}
        initialData={{ name: input.name, settingsConfig: input }}
      />,
    );

    expect(
      screen.getByLabelText("pi.form.optionKey"),
    ).toHaveValue("supportsDeveloperRole");
    expect(screen.getByLabelText("pi.form.optionValue")).toHaveValue("true");

    fireEvent.click(
      screen.getByRole("button", { name: "Save explicit true" }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(JSON.parse(onSubmit.mock.calls[0][0].settingsConfig)).toEqual(input);
  });

  it("round-trips an explicit supportsDeveloperRole false default on openai-completions", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const input = {
      name: "Explicit false",
      api: "openai-completions",
      baseUrl: "https://api.example.com/v1",
      compat: { supportsDeveloperRole: false },
      models: [
        {
          id: "model",
          name: "Model",
          reasoning: false,
          input: ["text"],
          contextWindow: 128_000,
          maxTokens: 16_384,
        },
      ],
    };

    render(
      <PiProviderForm
        appId="pi"
        providerId="explicit-false"
        submitLabel="Save explicit false"
        onSubmit={onSubmit}
        onCancel={() => {}}
        initialData={{ name: input.name, settingsConfig: input }}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Save explicit false" }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(JSON.parse(onSubmit.mock.calls[0][0].settingsConfig).compat).toEqual(
      { supportsDeveloperRole: false },
    );
  });

  it("asks for endpoint and credentials before calling fetch-models", () => {
    renderCustomForm();

    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );

    expect(toastErrorMock).toHaveBeenCalledWith(
      "providerForm.fetchModelsNeedConfig",
    );
  });

  it("treats request headers as credentials when the API key is empty", async () => {
    let requestBody: Record<string, unknown> | undefined;
    server.use(
      http.post(
        `${TAURI_ENDPOINT}/fetch_models_for_config`,
        async ({ request }) => {
          requestBody = (await request.json()) as Record<string, unknown>;
          return HttpResponse.json([{ id: "header-model" }]);
        },
      ),
    );

    renderCustomForm();
    fireEvent.change(
      screen.getByPlaceholderText("https://api.example.com/v1"),
      { target: { value: "https://headers.example/v1" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Add header" }));
    const headerName = screen.getByLabelText("Header");
    fireEvent.change(headerName, { target: { value: "X-Title" } });
    fireEvent.blur(headerName);
    fireEvent.change(screen.getByLabelText("Value"), {
      target: { value: "pi-ui" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );

    await waitFor(() =>
      expect(requestBody).toEqual({
        baseUrl: "https://headers.example/v1",
        apiKey: "",
        isFullUrl: false,
        requestHeaders: { "X-Title": "pi-ui" },
      }),
    );
    expect(requestBody?.apiFormat).toBeUndefined();
    await waitFor(() =>
      expect(toastSuccessMock).toHaveBeenCalledWith(
        "providerForm.fetchModelsSuccess",
      ),
    );
  });

  it("surfaces an empty fetch-models result without inventing a dropdown", async () => {
    server.use(
      http.post(`${TAURI_ENDPOINT}/fetch_models_for_config`, () =>
        HttpResponse.json([]),
      ),
    );

    renderCustomForm();
    fireEvent.change(screen.getByLabelText("pi.form.credential"), {
      target: { value: "literal-key" },
    });
    fireEvent.change(
      screen.getByPlaceholderText("https://api.example.com/v1"),
      { target: { value: "https://empty.example/v1" } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );

    await waitFor(() =>
      expect(toastInfoMock).toHaveBeenCalledWith(
        "providerForm.fetchModelsEmpty",
      ),
    );
    expect(screen.queryByRole("button", { name: "Select model" })).not.toBeInTheDocument();
  });

  it("maps fetch-models HTTP 401 to the shared auth-failed toast", async () => {
    server.use(
      http.post(`${TAURI_ENDPOINT}/fetch_models_for_config`, () =>
        HttpResponse.text("HTTP 401", { status: 500 }),
      ),
    );

    renderCustomForm();
    fireEvent.change(screen.getByLabelText("pi.form.credential"), {
      target: { value: "bad-key" },
    });
    fireEvent.change(
      screen.getByPlaceholderText("https://api.example.com/v1"),
      { target: { value: "https://auth.example/v1" } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );

    await waitFor(() =>
      expect(toastErrorMock).toHaveBeenCalledWith(
        "providerForm.fetchModelsAuthFailed",
      ),
    );
  });

  it("disables fetch-models while a request is in flight", async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    server.use(
      http.post(`${TAURI_ENDPOINT}/fetch_models_for_config`, async () => {
        await gate;
        return HttpResponse.json([{ id: "slow-model" }]);
      }),
    );

    renderCustomForm();
    fireEvent.change(screen.getByLabelText("pi.form.credential"), {
      target: { value: "literal-key" },
    });
    fireEvent.change(
      screen.getByPlaceholderText("https://api.example.com/v1"),
      { target: { value: "https://slow.example/v1" } },
    );
    const fetchButton = screen.getByRole("button", {
      name: "providerForm.fetchModels",
    });
    fireEvent.click(fetchButton);
    await waitFor(() => expect(fetchButton).toBeDisabled());

    release?.();
    await waitFor(() => expect(fetchButton).toBeEnabled());
    await waitFor(() =>
      expect(toastSuccessMock).toHaveBeenCalledWith(
        "providerForm.fetchModelsSuccess",
      ),
    );
  });

  it("does not send the Pi api field when fetching models after switching format", async () => {
    const user = userEvent.setup();
    let requestBody: Record<string, unknown> | undefined;
    server.use(
      http.post(
        `${TAURI_ENDPOINT}/fetch_models_for_config`,
        async ({ request }) => {
          requestBody = (await request.json()) as Record<string, unknown>;
          return HttpResponse.json([{ id: "anthropic-model" }]);
        },
      ),
    );

    renderCustomForm();
    await user.click(document.querySelector("#pi-provider-api-select")!);
    await user.click(
      await screen.findByRole("option", { name: "Anthropic Messages" }),
    );
    fireEvent.change(screen.getByLabelText("pi.form.credential"), {
      target: { value: "literal-key" },
    });
    fireEvent.change(
      screen.getByPlaceholderText("https://api.example.com/v1"),
      { target: { value: "https://api.anthropic.example" } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );

    await waitFor(() =>
      expect(requestBody).toMatchObject({
        baseUrl: "https://api.anthropic.example",
        apiKey: "literal-key",
        isFullUrl: false,
      }),
    );
    expect(requestBody?.apiFormat).toBeUndefined();
    expect(requestBody).not.toHaveProperty("api");
  }, 15_000);
});
