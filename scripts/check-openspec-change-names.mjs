import { readdir } from "node:fs/promises";
import { join } from "node:path";

const root = join(process.cwd(), "openspec", "changes");
const valid = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/;
const entries = await readdir(root, { withFileTypes: true }).catch(() => []);
const invalid = entries
  .filter((entry) => entry.isDirectory() && !entry.name.startsWith("."))
  .filter((entry) => entry.name !== "archive" && !valid.test(entry.name))
  .map((entry) => entry.name);

if (invalid.length > 0) {
  console.error(`Invalid active OpenSpec change name(s): ${invalid.join(", ")}`);
  process.exit(1);
}

console.log("OpenSpec active change names are valid.");
