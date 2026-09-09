import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// happy-dom doesn't auto-remove the rendered DOM between tests, so
// a second `getByRole("button")` will match leftover buttons from a
// previous render. Clearing the document after every test keeps each
// case focused on its own render.
afterEach(() => {
  cleanup();
});
