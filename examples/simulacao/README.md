# Simulação

Como medir o Passo com modelos baratos: o mesmo pedido, um agente por braço, uma suíte de aceite que nenhum agente vê.

## Rodar

```bash
sh examples/simulacao/rodar.sh haiku 3
sh examples/simulacao/rodar.sh sonnet 3
node examples/simulacao/tabela.js
```

`rodar.sh <modelo> <rodadas>` faz o protocolo abaixo sozinho, uma rodada por vez: copia o projeto de exemplo para uma pasta temporária, faz `git init`, `passo init --agent claude`, escreve o `passo.yaml` e o `context.md`, e roda o agente sem interação (`claude -p`). O `claude -p` termina quando o agente para na vez do dev; então o script aplica a regra do dev e retoma a mesma sessão (`claude -p --continue`), até o run terminar. No fim, roda a suíte escondida e guarda o resultado e o `passo stats --json` em `resultados/<modelo>/<rodada>.json`.

A regra do dev, fixa:

| Gate                          | O dev responde                                                       |
| ----------------------------- | -------------------------------------------------------------------- |
| `aprovar`, `pergunta`, `limite` | `passo ok`                                                         |
| `aceitar`                     | `passo ok` se as checagens passaram e nenhum critério foi reprovado; senão `passo no "<primeiro critério reprovado>"` |
| `travado`                     | `passo ok` na primeira vez; `passo stop` na segunda                  |

Decisão: no gate `limite` (o diff passou do que o tamanho permite) o dev segue (`passo ok`), porque o tamanho foi escolhido no começo e a regra não o muda no meio.

Os comandos do dev saem com `PASSO_DEV=1`, porque o script pode rodar dentro do terminal de um agente. Com os hooks ativos (`--setting-sources project`), os tokens entram em `passo stats` sem nada à mão.

`tabela.js` lê `resultados/` e imprime a tabela em Markdown: a mediana de cada linha, com o intervalo entre parênteses. **A tabela da simulação no README principal sai daqui, nunca escrita à mão.**

O script é melhor esforço: gasta tokens da sua conta (`BUDGET_USD`, padrão 3 por chamada) e é o único lugar a ajustar quando a CLI do agente mudar (os flags ficam numa variável no topo). Variáveis: `CLAUDE`, `PASSO`, `BUDGET_USD`, `MAX_TURNS` e `KEEP=1`, que mantém a pasta temporária de cada rodada (por padrão ela é apagada no fim). Além do `passo stats --json`, cada resultado guarda a suíte escondida, os testes do projeto no fim e, do `run.json`, a prova, as perguntas e os passos.

## Protocolo

1. Copie [`examples/tarefas`](../tarefas) para uma pasta fora deste repositório, uma por braço, e faça `git init` e o primeiro commit em cada uma.
2. Em cada cópia: `passo init --agent claude`, ajuste as checagens em `.passo/passo.yaml`, escreva `.passo/context.md` e commite:

   ```yaml
   checks:
     step: [npm run -s check, npm test --silent]
     final: [npm test --silent]
   ```

3. Dê a cada agente o mesmo prompt: o pedido abaixo, a instrução de ler `.claude/skills/passo/SKILL.md`, `--size m`, e parar na vez do dev.

   > Adicionar prioridade às tarefas: alta, média ou baixa. A prioridade é definida ao criar com `tarefas add "título" --prioridade alta` (sem a flag fica média), aparece na listagem, e `tarefas list --prioridade alta` mostra só as tarefas daquela prioridade.

4. Faça o dev com uma regra fixa: aprove com as recomendadas; aceite a entrega quando as checagens passarem e o revisor não apontar um problema real; caso contrário, recuse com o problema em uma frase.
5. Julgue cada entrega com a suíte escondida (ela deve falhar no projeto original e passar numa entrega correta):

   ```bash
   ARM_DIR=/caminho/da/copia node --test examples/simulacao/aceite.test.js
   ```

6. Compare com `passo stats` em cada cópia. Para ter tokens por etapa sem hooks (subagentes, por exemplo), registre a transcrição do agente em `.passo/runs/<id>/sessions.json`: `[{"platform": "claude", "path": "<transcrição.jsonl>"}]`.
