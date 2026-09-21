import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FetchedModelPicker } from "@/components/providers/forms/FetchedModelPicker";

const MODELS = [
  { id: "kimi-k2", ownedBy: "moonshot" },
  { id: "model-a", ownedBy: "vendor-a" },
  { id: "model-b", ownedBy: "vendor-b" },
  { id: "glm-5", ownedBy: null },
];

describe("FetchedModelPicker", () => {
  it("lists fetched models and keeps Add selected disabled until a new model is checked", () => {
    const onAdd = vi.fn();
    render(
      <FetchedModelPicker
        models={MODELS}
        configuredModelIds={["kimi-k2"]}
        onAdd={onAdd}
      />,
    );

    expect(
      screen.getByText("Available models (4)", { selector: "legend" }),
    ).toBeInTheDocument();
    const alreadyAdded = screen.getByRole("checkbox", { name: "kimi-k2" });
    expect(alreadyAdded).toBeChecked();
    expect(alreadyAdded).toBeDisabled();
    expect(screen.getByText("Already added")).toBeInTheDocument();
    expect(screen.getByText("moonshot")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Add selected (0)" }),
    ).toBeDisabled();

    fireEvent.click(alreadyAdded);
    expect(onAdd).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Add selected (0)" }),
    ).toBeDisabled();
  });

  it("filters by model id and vendor, including case-insensitive ownedBy matches", () => {
    render(
      <FetchedModelPicker
        models={MODELS}
        configuredModelIds={[]}
        onAdd={vi.fn()}
      />,
    );

    const search = screen.getByRole("textbox", { name: "Search models..." });
    fireEvent.change(search, { target: { value: "VENDOR-A" } });
    expect(screen.getByRole("checkbox", { name: "model-a" })).toBeInTheDocument();
    expect(
      screen.queryByRole("checkbox", { name: "model-b" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("checkbox", { name: "kimi-k2" }),
    ).not.toBeInTheDocument();

    fireEvent.change(search, { target: { value: "  glm  " } });
    expect(screen.getByRole("checkbox", { name: "glm-5" })).toBeInTheDocument();
    expect(
      screen.queryByRole("checkbox", { name: "model-a" }),
    ).not.toBeInTheDocument();

    fireEvent.change(search, { target: { value: "no-such-model" } });
    expect(screen.getByText("No matching models.")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("does not submit a surrounding form when Enter is pressed in the search box", () => {
    const onSubmit = vi.fn((event: { preventDefault: () => void }) =>
      event.preventDefault(),
    );
    const onAdd = vi.fn();
    render(
      <form onSubmit={onSubmit}>
        <FetchedModelPicker
          models={MODELS}
          configuredModelIds={[]}
          onAdd={onAdd}
        />
        <button type="submit">save</button>
      </form>,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "model-a" }));
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Search models..." }), {
      key: "Enter",
    });

    expect(onSubmit).not.toHaveBeenCalled();
    expect(onAdd).not.toHaveBeenCalled();
    expect(screen.getByRole("checkbox", { name: "model-a" })).toBeChecked();
  });

  it("batch-adds only currently selected unconfigured models and then clears the selection", () => {
    const onAdd = vi.fn();
    render(
      <FetchedModelPicker
        models={MODELS}
        configuredModelIds={["kimi-k2"]}
        onAdd={onAdd}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "model-a" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "model-b" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "glm-5" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "model-b" }));
    expect(
      screen.getByRole("button", { name: "Add selected (2)" }),
    ).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Add selected (2)" }));

    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd).toHaveBeenCalledWith(["model-a", "glm-5"]);
    expect(screen.getByRole("checkbox", { name: "model-a" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "glm-5" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "kimi-k2" })).toBeChecked();
    expect(
      screen.getByRole("button", { name: "Add selected (0)" }),
    ).toBeDisabled();
  });

  it("keeps selections that are hidden by search when adding the visible subset", () => {
    const onAdd = vi.fn();
    render(
      <FetchedModelPicker
        models={MODELS}
        configuredModelIds={[]}
        onAdd={onAdd}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "model-a" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "model-b" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search models..." }), {
      target: { value: "model-a" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add selected (2)" }));

    expect(onAdd).toHaveBeenCalledWith(["model-a", "model-b"]);
  });
});
