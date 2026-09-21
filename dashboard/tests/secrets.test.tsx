import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import {
  assertSecretReferenceSafe,
  redactSecretsFromText,
  renderSecretReference,
} from "../src/lib/secrets";
import { SecretList } from "../src/components/SecretList";

describe("secret references never carry values", () => {
  it("renders references with version and scope, without values", () => {
    const text = renderSecretReference({
      ref_id: "db-url",
      version_hint: "7",
      environment: "production",
      rotation_state: "current",
    });
    expect(text).toContain("db-url");
    expect(text).toContain("production");
    expect(text).not.toMatch(/postgres:\/\/|hunter2/i);
  });

  it("rejects any value-shaped field before it reaches UI state", () => {
    expect(() =>
      assertSecretReferenceSafe({ ref_id: "x", value: "super-secret" }),
    ).toThrow(/must not enter/);
    expect(() =>
      assertSecretReferenceSafe({ ref_id: "x", plaintext: "super-secret" }),
    ).toThrow(/must not enter/);
  });

  it("redacts known material from free text", () => {
    expect(redactSecretsFromText("conn postgres://secret-db", ["postgres://secret-db"])).toBe(
      "conn [redacted]",
    );
  });

  it("secret list shows references only", () => {
    const { container } = render(
      <SecretList
        secrets={[
          { ref_id: "db-url", version_hint: "7", environment: "production", rotation_state: "current" },
        ]}
      />,
    );
    expect(screen.getByLabelText(/secret references/i)).toBeInTheDocument();
    expect(container.textContent ?? "").not.toMatch(/hunter2|postgres:\/\//i);
  });
});
