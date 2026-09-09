import { describe, expect, it } from "vitest";
import { isProxyOffDetail } from "@/lib/query/pi";

describe("isProxyOffDetail", () => {
  it("maps Windows /ping connection refused to start-the-proxy", () => {
    expect(
      isProxyOffDetail('Get "http://127.0.0.1:15721/ping": connection refused'),
    ).toBe(true);
  });

  it("maps the backend sentinel and leaves provider 401s alone", () => {
    expect(isProxyOffDetail("please start the local proxy")).toBe(true);
    expect(isProxyOffDetail("the CC Switch local proxy is not running")).toBe(
      true,
    );
    expect(isProxyOffDetail("HTTP 401 from the upstream provider")).toBe(false);
  });
});
