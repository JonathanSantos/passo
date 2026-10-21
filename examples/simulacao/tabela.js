// Lê os resultados do rodar.sh e imprime a tabela da simulação em Markdown: mediana e intervalo de cada linha, por
// modelo. A tabela do README principal sai daqui; não a escreva à mão.
//
//   node examples/simulacao/tabela.js [pasta de resultados]
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = process.argv[2] ?? join(fileURLToPath(new URL(".", import.meta.url)), "resultados");
if (!existsSync(dir)) {
  console.error(`sem resultados em ${dir}: rode antes sh examples/simulacao/rodar.sh <modelo> <rodadas>`);
  process.exit(1);
}

const models = readdirSync(dir, { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => ({
    name: d.name,
    runs: readdirSync(join(dir, d.name))
      .filter((f) => f.endsWith(".json"))
      .map((f) => JSON.parse(readFileSync(join(dir, d.name, f), "utf8"))),
  }))
  .filter((m) => m.runs.length > 0);

if (models.length === 0) {
  console.error(`nenhum resultado em ${dir}`);
  process.exit(1);
}

const billable = (t) => (t ? t.input + t.output + t.cache_write : 0);
const ROWS = [
  ["Suíte escondida (testes que passaram)", (r) => r.suite.pass, (r) => `/${r.suite.pass + r.suite.fail}`],
  ["Runs concluídos", null],
  ["Vezes do dev", (r) => r.stats?.dev_turns],
  ["Tentativas que falharam", (r) => r.stats?.steps_failed],
  ["Entregas inválidas", (r) => r.stats?.invalid_outputs],
  ["Escaladas para o nível forte", (r) => r.stats?.escalations],
  ["Ações do agente", (r) => r.stats?.tool_calls],
  ["Tempo total (min)", (r) => (r.stats ? r.stats.total_ms / 60000 : undefined)],
  ["Tokens faturáveis (mil)", (r) => (r.stats?.tokens?.calls ? billable(r.stats.tokens) / 1000 : undefined)],
];

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

const fmt = (n) => (Number.isInteger(n) ? String(n) : n.toFixed(1).replace(".", ","));

function cell(runs, pick, suffix) {
  const values = runs.map(pick).filter((v) => typeof v === "number" && !Number.isNaN(v));
  if (values.length === 0) return "—";
  const min = Math.min(...values);
  const max = Math.max(...values);
  const tail = suffix ? suffix(runs[0]) : "";
  const range = min === max ? "" : ` (${fmt(min)}–${fmt(max)})`;
  return `${fmt(median(values))}${tail}${range}`;
}

const header = `| | ${models.map((m) => `${m.name} (n=${m.runs.length})`).join(" | ")} |`;
const rule = `| --- | ${models.map(() => "---").join(" | ")} |`;
const lines = ROWS.map(([label, pick, suffix]) => {
  const cells = models.map((m) =>
    pick ? cell(m.runs, pick, suffix) : `${m.runs.filter((r) => r.estado === "fim").length}/${m.runs.length}`,
  );
  return `| ${label} | ${cells.join(" | ")} |`;
});

console.log([header, rule, ...lines].join("\n"));
console.log("\nMediana de cada linha, com o intervalo entre parênteses quando as rodadas diferem.");
