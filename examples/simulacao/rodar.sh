#!/bin/sh
# Simulação repetível do Passo: N rodadas de um modelo no projeto de exemplo, com o dev feito por uma regra fixa
# e cada entrega julgada pela suíte escondida.
#
#   sh examples/simulacao/rodar.sh <modelo> <rodadas>      ex.: sh examples/simulacao/rodar.sh haiku 3
#
# Precisa de: claude (Claude Code), passo, git e node no PATH. Gasta tokens da sua conta.
# Este é o único lugar a ajustar quando a CLI do agente mudar: os flags ficam aqui em cima.
set -eu

CLAUDE="${CLAUDE:-claude}"
CLAUDE_FLAGS="--setting-sources project --strict-mcp-config --permission-mode bypassPermissions --max-budget-usd ${BUDGET_USD:-3}"
PASSO="${PASSO:-passo}"
MAX_TURNS="${MAX_TURNS:-12}"

model="${1:?uso: rodar.sh <modelo> <rodadas>}"
rounds="${2:?uso: rodar.sh <modelo> <rodadas>}"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$here/resultados/$model"
mkdir -p "$out"

PROMPT='O dev pediu, com o passo: "Adicionar prioridade às tarefas: alta, média ou baixa. A prioridade é definida ao criar com `tarefas add "título" --prioridade alta` (sem a flag fica média), aparece na listagem, e `tarefas list --prioridade alta` mostra só as tarefas daquela prioridade."

Este projeto usa o Passo. Leia a skill em .claude/skills/passo/SKILL.md e siga-a à risca. Comece com `passo start` usando o pedido do dev e `--size m`. Quando a saída do Passo disser que é a vez do dev, pare e copie o que o Passo mostrou. Não rode ok, no, change, size nem stop: são do dev.'

CONTINUE='O dev respondeu. Rode passo next e siga a saída; pare de novo na vez do dev ou quando o run terminar.'

# O estado do run, para a regra do dev: fim | interrompido | agente | aprovar | pergunta | limite | travado |
# aceitar-ok | aceitar-no:<primeiro critério reprovado>
state() {
  node -e '
    const fs = require("fs");
    let run;
    try { run = JSON.parse(fs.readFileSync(".passo/runs/" + fs.readFileSync(".passo/current", "utf8").trim() + "/run.json", "utf8")); }
    catch { console.log("agente"); process.exit(0); }
    if (run.status === "done") { console.log("fim"); process.exit(0); }
    if (run.status === "aborted") { console.log("interrompido"); process.exit(0); }
    if (!run.gate) { console.log("agente"); process.exit(0); }
    if (run.gate.kind !== "aceitar") { console.log(run.gate.kind); process.exit(0); }
    const checks = (run.checks || []).every((c) => c.ok);
    const failed = (run.criteria || []).find((c) => c.ok === false);
    console.log(checks && !failed ? "aceitar-ok" : "aceitar-no:" + (failed ? failed.id + " " + failed.text : "as checagens finais falharam"));
  '
}

dev() { PASSO_DEV=1 "$PASSO" "$@" >/dev/null 2>&1 || true; }

i=1
while [ "$i" -le "$rounds" ]; do
  work="$(mktemp -d)"
  dir="$work/tarefas"
  cp -R "$repo/examples/tarefas" "$dir"
  cd "$dir"
  git init -q -b main
  git config user.email sim@passo.dev
  git config user.name simulacao
  git add -A
  git commit -qm "projeto inicial"
  PASSO_DEV=1 "$PASSO" init --agent claude >/dev/null
  cat > .passo/passo.yaml <<'YAML'
checks:
  step: [npm run -s check, npm test --silent]
  final: [npm test --silent]
YAML
  cat > .passo/context.md <<'MD'
# Convenções do projeto

- JavaScript ESM, Node 22, sem dependências.
- Lógica em src/tasks.js; a CLI (src/cli.js) só traduz argumentos e imprime.
- Testes com node:test em test/*.test.js; cada teste usa um arquivo temporário.
- Mensagens para o usuário em português.
MD
  git add -A
  git commit -qm "passo"

  echo "rodada $i de $rounds ($model) em $dir"
  # shellcheck disable=SC2086
  "$CLAUDE" -p "$PROMPT" --model "$model" $CLAUDE_FLAGS > "$out/$i.agente.log" 2>&1 || true
  stuck=0
  turn=1
  while [ "$turn" -le "$MAX_TURNS" ]; do
    s="$(state)"
    case "$s" in
      fim | interrompido) break ;;
      # limite → passo ok: the size was the dev's choice, so the rule keeps it (a decision, see the README).
      aprovar | pergunta | limite | aceitar-ok) dev ok ;;
      aceitar-no:*) dev no "${s#aceitar-no:}" ;;
      travado)
        if [ "$stuck" -eq 0 ]; then
          stuck=1
          dev ok
        else
          dev stop "travou duas vezes"
          break
        fi
        ;;
      agente) ;; # o agente parou na vez dele: só manda continuar
    esac
    turn=$((turn + 1))
    # shellcheck disable=SC2086
    "$CLAUDE" -p --continue "$CONTINUE" --model "$model" $CLAUDE_FLAGS >> "$out/$i.agente.log" 2>&1 || true
  done

  suite="$(ARM_DIR="$dir" node --test "$here/aceite.test.js" 2>&1 || true)"
  pass="$(printf '%s\n' "$suite" | sed -n 's/^# pass //p' | tail -1)"
  fail="$(printf '%s\n' "$suite" | sed -n 's/^# fail //p' | tail -1)"
  project="$(npm test --silent 2>&1 || true)"
  project_pass="$(printf '%s\n' "$project" | sed -n 's/^# pass //p' | tail -1)"
  stats="$("$PASSO" stats --json 2>/dev/null || echo null)"
  final="$(state)"
  MODEL="$model" ROUND="$i" STATE="$final" PASS="${pass:-0}" FAIL="${fail:-0}" PROJECT_PASS="${project_pass:-0}" STATS="$stats" node -e '
    const fs = require("fs");
    const e = process.env;
    let stats = null;
    try { stats = JSON.parse(e.STATS); } catch {}
    let run = null;
    try { run = JSON.parse(fs.readFileSync(".passo/runs/" + fs.readFileSync(".passo/current", "utf8").trim() + "/run.json", "utf8")); } catch {}
    const steps = run ? run.steps.filter((s) => s.status === "done") : [];
    const result = {
      model: e.MODEL,
      round: Number(e.ROUND),
      state: e.STATE,
      suite: { pass: Number(e.PASS), fail: Number(e.FAIL) },
      project_tests: Number(e.PROJECT_PASS),
      run: run && {
        proof: run.proof ?? null,
        questions: (run.questions ?? []).length,
        steps: steps.filter((s) => !s.fix).length,
        fixes: steps.filter((s) => s.fix).length,
      },
      stats,
    };
    console.log(JSON.stringify(result, null, 2));
  ' > "$out/$i.json"
  echo "  suíte escondida: ${pass:-0} passaram, ${fail:-0} falharam · estado: $final"
  cd "$repo"
  if [ "${KEEP:-0}" = "1" ]; then
    echo "  pasta mantida (KEEP=1): $dir"
  else
    rm -rf "$work"
  fi
  i=$((i + 1))
done

echo "resultados em $out; a tabela: node $here/tabela.js"
