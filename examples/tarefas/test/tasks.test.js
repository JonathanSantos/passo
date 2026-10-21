import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { add, complete, list } from "../src/tasks.js";
import { format } from "../src/cli.js";

const file = () => join(mkdtempSync(join(tmpdir(), "tarefas-")), "t.json");

test("add cria tarefas com ids crescentes", () => {
  const f = file();
  assert.equal(add(f, "comprar pão").id, 1);
  assert.equal(add(f, "pagar conta").id, 2);
  assert.equal(list(f).length, 2);
});

test("add recusa título vazio", () => {
  assert.throws(() => add(file(), "  "), /vazio/);
});

test("complete marca como feita", () => {
  const f = file();
  add(f, "x");
  assert.equal(complete(f, 1).done, true);
  assert.throws(() => complete(f, 9), /não existe/);
});

test("format mostra o estado", () => {
  assert.equal(format({ id: 3, title: "ler", done: true }), "[x] 3. ler");
});
