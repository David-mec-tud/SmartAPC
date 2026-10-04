// User-level form behavior: optional values stay absent and typed boolean fields
// do not become numeric tuning values. These are contracts, not CSS snapshots.
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { NumberInput } from "./ui";
afterEach(cleanup);
describe("optional engineering numeric values", () => {
  it("preserves blank optional offsets and accepts a signed fractional offset", async () => {
    function Form() {
      const [value, setValue] = useState<number | null>(null);
      return (
        <>
          <NumberInput
            aria-label="Band offset"
            optional
            value={value}
            onChange={setValue}
          />
          <output>{value === null ? "absent" : String(value)}</output>
        </>
      );
    }
    const user = userEvent.setup();
    render(<Form />);
    expect(screen.getByRole("status").textContent).toBe("absent");
    await user.type(screen.getByLabelText("Band offset"), "-2.5");
    expect(screen.getByRole("status").textContent).toBe("-2.5");
    await user.clear(screen.getByLabelText("Band offset"));
    expect(screen.getByRole("status").textContent).toBe("absent");
  });
});
