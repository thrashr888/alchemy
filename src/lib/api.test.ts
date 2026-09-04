import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
  isTauri: () => false,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

import { api } from "./api";

describe("IPC policy", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("retries an idempotent read twice with exponential backoff", async () => {
    invokeMock
      .mockRejectedValueOnce("first failure")
      .mockRejectedValueOnce("second failure")
      .mockResolvedValueOnce([]);

    const pending = api.listNotebooks();
    await vi.advanceTimersByTimeAsync(0);
    expect(invokeMock).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(300);
    expect(invokeMock).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(600);
    await expect(pending).resolves.toEqual([]);
    expect(invokeMock).toHaveBeenCalledTimes(3);
  });

  it("never retries a mutation", async () => {
    invokeMock.mockRejectedValueOnce("write failed");

    await expect(api.createNotebook("Test")).rejects.toThrow("write failed");
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("does not retry a timed-out read", async () => {
    invokeMock.mockReturnValueOnce(new Promise(() => {}));

    const pending = api.listNotebooks();
    const rejected = expect(pending).rejects.toThrow(
      "The request timed out. It may just be busy — try again.",
    );
    await vi.advanceTimersByTimeAsync(30_000);
    await rejected;
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});
