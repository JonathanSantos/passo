// Lê os resultados do rodar.sh e imprime a tabela da simulação em Markdown, com as mesmas linhas, na mesma ordem,
// que a tabela do README principal: mediana de cada linha por modelo, com o intervalo quando as rodadas diferem.
// A tabela do README sai daqui; não a escreva à mão.
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

const STAGES = ["refinar", "planejar", "construir", "verificar", "entregar"];
const billable = (t) => (t && t.calls ? t.input + t.output + t.cache_write : undefined);
const numbers = (values) => values.filter((v) => typeof v === "number" && !Number.isNaN(v));

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

const plain = (n) => (Number.isInteger(n) ? String(n) : n.toFixed(1).replace(".", ","));
const kilo = (n) => `${(n / 1000).toFixed(1).replace(".", ",")}k`;
function duration(ms) {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}min ${s % 60}s`;
}

// Median, with the range in parentheses when the rounds differ; `—` when nothing was measured.
function stat(values, format = plain) {
  const v = numbers(values);
  if (v.length === 0) return "—";
  const [min, max] = [Math.min(...v), Math.max(...v)];
  return format(median(v)) + (min === max ? "" : ` (${format(min)}–${format(max)})`);
}

function proofs(runs) {
  const counts = {};
  for (const r of runs) {
    const proof = r.run?.proof;
    if (proof) counts[proof] = (counts[proof] ?? 0) + 1;
  }
  const names = { forte: "forte", media: "média", fraca: "fraca", nenhuma: "nenhuma" };
  const entries = Object.entries(counts).sort((a, b) => b[1] - a[1]);
  if (entries.length === 0) return "—";
  if (entries.length === 1) return names[entries[0][0]] ?? entries[0][0];
  return entries.map(([p, n]) => `${names[p] ?? p} (${n})`).join(" · ");
}

function steps(runs) {
  const main = stat(runs.map((r) => r.run?.steps));
  const fixes = numbers(runs.map((r) => r.run?.fixes));
  return fixes.some((f) => f > 0) ? `${main}, mais ${stat(fixes)} de ajuste` : main;
}

function toolsPerStep(r) {
  const per = Object.values(r.stats?.tools_per_step ?? {});
  return per.length ? { avg: per.reduce((a, b) => a + b, 0) / per.length, max: Math.max(...per) } : null;
}

// Exactly the README's rows, in its order; new rows go at the end.
export const ROWS = [
  ["**Suíte escondida**", (rs) => {
    const total = rs[0].suite.pass + rs[0].suite.fail;
    const passed = numbers(rs.map((r) => r.suite.pass));
    const [min, max] = [Math.min(...passed), Math.max(...passed)];
    const range = min === max ? "" : ` (${plain(min)}–${plain(max)})`;
    return `**${plain(median(passed))}/${total}${range}**`;
  }],
  ["Testes do projeto no fim", (rs) => stat(rs.map((r) => r.project_tests))],
  ["Grau de prova", proofs],
  ["Perguntas ao dev", (rs) => stat(rs.map((r) => r.run?.questions))],
  ["Vezes do dev", (rs) => stat(rs.map((r) => r.stats?.dev_turns))],
  ["Passos", steps],
  ["Tentativas que falharam · entregas inválidas", (rs) =>
    `${stat(rs.map((r) => r.stats?.steps_failed))} · ${stat(rs.map((r) => r.stats?.invalid_outputs))}`],
  ["Tempo total", (rs) => stat(rs.map((r) => r.stats?.total_ms), duration)],
  ["Tokens por etapa (refinar · planejar · construir · verificar · entregar)", (rs) =>
    STAGES.map((_, i) => stat(rs.map((r) => billable(r.stats?.stages?.[i]?.tokens)), kilo)).join(" · ")],
  ["Tokens no run · chamadas ao modelo", (rs) =>
    `${stat(rs.map((r) => billable(r.stats?.tokens)), kilo)} · ${stat(rs.map((r) => r.stats?.tokens?.calls || undefined))}`],
  ["Escaladas para o nível forte", (rs) => stat(rs.map((r) => r.stats?.escalations))],
  ["Ações por passo (média · máximo)", (rs) =>
    `${stat(rs.map((r) => toolsPerStep(r)?.avg))} · ${stat(rs.map((r) => toolsPerStep(r)?.max))}`],
];

const header = `| | ${models.map((m) => `${m.name} (n=${m.runs.length})`).join(" | ")} |`;
const rule = `| --- | ${models.map(() => "---").join(" | ")} |`;
const lines = ROWS.map(([label, cell]) => `| ${label} | ${models.map((m) => cell(m.runs)).join(" | ")} |`);

console.log([header, rule, ...lines].join("\n"));
console.log("\nMediana de cada linha, com o intervalo entre parênteses quando as rodadas diferem.");
