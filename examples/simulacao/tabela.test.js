// The table tabela.js prints has the README's rows, in the README's order.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const labels = (table) =>
  table
    .split("\n")
    .filter((l) => l.startsWith("| ") && !l.startsWith("| --- ") && !l.startsWith("| |"))
    .map((l) => l.split(" | ")[0].slice(2).trim());

const result = (model, round, pass, steps) => ({
  model,
  round,
  state: "fim",
  suite: { pass, fail: 6 - pass },
  project_tests: 10 + round,
  run: { proof: "forte", questions: round, steps, fixes: round - 1 },
  stats: {
    dev_turns: 2,
    steps_failed: 0,
    invalid_outputs: round,
    escalations: 0,
    total_ms: 200000 + round * 1000,
    tokens: { input: 50, output: 300, cache_read: 1000000, cache_write: 50000, calls: 20 },
    stages: Array.from({ length: 5 }, () => ({ tokens: { input: 1, output: 10, cache_read: 0, cache_write: 5000, calls: 3 } })),
    tools_per_step: { S1: 10, S2: 20 },
  },
});

test("tabela.js prints the README's rows in the README's order", () => {
  const dir = mkdtempSync(join(tmpdir(), "passo-tabela-"));
  for (const model of ["haiku", "sonnet"]) {
    mkdirSync(join(dir, model));
    for (const round of [1, 2]) {
      writeFileSync(join(dir, model, `${round}.json`), JSON.stringify(result(model, round, 5 + (round % 2), 3)));
    }
  }
  const table = execFileSync("node", [join(here, "tabela.js"), dir], { encoding: "utf8" });
  const readme = readFileSync(join(here, "../../README.md"), "utf8");
  const start = readme.indexOf("| | Haiku 5.5");
  const block = readme.slice(start, readme.indexOf("\n\n", start));
  assert.deepEqual(labels(table), labels(block));
  assert.match(table, /\*\*5,5\/6 \(5–6\)\*\*/);
});
