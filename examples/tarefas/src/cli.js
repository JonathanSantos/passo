#!/usr/bin/env node
// tarefas add "título" · tarefas list · tarefas done <id>
import { add, complete, list } from "./tasks.js";

const FILE = process.env.TAREFAS_FILE ?? "tarefas.json";

export function format(task) {
  return `${task.done ? "[x]" : "[ ]"} ${task.id}. ${task.title}`;
}

export function main(args, out = console.log) {
  const [command, ...rest] = args;
  switch (command) {
    case "add":
      out(format(add(FILE, rest.join(" "))));
      return 0;
    case "list":
      for (const task of list(FILE)) out(format(task));
      return 0;
    case "done":
      out(format(complete(FILE, rest[0])));
      return 0;
    default:
      out("uso: tarefas add \"título\" | list | done <id>");
      return 1;
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    console.error(`erro: ${error.message}`);
    process.exitCode = 1;
  }
}
