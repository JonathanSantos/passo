// Suíte escondida: só o que o pedido diz. Roda contra a entrega de cada braço.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const ROOT = process.env.ARM_DIR;
const run = (file, ...args) =>
  spawnSync("node", [join(ROOT, "src/cli.js"), ...args], { env: { ...process.env, TAREFAS_FILE: file }, encoding: "utf8" });
const fresh = () => join(mkdtempSync(join(tmpdir(), "aceite-")), "t.json");

test("add aceita --prioridade e list mostra a prioridade", () => {
  const f = fresh();
  assert.equal(run(f, "add", "pagar conta", "--prioridade", "alta").status, 0);
  const out = run(f, "list").stdout;
  assert.match(out, /pagar conta/);
  assert.match(out, /alta/i);
  assert.doesNotMatch(out, /--prioridade/, "a prioridade aparece como prioridade, não como parte do título");
});

test("sem --prioridade a tarefa fica com média", () => {
  const f = fresh();
  run(f, "add", "ler livro");
  assert.match(run(f, "list").stdout, /m[eé]dia/i);
});

test("list --prioridade filtra", () => {
  const f = fresh();
  run(f, "add", "urgente", "--prioridade", "alta");
  run(f, "add", "depois", "--prioridade", "baixa");
  const out = run(f, "list", "--prioridade", "alta").stdout;
  assert.match(out, /urgente/);
  assert.doesNotMatch(out, /depois/);
});

test("prioridade inválida dá erro e não grava", () => {
  const f = fresh();
  const r = run(f, "add", "x", "--prioridade", "urgentissima");
  assert.notEqual(r.status, 0);
  assert.doesNotMatch(run(f, "list").stdout, /\bx\b/);
});

test("o título não engole a flag", () => {
  const f = fresh();
  run(f, "add", "comprar pão", "--prioridade", "baixa");
  assert.doesNotMatch(run(f, "list").stdout, /--prioridade/);
});

test("tarefas antigas sem prioridade continuam listando", () => {
  const f = fresh();
  run(f, "add", "a");
  assert.equal(run(f, "list").status, 0);
});
