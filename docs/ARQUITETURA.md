# Arquitetura

Um binário (`passo`), seis crates, uma direção de dependência:

```
passo-cli ──► passo-web ──► passo-app ──► passo-store ──► passo-core
                               │                              ▲
                               └──────► passo-adapters ───────┘
```

| Crate            | Responsabilidade                                                         | IO  |
| ---------------- | ------------------------------------------------------------------------ | --- |
| `passo-core`     | vocabulário, máquina de estados, perfis de tamanho, validação, trava    | não |
| `passo-store`    | `.passo/`, log de auditoria, `extends`, catálogo de agentes e etapas     | disco |
| `passo-adapters` | Claude Code e Copilot: instalação, payloads de hook, dialetos, tokens    | disco |
| `passo-app`      | casos de uso, efeitos (checagens, git), texto da tarefa, árvore do repositório, métricas, hooks | tudo |
| `passo-web`      | painel ao vivo e relatório HTML                                          | rede local |
| `passo-cli`      | argumentos e saída                                                       | terminal |

## O fluxo é uma função pura

```rust
Engine::apply(&run, command, now) -> Result<(Run, Outcome { events, effects }), FlowError>
```

- **Comandos** vêm do agente (`SubmitBrief`, `SubmitPlan`, `StepDone`, `SubmitReview`, `Ask`), do dev (`Approve`, `Reject`, `Change`, `Resize`, `Stop`) ou do próprio Passo (`FinalChecks`).
- **Fatos** do mundo entram dentro do comando: `StepDone` leva o resultado das checagens e as medidas do diff, que o `passo-app` mede antes de chamar o motor.
- **Efeitos** saem como dados (`Commit`, `RunFinalChecks`, `WriteReport`, `Deliver`) e o `passo-app` os executa. Quando um efeito produz um fato novo (as checagens finais), ele volta ao motor como outro comando.
- Um comando inválido não muda nada: o motor trabalha numa cópia do run.

Por isso o fluxo é determinístico: a mesma sequência de comandos e fatos dá o mesmo run, byte a byte. Há um teste para isso (`the_same_inputs_give_the_same_run`).

### Quem tem a vez

`Run::turn()` dá sempre uma resposta só: `Agent`, `Dev` ou `None`. Há vez do dev quando existe um `gate`:

| Gate        | Quando                                                        |
| ----------- | ------------------------------------------------------------- |
| `aprovar`   | o plano está pronto (ou há perguntas em aberto)               |
| `aceitar`   | a entrega está pronta. Todo tamanho tem; nenhuma config tira  |
| `pergunta`  | o agente rodou `passo ask`                                    |
| `travado`   | um passo esgotou as tentativas, ou a verificação as voltas    |
| `limite`    | o diff passou do que o tamanho permite                        |

### Mudanças no pedido

Toda mudança de intenção passa pelo Refinar: recusar o plano e `passo change` mandam o run de volta ao refinamento, e os passos já feitos ficam. Cada resposta, recusa, mudança, dica, troca de tamanho e nova versão do plano vira um `Change` com quem, quando, em que etapa e o antes e depois. Cada versão do plano fica em `plan-v<N>.json`, e a aprovada fica registrada pela impressão digital.

### Critérios amarrados aos passos

Cada passo do plano declara os critérios que atende (`criterios`, guardados em `Step.criteria`). O parser recusa referência a critério que não existe, e `submit_plan` recusa critério sem passo. Num re-plano, os passos já feitos perdem a amarração (`legacy: true`, `criteria` vazio): os critérios foram trocados e renumerados, e `C1` pode ser outra coisa. A correção de um critério reprovado carrega o critério e herda o `rodar` do último passo (não legado) que o atendia. Construir mostra o que o passo atende; Verificar, `C1 ← S1 [commit]`.

### O teste do passo (`rodar`)

Um passo pode trazer o comando que roda só o seu teste. É o primeiro texto escrito pelo agente que o próprio Passo executa, então ele passa por duas barreiras puras em `submit_plan`: é uma chamada simples a um executor de testes (sem `$(`, crase, `;`, `&&`, `||`, `|`, `>`, `<`, quebra de linha ou `\`, e começando por um programa de `tests.runners` ou de alguma checagem configurada, `Config::refuse_test_command`), e a trava é consultada depois, em profundidade. Um plano com `rodar` sempre abre o gate `aprovar`, e aprovar autoriza esses comandos. No `done`, o app roda o `rodar` antes das checagens do passo, e a falha volta como falha de checagem.

### Escalada para o nível forte

Quando um passo falha `escalate_after` vezes (padrão 1; `0` em `pp`), o core marca `Step.tier = "forte"` e emite `step.escalated`, desde que o agente da etapa não seja forte já (`StepFacts.agent_tier`). A tarefa seguinte delega ao gêmeo `passo-<agente>-forte`, que a instalação escreve para cada agente de nível leve. O `ok` num `travado` zera as tentativas e mantém o nível.

## O mapa do projeto

Opcional. Um agente explorador (`passo map`, só leitura) entrega o mapa do projeto em JSON; o Passo confere a forma (`spec::parse_map`, puro) e o disco (caminhos que casam arquivos rastreados, fontes que existem, nada mudado durante o mapeamento) e grava `.passo/map/` em Markdown, no git. O `App` guarda o resultado da leitura sem nunca falhar: um mapa quebrado vira um aviso e o fluxo segue sem ele, com a trava inteira. A cada etapa, `passo_core::map::select` (puro) escolhe o que entra no contexto pelo alvo da etapa (o pedido em Refinar, os arquivos do pedido refinado em Planejar, os do passo em Construir, os do diff em Verificar) e pelo orçamento de linhas do tamanho (`map_lines`). Com mapa, a árvore de pastas sai de Refinar e Planejar.

## Auditoria

`events.jsonl` guarda uma linha por evento, e cada linha leva o SHA-256 da anterior. Editar ou apagar uma linha quebra a cadeia, e `passo log --verify` (ou o rodapé do painel) diz onde. Hooks e CLI escrevem ao mesmo tempo, então cada escrita pega uma trava de arquivo (`File::lock`) e lê só a última linha para encadear.

A cadeia sozinha só detecta edição: quem reescreve o arquivo inteiro recomputa os hashes. A âncora externa é o git. Todo commit que o Passo faz (um por passo, o de `stop`, o do log e o merge da entrega) leva o trailer `Passo-Audit: <hash do último evento>`, e ao encerrar o run o log e as versões do plano são copiados para `.passo/audit/<run>/` e commitados no branch do run. `verify` confere a cadeia e, depois, que o evento apontado pelo último commit do run ainda está no log. Os eventos depois desse commit (a entrega em si, hooks tardios) só existem em `runs/`, que continua fora do git por ser o estado vivo; e a CLI lê só `runs/`, então num clone novo a cópia em `audit/` se lê com as ferramentas de texto, não com `passo log`.

## Métricas

As métricas saem do log, não de contadores paralelos: o tempo de cada etapa vem dos eventos `stage.entered`; as vezes do dev e a espera dele, de `gate.opened` até o próximo evento do dev; as falhas, as negações da trava, as entregas inválidas e as escaladas (`step.escalated`), dos eventos de mesmo nome; as ações por passo, dos eventos `tool`, que guardam o passo. Os tokens vêm das transcrições que os hooks informam, atribuídos a cada etapa pelo horário. A série de `passo stats --all` é a mesma conta run a run, agregada por uma função pura (`metrics::aggregate`), lendo também os runs que só existem em `.passo/audit/<id>/` (a entrega e o `stop` arquivam ali o `run.json` final).

O orçamento de ações (`max_tool_calls` por tamanho) também lê o hook: `PreToolUse` conta só as chamadas que a trava deixou passar, por passo e tentativa (por etapa fora de Construir), em `budget.json`. Passou do limite, o hook não nega: responde `additionalContext` pedindo para entregar ou perguntar, na primeira vez e a cada 20 ações, e a primeira deixa `step.budget` no log.

## A trava

`passo_core::lock::judge(tool, root, run)` decide sobre uma chamada de ferramenta já traduzida (`ToolUse { kind, paths, command }`). Os adapters fazem a tradução, e o core não sabe qual plataforma chamou. Além do hook, o `passo-app` confere no git, ao receber uma entrega de etapa só de leitura, se algum arquivo mudou por um caminho que o hook não vê (um redirecionamento no shell, por exemplo).

Comandos de shell são lidos por um lexer POSIX pequeno, escrito à mão (`passo_core::shell::lex`): aspas, escapes, separadores, `$(…)` e crases (que viram comandos próprios), `$VAR`, redirecionamentos, comentários e heredocs (cujo corpo é dado). Programas que rodam outro comando (`sh -c`, `eval`, `xargs`, `env`, `sudo`…) também entram na lista. A trava olha todos esses comandos: `passo <decisão do dev>` e `git` de escrita são negados onde quer que apareçam; os destinos de redirecionamento são caminhos de escrita e as palavras com cara de caminho, de leitura, então `echo x > src/a.rs` numa etapa só de leitura e `cat .env` são negados na hora. O que o lexer não lê (aspas sem par, `$(` sem fechar, heredoc sem fim, ou um programa vindo de variável) é negado quando cita `passo` ou `git`, e em etapas só de leitura; em Construir passa, e a conferência pelo git cobre a escrita. O `rodar` de um passo usa o mesmo lexer para exigir uma chamada simples.

Um hook que falha deixa a chamada passar (um hook quebrado não pode prender o agente), mas nunca em silêncio: o erro vai para `.passo/hook-errors.log` (escrito só a partir do `Layout`, porque o que falhou costuma ser o `App::at`, com um `passo.yaml` inválido), o log do run ganha `hook.failed` uma vez por motivo, e o `next`, o `status` e o painel avisam que a trava pode não estar valendo. `passo doctor` roda o hook de cada plataforma de ponta a ponta, num ambiente mínimo: resolve o binário, compara a versão, manda um payload de `passo ok` e exige a negação.

Os hooks do projeto valem para a sessão aberta nele e para os subagentes dela (no teste ao vivo, o `PreToolUse` registrou as ações dos subagentes). Os comandos do dev têm uma terceira camada, que não depende de hook: `Platform::agent_env` diz se o processo roda no ambiente do agente (o Claude Code define `CLAUDECODE`, e subagentes herdam), e a CLI recusa `ok`, `no`, `change`, `size` e `stop` ali, gravando `lock.denied` com `via: cli`. A recusa fica só na CLI, nunca em `App::decide`: a aprovação pelo chat roda dentro do hook de prompt, no mesmo ambiente, e negaria a si mesma. `PASSO_DEV=1` libera o dev que roda a CLI de propósito no terminal do agente. O Copilot ainda não tem um sinal confiável (o VS Code marca todo terminal integrado igual), então ali vale só o hook.

## Padrões usados

| Padrão                                 | Onde                                                                   |
| -------------------------------------- | ---------------------------------------------------------------------- |
| Functional core, imperative shell      | `passo-core` decide; `passo-app` executa                               |
| State machine                          | `Engine` + `Run::turn()`                                               |
| Ports and adapters                     | `Platform`, `CheckRunner`, `Clock`, `Reporter`                         |
| Strategy                               | `Profile` por tamanho                                                  |
| Template method                        | ordem fixa no core, conteúdo das etapas vindo do catálogo              |
| Registry                               | `passo_adapters::registry()`                                           |
| Event log com cadeia de hash           | `passo-store::audit`                                                   |

## Decisões

- **YAML para dados, Markdown para instruções**: `passo.yaml` e os presets são YAML (`serde_yaml_ng`, com `deny_unknown_fields` e merge em camadas sobre o `Mapping`); agentes, etapas e checklists são Markdown com frontmatter YAML lido pelo mesmo parser.
- **Rust, um binário, sem runtime**: roda em qualquer máquina e em CI sem instalar Node ou Python, e responde aos hooks em milissegundos.
- **JSON para o que o agente entrega, texto para o que ele lê**: JSON tem erros precisos ("`passos[2].titulo` está vazio"); a tarefa em texto é o que o modelo lê melhor.
- **Plano validado contra o tamanho**: um modelo barato que planeja 12 passos para um pedido `p` recebe a recusa com o motivo.
- **O plano mais forte, a construção mais leve**: por padrão, `planejador` e `revisor` pedem o nível `forte` e `refinador` e `construtor` o `leve`. Um bom plano é o que faz um modelo barato acertar.
- **Aprovação pelo chat**: o hook de prompt do usuário só dispara com o que o dev digita, então `ok 1b` no chat conta como decisão do dev, sem abrir outro terminal.
- **O grau de prova diz o que foi verificado**: `forte` exige as checagens finais passando, a revisão confirmando todos os critérios e, quando `checks.coverage` está ligado, a cobertura do diff no mínimo configurado; falta uma das três, `média`; só as checagens por passo, `fraca`; nada, `nenhuma`. A cobertura entra como fato (`Command::Coverage`) depois das checagens finais: o app roda o comando, lê o lcov e compara com `git diff -U0`; o motor decide.
- **Polling em vez de SSE** no painel: mais simples, funciona atrás de qualquer proxy e 1,5 s basta para acompanhar.
