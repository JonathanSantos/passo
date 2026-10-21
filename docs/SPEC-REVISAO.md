# Spec: correções da revisão da SPEC.md

A execução de `docs/SPEC.md` foi revisada item a item contra os critérios de pronto. Os onze itens estão feitos e as gates passam (fmt, clippy, 82 testes). A revisão achou um problema alto, quatro médios e um punhado de baixos. Esta spec os corrige, na ordem de gravidade.

Regras de execução iguais às de `docs/SPEC.md`: um commit por item, prefixo `rev-NN`, as três gates antes de cada commit, `CLAUDE.md` vale. Rode esta spec **depois** de `docs/SPEC-MAPA.md` terminar: os dois mexem em `service.rs`, `lock.rs` e `spec.rs`.

---

## 01 · Evidência do revisor: só arquivos dentro do projeto (alto)

**O que há.** `check_evidence` em `crates/passo-app/src/service.rs` faz `root.join(path)` com o texto escrito pelo revisor. `Path::join` troca a base quando o argumento é absoluto e não resolve `..`. Uma evidência `/etc/hosts:1` ou `../../x.rs:1` é confirmada, e o arquivo é lido para contar linhas. Um diretório (`src/`) também conta como evidência confirmada. E `.passo/runs/<id>/run.json`, `target/…` e `.git/HEAD` passam na checagem de caminho, enquanto a busca por símbolo exclui essas pastas: duas definições de "no projeto" na mesma função.

**Correção.**

- Uma referência de caminho só confirma se, depois de `canonicalize` (ou de normalizar `..` e `.` sem IO quando o arquivo não existe), o resultado começa pela raiz canonicalizada do projeto, não está em `IGNORED_DIRS` nem em `.passo/`, e é um arquivo. Caminho absoluto ou com `..` é recusado com `` `criterios[0].evidencia` cita /etc/hosts, que está fora do projeto ``. Diretório é recusado com `` `criterios[0].evidencia` cita src/, que é uma pasta: aponte o arquivo ``.
- Uma mesma lista de pastas excluídas serve ao caminho e ao símbolo.
- A mensagem de linha fora do arquivo fica como a spec fixou: `` `criterios[1].evidencia` cita src/task.rs:420, mas o arquivo tem 180 linhas `` (sem o `(s)`).
- Decisão sobre referência errada junto de referência certa: continua recusando. Evidência que cita uma linha que não existe é evidência errada, e o revisor barato aprende a ser exato num ciclo só. Atualizar o texto do item 03 da `docs/SPEC.md` para dizer isso em vez de "pelo menos uma referência que o Passo confirme".
- Sem git, a busca por símbolo varre os arquivos de teste e também os arquivos listados nos passos do plano, não só `tests.patterns`.
- Cache por entrega: cada arquivo de teste é lido uma vez por `done`, não uma vez por critério.

**Testes.** No app: evidência absoluta, com `..`, apontando pasta, apontando `.passo/` e apontando `target/` são recusadas com as mensagens acima; evidência só com o nome do teste entre crases (sem caminho) é aceita quando o teste existe e recusada quando não existe (hoje o teste cita caminho e nome juntos e nunca exercita o símbolo sozinho).

---

## 02 · `rodar` não confia no tokenizador da trava (médio)

**O que há.** O item 04 diz que cada `rodar` "passa pela trava". A trava quebra o comando em espaços e `;&|`, então `sh -c 'passo stop'`, `echo $(passo stop)`, `$(git commit -m x)` e `cat .env; false` passam, e é o próprio Passo quem executa o texto. A saída de um `rodar` que falha volta ao agente como feedback (cauda de 40 linhas), então `cat .env; false` leva o segredo ao agente. O gate `aprovar` lista os comandos e o dev lê, e essa é hoje a defesa real; a trava não é.

**Correção.** Um `rodar` não é um comando de shell qualquer: é uma chamada simples a um executor de testes. Regras, no core, em `parse_plan`/`submit_plan` (puras), com erro por campo:

- Proibidos em qualquer posição: `$(`, crase, `;`, `&&`, `||`, `|`, `>`, `<`, quebra de linha, `\`. Mensagem: `` `passos[1].rodar` não pode ter `$(`: o comando de teste é uma chamada simples, sem shell ``.
- A primeira palavra (o programa, sem caminho) precisa estar numa lista permitida: `tests.runners` em `passo.yaml`, com padrão `[cargo, npm, pnpm, yarn, bun, node, pytest, python, go, make, just, mvn, gradle, dotnet, mix, bundle, rspec]`, mais a primeira palavra de cada item de `checks.step` e `checks.final`. Fora da lista: `` `passos[1].rodar` começa com `curl`, que não é um executor de testes (permitidos: cargo, npm, …; ou acrescente em tests.runners) ``. A lista entra em `Config` (core, dado) e em `docs/EXTENDER.md`.
- A trava continua sendo consultada depois (defesa em profundidade), e o gate `aprovar` continua listando os comandos.
- O feedback de um `rodar` que falha continua mostrando a cauda da saída: com o programa restrito, a saída é a de um test runner.

**Testes.** No core: os cinco contornos acima são recusados com a mensagem certa; `npm test -- --grep prioridade` e `cargo test task::` passam; `tests.runners` do YAML amplia a lista.

---

## 03 · Re-plano não aponta passos feitos para critérios errados (médio)

**O que há.** Em `accept_plan` (`crates/passo-core/src/flow.rs`), depois de um `change`, os passos `Done` ficam com os `criteria` antigos enquanto `run.criteria` é substituído e renumerado. Um `S1` feito que atendia `C1 "ordenar"` passa a apontar para o novo `C1 "exportar"`, que nunca atendeu; o revisor vê `C1 ← S1 [hash]` errado; a herança de `rodar` do item 04 pega o comando desse passo. E a regra "todo critério precisa de um passo" só vale quando nenhum passo está feito, então o re-plano aceita critério órfão.

**Correção (decisão).** No re-plano, os passos feitos perdem a amarração: `criteria = []` e um campo `legacy: bool` (`#[serde(default)]`) marcando que vieram de um plano anterior. A regra de critério órfão vale sempre para os passos do plano novo. A herança de `rodar` só olha passos com critério igual, então passos legados não entram. `render` mostra os passos feitos de planos anteriores sob `Passos de planos anteriores (ficam, sem critério):` e nunca com `Atende:`. O mapa `C1 ← S1` em Verificar só lista passos não legados. A mensagem de contexto para o planejador no re-plano diz: `os passos já feitos ficam no branch; os critérios novos precisam de passos novos ou de um passo "conferir" quando já estão atendidos`.

**Testes.** No core: plano v1 com C1/C2, S1 feito, `change`, plano v2 com critérios novos: um passo sem `criterios` cobrindo um critério é recusado como órfão; com o plano certo, `S1.criteria` está vazio e `legacy = true`; `rodar` da correção de revisão não herda de `S1`.

---

## 04 · A simulação gera a tabela que o README mostra (médio)

**O que há.** `examples/simulacao/tabela.js` imprime um conjunto de linhas diferente da tabela em `README.md` (que tem suíte escondida, testes no fim, grau de prova, perguntas, vezes do dev, passos, tentativas · inválidas, tempo, tokens por etapa, tokens · chamadas). O README diz que a tabela deve ser regenerada pelo script, mas regenerar troca a estrutura.

**Correção.** `tabela.js` passa a emitir exatamente as linhas do README, na mesma ordem, a partir de `stats --json`, do `run.json` (prova, perguntas, passos) e do resultado da suíte escondida que `rodar.sh` guarda. Linhas novas (escaladas, ações por passo) entram no fim da tabela, e o README ganha essas linhas com `—` até a próxima rodada real. `rodar.sh`: documentar no README do exemplo a regra `limite → passo ok` como decisão; remover as pastas `mktemp -d` no fim de cada rodada (variável `KEEP=1` para manter); nomes de variáveis em inglês.

**Testes.** `node tabela.js` com dois resultados sintéticos imprime uma tabela com as mesmas linhas, na mesma ordem, que a do README (o teste compara os rótulos da primeira coluna).

---

## 05 · Orçamento de ações: teste da chave por passo e contagem só do permitido (médio)

**O que há.** O teste do item 08 roda em Refinar e só exercita a chave por etapa; a chave `<stage>:<step_id>:<attempts>` e o zerar na nova tentativa não têm teste. O hook conta antes do veredito, então chamadas negadas pela trava entram no orçamento. O painel conta por passo em todas as tentativas, enquanto o aviso zera por tentativa.

**Correção.** Contar só chamadas permitidas. Painel conta pela mesma chave (passo + tentativa). Teste em Construir: três chamadas permitidas e uma negada; a negada não conta; depois de uma falha de checagem a conta volta a zero.

---

## 06 · Escalada: nomes na skill, gêmeo do Copilot testado, sem escalar o que já é forte (baixo)

- `assets/skill/SKILL.md` nomeia os subagentes `-forte` (`passo-construtor-forte`, `passo-refinador-forte`), não só "a versão -forte".
- Teste de adapter afirma que o Copilot escreve `passo-construtor-forte.agent.md` com o modelo de `forte`.
- Se o agente da etapa já é `forte` (projeto configurou `model: forte` no construtor), o core não escala: `step_done` recebe esse fato no `StepFacts` (`agent_tier: String`) ou o app não liga a escalada; decisão: `StepFacts.agent_tier`, e o core só escala quando `agent_tier != "forte"`. Sem evento, sem métrica.

---

## 07 · Contexto do construtor: teste que prova o teto (baixo)

O teste usa 12 notas `nota N`, que não casam nenhuma regra; só a regra "cinco recentes" dispara e `<= 10` passa à toa. Usar 12 notas `Revisão: …` e afirmar exatamente 10. Ordem de saída: notas filtradas e recentes em ordem cronológica única, sem duplicata. O fallback "todos os critérios" vem depois de "Passo da vez", junto do passo.

---

## 08 · Árvore: testes de exclusão e de regeneração, erro não engolido (baixo)

- Testes afirmam que `.git`, `.passo` (que tem `audit/` rastreado) e `target` não aparecem na árvore; e que `change` regenera `tree.md`.
- `write_atomic(...).ok()` em `start`/`change` vira erro propagado ou, no mínimo, um evento `tree.failed` no log.
- "… e mais N pasta(s)" conta pastas, não linhas.
- `docs/SPEC.md` item 06: acrescentar a nota "renomeado para árvore pela SPEC-MAPA item 00".

---

## 09 · Recusa na CLI: teste do evento (baixo)

`crates/passo-cli/tests/agent_env.rs` roda sem run ativo, então o `lock.denied` com `via = "cli"` nunca é escrito. Acrescentar um caso com run ativo e afirmar o evento no `events.jsonl`.

---

## 10 · Documentação que ficou atrás do código (baixo)

- `docs/ARQUITETURA.md`: a escalada na falha (`step.escalated`, agentes `-forte`), o `rodar` do passo e a lista de executores permitidos, os critérios amarrados aos passos e o orçamento de ações (`step.budget`). Um parágrafo cada, nas seções que já existem.
- `README.md`, bullet do `passo stats`: ações por passo e escaladas.
