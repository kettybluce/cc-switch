import type { TFunction } from "i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { showFetchModelsError } from "@/lib/api/model-fetch";

const toastErrorMock = vi.fn();

vi.mock("sonner", () => ({
  toast: {
    error: (...args: unknown[]) => toastErrorMock(...args),
    info: vi.fn(),
    success: vi.fn(),
  },
}));

const t = ((key: string) => key) as TFunction;

describe("showFetchModelsError", () => {
  beforeEach(() => {
    toastErrorMock.mockReset();
  });

  it.each([
    [
      { hasApiKey: false, hasBaseUrl: false },
      "providerForm.fetchModelsNeedConfig",
    ],
    [
      { hasApiKey: false, hasBaseUrl: true },
      "providerForm.fetchModelsNeedApiKey",
    ],
    [
      { hasApiKey: true, hasBaseUrl: false },
      "providerForm.fetchModelsNeedEndpoint",
    ],
  ] as const)("maps missing-field prechecks %j", (opts, key) => {
    showFetchModelsError(null, t, opts);
    expect(toastErrorMock).toHaveBeenCalledWith(key);
  });

  it.each([
    ["HTTP 401 from upstream", "providerForm.fetchModelsAuthFailed"],
    ["HTTP 403 forbidden", "providerForm.fetchModelsAuthFailed"],
    [
      "All candidates failed with HTTP 404",
      "providerForm.fetchModelsEndpointNotFound",
    ],
    ["HTTP 404", "providerForm.fetchModelsEndpointNotFound"],
    ["HTTP 405", "providerForm.fetchModelsEndpointNotFound"],
    ["request timeout after 15s", "providerForm.fetchModelsTimeout"],
    ["connection timed out", "providerForm.fetchModelsTimeout"],
    ["Failed to parse models payload", "providerForm.fetchModelsNotSupported"],
    ["socket hang up", "providerForm.fetchModelsFailed"],
  ])("maps backend error %s", (message, key) => {
    showFetchModelsError(new Error(message), t);
    expect(toastErrorMock).toHaveBeenCalledWith(key);
  });
});
