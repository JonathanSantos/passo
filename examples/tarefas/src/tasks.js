// Tarefas guardadas num arquivo JSON. Cada função recebe o caminho do arquivo, para os testes usarem um temporário.
import { existsSync, readFileSync, writeFileSync } from "node:fs";

export function load(file) {
  if (!existsSync(file)) return [];
  return JSON.parse(readFileSync(file, "utf8"));
}

function save(file, tasks) {
  writeFileSync(file, JSON.stringify(tasks, null, 2) + "\n");
}

export function add(file, title) {
  const text = String(title ?? "").trim();
  if (!text) throw new Error("o título não pode ficar vazio");
  const tasks = load(file);
  const id = tasks.reduce((max, t) => Math.max(max, t.id), 0) + 1;
  const task = { id, title: text, done: false };
  tasks.push(task);
  save(file, tasks);
  return task;
}

export function list(file) {
  return load(file);
}

export function complete(file, id) {
  const tasks = load(file);
  const task = tasks.find((t) => t.id === Number(id));
  if (!task) throw new Error(`tarefa ${id} não existe`);
  task.done = true;
  save(file, tasks);
  return task;
}
