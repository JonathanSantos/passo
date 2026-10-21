# Estender o Passo

Tudo se estende com arquivos dentro de `.passo/`, sem tocar no binário. Uma regra vale para todos: **você muda como uma etapa é feita, nunca se ela acontece**. A ordem Refinar → Planejar → Construir → Verificar → Entregar e a aprovação da entrega ficam no core.

| Quero…                                       | Arquivo                              |
| -------------------------------------------- | ------------------------------------ |
| herdar a config de um time ou de uma stack   | `.passo/passo.toml` → `extends`      |
| contar as convenções do projeto aos agentes | `.passo/context.md`                  |
| ajustar ou trocar um agente                  | `.passo/agents/<nome>.md`            |
| mudar como uma etapa é feita                 | `.passo/stages/<etapa>.md`           |
| trazer um checklist do domínio               | `.passo/checklists/<nome>.md`        |
| mudar quem faz cada etapa num tamanho        | `.passo/passo.toml` → `[sizes.<t>]`  |
| trocar o modelo de um nível                  | `.passo/passo.toml` → `[models]`     |

Depois de mexer em agentes, rode `passo sync`: ele reescreve os arquivos de cada plataforma (`.claude/agents`, `.github/agents`). `passo doctor` mostra de onde veio cada coisa.

## `extends`: configuração em camadas

```toml
# .passo/passo.toml
extends = ["preset:rust", "~/time/passo.toml", "../plataforma/passo.toml"]
```

Da mais fraca para a mais forte: padrões do Passo, `~/.passo/passo.toml`, cada item de `extends` (na ordem) e por fim o próprio arquivo. **Tabelas se juntam chave a chave, listas e valores substituem.** Um arquivo de `extends` pode ter o seu próprio `extends`. Ciclos dão erro.

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

## Tamanhos (o fast travel)

```toml
[sizes.p]
verificar = "auto"     # só as checagens, sem revisor
approve_plan = false   # perguntas em aberto ainda chegam ao dev

[sizes.pp]
max_files = 5
max_lines = 200
```

| Campo                         | O que faz                                                                 |
| ----------------------------- | ------------------------------------------------------------------------- |
| `refinar`, `planejar`, `verificar` | `"agente"` ou `"auto"`. Construir é sempre um agente; entregar é sempre o Passo, com você aceitando |
| `approve_plan`                | pede a aprovação do plano. Perguntas abertas chegam ao dev de qualquer jeito |
| `max_questions`, `max_steps`  | limites para o refinamento e o plano                                      |
| `require_tests`               | um passo com `teste` precisa mudar um arquivo de teste                    |
| `max_files`, `max_lines`      | limites do diff, medidos pelo Passo (0 = sem limite; testes e `.md` não contam) |
| `block_sensitive`             | tocar em `limits.sensitive` para o run                                    |
| `max_rounds`                  | voltas da verificação antes de chamar o dev                               |

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

```toml
[models]
leve  = { claude = "haiku",  copilot = "Claude Haiku 5.5" }
forte = { claude = "sonnet", copilot = "Claude Sonnet 5.5" }
barato = { claude = "haiku", copilot = "GPT-5 mini" }
```

O agente pede um nível; cada plataforma recebe o nome de modelo dela. Para rodar tudo no modelo mais barato, aponte `forte` para o mesmo modelo de `leve`.

## Uma plataforma nova

Plataformas ficam atrás do trait `Platform` em [`crates/passo-adapters`](../crates/passo-adapters/src/lib.rs). Uma nova é um módulo que implementa:

- `install`: escreve a skill, os agentes e os hooks no projeto.
- `parse_hook` / `render`: lê o payload do hook e responde no dialeto da plataforma.
- `usage`: lê o uso de tokens das transcrições, se a plataforma as expõe.
- `delegation`: como o agente principal passa uma etapa a um subagente.

E uma linha em `registry()`. Nada fora desse crate conhece uma plataforma pelo nome.
