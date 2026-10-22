# Estender o Passo

Tudo se estende com arquivos dentro de `.passo/`, sem tocar no binário. Uma regra vale para todos: **você muda como uma etapa é feita, nunca se ela acontece**. A ordem Refinar → Planejar → Construir → Verificar → Entregar e a aprovação da entrega ficam no core.

Dois formatos, cada um para uma coisa:

- **Dados em YAML**: `.passo/passo.yaml` guarda os números e as escolhas do projeto (checagens, git, limites, tamanhos, modelos) e herda de outros arquivos com `extends`.
- **Instruções em Markdown**: agentes, etapas, checklists e as convenções do projeto são `.md`. O frontmatter (YAML entre `---`) leva só os dados daquele arquivo (`extends`, `model`, `tools`, `agent`, `checks`, `match`); o corpo é a instrução. Números do projeto e dos tamanhos nunca vão no frontmatter: ficam no `passo.yaml`.

Para acrescentar comportamento (um agente novo, uma etapa feita de outro jeito, um checklist), escreva um `.md`.

| Quero…                                       | Arquivo                              |
| -------------------------------------------- | ------------------------------------ |
| herdar a config de um time ou de uma stack   | `.passo/passo.yaml` → `extends`      |
| contar as convenções do projeto aos agentes | `.passo/context.md`                  |
| ajustar ou trocar um agente                  | `.passo/agents/<nome>.md`            |
| mudar como uma etapa é feita                 | `.passo/stages/<etapa>.md`           |
| trazer um checklist do domínio               | `.passo/checklists/<nome>.md`        |
| mudar quem faz cada etapa num tamanho        | `.passo/passo.yaml` → `sizes.<t>`    |
| trocar o modelo de um nível                  | `.passo/passo.yaml` → `models`       |
| dizer aos agentes como o projeto é organizado | `.passo/map/` (o mapa do projeto)   |
| transformar o que corrijo em convenção       | `passo learn` (veja abaixo)          |

Depois de mexer em agentes, rode `passo sync`: ele reescreve os arquivos de cada plataforma (`.claude/agents`, `.github/agents`). `passo doctor` mostra de onde veio cada coisa.

## `extends`: configuração em camadas

```yaml
# .passo/passo.yaml
extends: [preset:rust, ~/time/passo.yaml, ../plataforma/passo.yaml]
```

Da mais fraca para a mais forte: padrões do Passo, `~/.passo/passo.yaml`, cada item de `extends` (na ordem) e por fim o próprio arquivo. Um caminho relativo é relativo ao arquivo que o cita. **Mapas se juntam chave a chave, listas e valores substituem.** Um arquivo de `extends` pode ter o seu próprio `extends`. Ciclos dão erro.

Presets embutidos: `preset:rust`, `preset:node`, `preset:python`, `preset:go`. Eles só definem as checagens. O `passo init` escolhe o preset pela stack que encontrar.

Uma chave desconhecida dá erro com o nome dela: um erro de digitação nunca é ignorado em silêncio.

## Agentes

Os quatro agentes embutidos são `refinador`, `planejador`, `construtor` e `revisor` (veja [`assets/agents`](../assets/agents)).

**Acrescentar** a um agente (o caso comum):

```markdown
<!-- .passo/agents/construtor.md -->
---
extends: construtor
---
- Erros de domínio usam `thiserror`; nada de `unwrap()` fora de testes.
- Testes de integração ficam em `tests/`, um arquivo por módulo.
```

**Substituir** um agente: o mesmo nome, sem `extends`. **Criar** um agente: um nome novo, com ou sem `extends`:

```markdown
<!-- .passo/agents/construtor-front.md -->
---
extends: construtor
description: Constrói telas React seguindo o design system do time.
model: forte
tools: [ler, editar, terminal]
---
Componentes em `src/ui/`, com stories. Use os tokens de `src/ui/tokens.ts`; nada de cor fixa.
```

Campos: `extends`, `name`, `description`, `model` (um nível de `[models]`: `leve`, `forte` ou um seu), `tools` (`ler`, `editar`, `terminal`; cada plataforma traduz para os nomes dela).

## Etapas

Um arquivo por etapa, com o nome dela: `refinar.md`, `planejar.md`, `construir.md`, `verificar.md`.

```markdown
<!-- .passo/stages/construir.md -->
---
agent: construtor-front
checks: [npm run lint, npm run test:unit -- --changed]
---
Rode o Storybook localmente antes de entregar.

## m
Cada componente novo ganha uma story com os estados vazio, carregando e erro.
```

- `agent`: qual agente faz a etapa.
- `checks`: checagens extras. Em `construir`, rodam depois de cada passo; em `verificar`, junto das finais.
- O corpo é acrescentado às instruções embutidas. O texto antes do primeiro `##` vale para todos os tamanhos; `## pp`, `## p`, `## m`, `## g` valem só para aquele tamanho.
- `mode: replace` substitui as instruções embutidas em vez de acrescentar.
- `skip`, `enabled` e afins dão erro: etapas não se desligam. Para um tamanho não usar agente numa etapa, use os tamanhos.

## Cobertura do diff

Prova `forte` diz que a suíte passou e o revisor confirmou os critérios. Com a cobertura ligada, diz também que os testes executam as linhas que o run mudou:

```yaml
checks:
  coverage:
    command: cargo llvm-cov --lcov --output-path {out} --quiet   # {out}: o lcov que o Passo lê
    min_diff: 70        # % das linhas mudadas que algum teste executa; 0 desliga
    required: false     # true: abaixo do mínimo vira passo de correção
```

`{out}` é o arquivo lcov que o Passo lê; `{outdir}` é uma pasta, lida como `{outdir}/lcov.info`. Os presets trazem o comando (rust: `cargo llvm-cov`; node: `npx c8`; python: `pytest --cov`) com `min_diff: 0`; para ligar, ponha só `min_diff` no seu `passo.yaml`. Go fica sem, por enquanto.

Depois que as checagens finais passam, o Passo roda o comando, lê o lcov e compara com as linhas que o run mudou (`git diff <base> -U0`; arquivos de teste e `.md` ficam de fora, como no limite do diff, e só contam as linhas que o lcov considera executáveis). O resultado vai para o log (`Cobertura do diff: 83% (12 linha(s) sem teste em 2 arquivo(s))`), para o revisor, para o gate de aceitar e para o painel. Abaixo do mínimo, com `required: false`, a prova fica no máximo em `média` e as linhas sem teste viram nota para o revisor; com `required: true`, viram um passo de correção, que conta como volta da verificação. Sem git, ou se o comando falhar, a cobertura fica como não medida e a prova não passa de `média`.

O comando de cobertura roda a suíte de novo: a verificação demora cerca do dobro.

## Executores de teste

O `rodar` de um passo (o comando que roda só o teste dele) é uma chamada simples a um executor de testes: sem `$(`, crase, `;`, `&&`, `||`, `|`, `>`, `<`, quebra de linha ou `\`, sem variável de ambiente na frente, e começando por um programa permitido. Por padrão: `cargo`, `npm`, `pnpm`, `yarn`, `bun`, `node`, `pytest`, `python`, `go`, `make`, `just`, `mvn`, `gradle`, `dotnet`, `mix`, `bundle` e `rspec`, mais o programa de cada item de `checks.step` e `checks.final`. Para acrescentar:

```yaml
tests:
  runners: [vitest]          # soma aos padrões
  # runners_only: true       # use só estes (e os das checagens)
```

A trava também é consultada, e o dev aprova os comandos junto com o plano. O plano volta recusado com uma destas mensagens:

- `` `passos[1].rodar` não pode ter `&&`: o comando de teste é uma chamada simples, sem shell `` (vale para cada símbolo da lista acima);
- `` `passos[1].rodar` não pode ter variável de ambiente (FOO=1); configure em checks ou no comando de teste do projeto ``;
- `` `passos[1].rodar` começa com `x`, que não é um executor de testes (permitidos: …; ou acrescente em tests.runners) ``;
- `` `passos[1].rodar` não é uma chamada simples (variável, redirecionamento ou mais de um comando): o comando de teste é uma chamada simples, sem shell ``.

## Segredos no log

O log de auditoria, o `run.json`, o painel e o relatório guardam texto vindo do agente e da shell (comandos, saída das checagens). Antes de guardar, o Passo esconde valores de chaves como `token`, `secret`, `password`, `api_key`, `authorization`, `cookie` e `session` (`TOKEN=…`, `password: …`, `--token …`, `Authorization: Bearer …`), tokens com prefixo conhecido (`ghp_`, `sk-`, `AKIA…`, blocos PEM) e valores longos de linhas de `.env`. Para tratar outras palavras como chave:

```yaml
audit:
  secrets: [minha_chave, segredo_interno]
```

## Tamanhos (o fast travel)

```yaml
sizes:
  p:
    verificar: auto       # só as checagens, sem revisor
    approve_plan: false   # perguntas em aberto ainda chegam ao dev
  pp:
    max_files: 5
    max_lines: 200
```

| Campo                         | O que faz                                                                 |
| ----------------------------- | ------------------------------------------------------------------------- |
| `refinar`, `planejar`, `verificar` | `agente` ou `auto`. Construir é sempre um agente; entregar é sempre o Passo, com você aceitando |
| `approve_plan`                | pede a aprovação do plano. Perguntas abertas chegam ao dev de qualquer jeito |
| `max_questions`, `max_steps`  | limites para o refinamento e o plano                                      |
| `require_tests`               | um passo com `teste` precisa mudar um arquivo de teste                    |
| `max_files`, `max_lines`      | limites do diff, medidos pelo Passo (0 = sem limite; testes e `.md` não contam) |
| `block_sensitive`             | tocar em `limits.sensitive` para o run                                    |
| `max_rounds`                  | voltas da verificação antes de chamar o dev                               |
| `max_tool_calls`              | ações do agente por passo (por etapa, fora de Construir) antes de o Passo avisar para entregar ou perguntar; o aviso não nega nada (0 = sem limite; padrões 30, 60, 80 e 120) |
| `max_tokens`                  | tokens faturáveis do run (entrada + saída + escrita de cache) antes de o Passo parar e perguntar: `passo ok` segue até o fim, `passo size` sobe o teto (0 = sem limite; padrões 60k, 150k, 400k e 1M). Só vale onde a plataforma expõe as transcrições (hoje o Claude Code) |
| `map_lines`                   | linhas do mapa do projeto que cada etapa lê: `{ refinar, planejar, construir, verificar }` (`0` desliga a etapa; padrão 0 em `pp` e 60, 120, 60 e 100 nos outros) |
| `escalate_after`              | tentativas falhas de um passo antes de a próxima pedir o nível forte (0 = nunca; padrão 0 em `pp`, 1 nos outros). Cada agente de nível leve ganha um gêmeo `-forte` na instalação |

## O mapa do projeto

Um agente explora o projeto e entrega um mapa: a estrutura, as áreas, as práticas, as bibliotecas e como cada uma é usada. O Passo guarda o mapa em `.passo/map/`, no git, e mostra a cada etapa só a parte que interessa ao que ela está fazendo. O mapa é opcional: sem ele, o Passo mostra a árvore de pastas que ele mesmo gera.

```text
.passo/map/
  projeto.md         stack, como rodar, como testar, as convenções que mais importam
  areas/<id>.md      uma por área (módulo, camada, pasta com responsabilidade própria)
  libs/<nome>.md     uma por biblioteca que o projeto envolve de um jeito próprio
```

Uma área:

```markdown
---
paths: [src/tasks.js, src/cli.js]
depends_on: [armazenamento]
libs: [commander]
tags: [cli, tarefas]
sources: [src/tasks.js, test/tasks.test.js]
commit: "7033609"
---
Regras de negócio das tarefas. Cada comando da CLI chama uma função pura daqui.
Testes em `test/tasks.test.js`, com `node:test`, um `describe` por comando, sem mock do disco.
```

| Campo        | O que é                                                              |
| ------------ | -------------------------------------------------------------------- |
| `paths`      | globs relativos à raiz do projeto: os arquivos que a área cobre      |
| `depends_on` | ids de outras áreas                                                  |
| `libs`       | bibliotecas que a área usa (cada uma pode ter o seu `libs/<nome>.md`) |
| `tags`       | palavras que trazem a área quando aparecem num pedido                |
| `sources`    | arquivos que sustentam o que o corpo afirma                          |
| `commit`     | commit em que a área foi escrita ou confirmada (entre aspas)         |

O `projeto.md` aceita `sources` e `commit`; uma lib, `tags`, `sources` e `commit`. Chave desconhecida ou `depends_on` para área que não existe dão erro com o arquivo e a linha. Edite à mão à vontade: o que o Passo confere a mais é o que o agente entrega com `passo map`. Um novo `passo map` reescreve o corpo de cada área, mas mantém as seções `## Anotações` (do revisor) e `## Aprendido` (do `passo learn`) e o que você escreveu abaixo delas; uma área que sai do mapa leva as anotações junto, e a saída diz quais saíram. Durante o mapeamento, o explorador pode ler `.passo/map/` e `.passo/context.md`, nunca escrever. O `context.md` continua acima do mapa: é o que você escreve e o mapa não substitui. `passo doctor` diz se há mapa, quantas áreas e de quando ele é.

### Projetos anexados

Quando um pedido depende de outro repositório ao lado, anexe-o no `passo.yaml`:

```yaml
projects:
  - name: api
    path: ../api
```

O Passo lê o mapa de `../api/.passo/map/` com os ids prefixados (`api/armazenamento`), e uma área daqui pode citar `depends_on: [api/armazenamento]`. As áreas anexadas nunca casam os arquivos daqui: chegam ao contexto por dependência ou pelas palavras do pedido, mostradas a partir daqui (`api/armazenamento (../api/src/store.ts)`). O projeto anexado é só leitura: durante um run ou um mapeamento, a trava nega escrita nele (inclusive depois de um `cd ../api` no mesmo comando), e, quando ele é um repositório git, a entrega de uma etapa só de leitura e a do mapa conferem que nada mudou lá (`api:src/x.ts` no aviso). O mapa dele se faz lá, com o `passo map` de lá; sem mapa, o `start` e o `doctor` avisam e seguem. Um `depends_on: [api/x]` precisa citar um projeto de `projects` e uma área que existe no mapa dele; um mapa anexado que não lê fica de fora com aviso, e o mapa daqui continua valendo.

## Aprender com os runs

Cada recusa (`passo no "…"`), mudança no pedido, dica e resposta diferente da recomendada fica no run, assim como as observações do revisor. Quando há 3 ou mais dessas lições ainda não vistas, de 2 ou mais runs, `passo status` e o fim de uma entrega avisam. Então:

1. O agente roda `passo learn`. O Passo junta as lições (sem modelo: lições com duas palavras longas em comum ficam no mesmo grupo; um grupo de mais de um run aparece primeiro) e mostra ao refinador, no nível forte, com o `context.md` atual e as áreas do mapa que os arquivos tocam.
2. O agente entrega até 10 propostas com `passo done`: uma frase cada (até 160 caracteres), um destino que existe (`context`, `mapa:<área>` ou `agente:<agente>`) e as fontes, copiadas das lições. A lição que não vale como regra vai em `descartadas`, com a fonte e o porquê.
3. Você escolhe: `passo learn --apply 1,3` (ou `todas`), ou `passo learn --cancel`.

Aplicar escreve:

| Destino           | Onde                                                                          |
| ----------------- | ----------------------------------------------------------------------------- |
| `context`         | `.passo/context.md`, sob `## Aprendido com os runs`, com as fontes num comentário HTML |
| `mapa:<área>`     | `## Anotações` da área em `.passo/map/areas/<área>.md`                        |
| `agente:<nome>`   | `.passo/agents/<nome>.md` sob `## Aprendido com os runs` (criado com `extends: <nome>` se não existe); rode `passo sync` |

As lições citadas pelas propostas, aplicadas ou não, e as `descartadas` entram em `.passo/learn/seen.json`, que vai no git: ninguém do time revê as mesmas. As que o agente não citou voltam na próxima vez; ninguém decidiu sobre elas. Descartar a rodada inteira (`--cancel`) não marca nada como visto. Enquanto o agente trabalha, o projeto é só leitura, como no mapeamento.

## Checklists de domínio

```markdown
<!-- .passo/checklists/pagamentos.md -->
---
title: Pagamentos
match: [pagamento, cobrança, pix, boleto, cartão]
---
- Valores em centavos, inteiros; nunca float.
- Toda chamada ao gateway tem idempotency key.
- Webhook valida a assinatura antes de qualquer efeito.
```

Quando uma palavra de `match` aparece no pedido, o checklist entra no planejamento e na revisão. Um arquivo com o nome de um embutido (`api-http`, `cli`, `ui`, `dados`) o substitui.

## Modelos

```yaml
models:
  leve: { claude: haiku, copilot: Claude Haiku 5.5 }
  forte: { claude: sonnet, copilot: Claude Sonnet 5.5 }
  barato: { claude: haiku, copilot: GPT-5 mini }
```

O agente pede um nível; cada plataforma recebe o nome de modelo dela. Os valores acima são os padrões, e cada plataforma traz os seus: você só escreve o que muda. Para rodar tudo no modelo mais barato, aponte `forte` para o mesmo modelo de `leve`.

## Uma plataforma nova

Plataformas ficam atrás do trait `Platform` em [`crates/passo-adapters`](../crates/passo-adapters/src/lib.rs). Uma nova é um módulo que implementa:

- `install`: escreve a skill, os agentes e os hooks no projeto.
- `installed`: diz se o `init` já passou por este projeto.
- `default_model`: o modelo da plataforma para cada nível (`leve`, `forte`).
- `parse_hook` / `render`: lê o payload do hook e responde no dialeto da plataforma. Se a plataforma informa a raiz do projeto por fora do payload (uma variável de ambiente), é `parse_hook` que preenche `project_dir`.
- `usage`: lê o uso de tokens das transcrições, se a plataforma as expõe.
- `delegation`: como o agente principal passa uma etapa a um subagente.

E uma linha em `registry()`. Nada fora desse crate conhece uma plataforma pelo nome.
