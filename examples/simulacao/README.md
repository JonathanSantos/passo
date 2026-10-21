# Simulação

Como medir o Passo com modelos baratos: o mesmo pedido, um agente por braço, uma suíte de aceite que nenhum agente vê.

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
