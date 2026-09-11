import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ProviderActions } from "@/components/providers/ProviderActions";

function renderPiActions({
  isCurrent = false,
  isInConfig = false,
  isRemovalProtected = false,
  isStateChangeProtected = false,
  isAutoFailoverEnabled = false,
  isInFailoverQueue = false,
  isDefaultModel = false,
  onSwitch = vi.fn(),
  onEdit = vi.fn(),
  onRemoveFromConfig = vi.fn(),
  onDelete = vi.fn(),
  onSetAsDefault = vi.fn(),
  onToggleFailover,
}: {
  isCurrent?: boolean;
  isInConfig?: boolean;
  isRemovalProtected?: boolean;
  isStateChangeProtected?: boolean;
  isAutoFailoverEnabled?: boolean;
  isInFailoverQueue?: boolean;
  isDefaultModel?: boolean;
  onSwitch?: ReturnType<typeof vi.fn>;
  onEdit?: ReturnType<typeof vi.fn>;
  onRemoveFromConfig?: ReturnType<typeof vi.fn>;
  onDelete?: ReturnType<typeof vi.fn>;
  onSetAsDefault?: ReturnType<typeof vi.fn>;
  onToggleFailover?: ReturnType<typeof vi.fn>;
}) {
  render(
    <ProviderActions
      appId="pi"
      isCurrent={isCurrent}
      isInConfig={isInConfig}
      isRemovalProtected={isRemovalProtected}
      isStateChangeProtected={isStateChangeProtected}
      isAutoFailoverEnabled={isAutoFailoverEnabled}
      isInFailoverQueue={isInFailoverQueue}
      isDefaultModel={isDefaultModel}
      onToggleFailover={onToggleFailover}
      onSwitch={onSwitch}
      onRemoveFromConfig={onRemoveFromConfig}
      onSetAsDefault={onSetAsDefault}
      onEdit={onEdit}
      onDuplicate={vi.fn()}
      onDelete={onDelete}
    />,
  );
  return { onSwitch, onEdit, onRemoveFromConfig, onDelete, onSetAsDefault };
}

describe("ProviderActions Pi provider switching", () => {
  it("omits duplication when the caller disallows it", () => {
    render(
      <ProviderActions
        appId="codex"
        isCurrent={false}
        onSwitch={vi.fn()}
        onEdit={vi.fn()}
        onDelete={vi.fn()}
      />,
    );

    expect(screen.queryByTitle("provider.duplicate")).not.toBeInTheDocument();
  });

  it("enables a provider that is not in Pi", async () => {
    const user = userEvent.setup();
    const { onSwitch } = renderPiActions({});

    await user.click(screen.getByRole("button", { name: "启用" }));

    expect(onSwitch).toHaveBeenCalledTimes(1);
    expect(
      screen.queryByRole("button", { name: "provider.setAsDefault" }),
    ).not.toBeInTheDocument();
  });

  it("offers set-as-default for an enabled Pi provider", async () => {
    const user = userEvent.setup();
    const { onSetAsDefault, onRemoveFromConfig } = renderPiActions({
      isInConfig: true,
      onSetAsDefault: vi.fn(),
    });

    await user.click(screen.getByRole("button", { name: "设为默认" }));

    expect(onSetAsDefault).toHaveBeenCalledTimes(1);
    expect(onRemoveFromConfig).not.toHaveBeenCalled();
  });

  it("shows the current Pi default without blocking remove or delete", () => {
    renderPiActions({
      isCurrent: true,
      isInConfig: true,
      isDefaultModel: true,
    });

    expect(screen.getByRole("button", { name: "移除" })).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "当前默认" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "common.delete" })).toBeEnabled();
  });

  it("fails closed while Pi's authoritative state is unavailable", async () => {
    const user = userEvent.setup();
    const { onSwitch, onEdit, onDelete } = renderPiActions({
      isStateChangeProtected: true,
    });

    const enableButton = screen.getByRole("button", { name: "启用" });
    const deleteButton = screen.getByRole("button", {
      name: "common.delete",
    });
    const editButton = screen.getByRole("button", { name: "common.edit" });
    expect(enableButton).toBeDisabled();
    expect(deleteButton).toBeDisabled();
    expect(editButton).toBeEnabled();

    await user.click(enableButton);
    await user.click(editButton);
    await user.click(deleteButton);
    expect(onSwitch).not.toHaveBeenCalled();
    expect(onEdit).toHaveBeenCalledTimes(1);
    expect(onDelete).not.toHaveBeenCalled();
  });

  it("keeps Pi in membership mode even if stale failover props are supplied", async () => {
    const user = userEvent.setup();
    const onToggleFailover = vi.fn();
    const { onSwitch } = renderPiActions({
      isAutoFailoverEnabled: true,
      isInFailoverQueue: false,
      onToggleFailover,
    });

    await user.click(screen.getByRole("button", { name: "启用" }));

    expect(onSwitch).toHaveBeenCalledTimes(1);
    expect(onToggleFailover).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: "failover.addQueue" }),
    ).not.toBeInTheDocument();
  });
});
