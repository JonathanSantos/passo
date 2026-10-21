# Marca e design system

## Nome

**Passo.** De "passo a passo". O nome diz o que o harness faz: leva o pedido uma etapa por vez, com o dev aprovando. Não há analogia para aprender: as etapas são verbos comuns (refinar, planejar, construir, verificar, entregar), os tamanhos são os de camiseta (pp, p, m, g) e os agentes têm o nome do que fazem.

Frase: **passo a passo, com você aprovando.**

## Marca

<img src="assets/logo-light.svg" alt="passo" width="220">

Cinco degraus subindo, um por etapa, com o último no acento: a entrega, que é sua. Funciona em 16 px (favicon) e em uma cor só. O texto da marca é `passo`, em minúsculas, peso 600, espaçamento levemente negativo.

Arquivos em [`assets/brand`](../assets/brand): `mark.svg`, `logo-light.svg`, `logo-dark.svg`.

## Princípios

1. **Papel e tinta, um acento.** O acento significa "vez do dev". Nada mais compete com ele: se algo está verde, é você quem precisa agir.
2. **Linhas finas, nada de sombra.** Hierarquia vem de espaço e peso, não de profundidade.
3. **A fonte do sistema.** Sem webfont para baixar; números e código na monoespaçada do sistema, com algarismos tabulares.
4. **Calmo por padrão.** Um único ponto animado (a vez do dev); o resto não se mexe.
5. **Mesma página, dois usos.** O painel ao vivo e o relatório offline são o mesmo HTML.

## Tokens

| Token          | Claro     | Escuro    | Uso                                   |
| -------------- | --------- | --------- | ------------------------------------- |
| `--paper`      | `#F7F6F2` | `#111113` | fundo                                 |
| `--surface`    | `#FFFFFF` | `#18181B` | cartões                               |
| `--ink`        | `#1A1A1A` | `#EDEDE8` | texto, agente, etapa feita            |
| `--ink-2`      | `#55555C` | `#A6A6AD` | texto secundário                      |
| `--ink-3`      | `#8A8A90` | `#6F6F76` | rótulos, horários, auto               |
| `--line`       | `#E4E2DC` | `#2A2A2E` | bordas                                |
| `--accent`     | `#2F6B55` | `#8CC4AC` | **vez do dev**, aprovação, recomendado |
| `--ok`         | `#2F7D4F` | `#7CC49A` | checagem passou                       |
| `--warn`       | `#A5650F` | `#D9A45A` | tentativa, atenção                    |
| `--fail`       | `#B23A3A` | `#E08A80` | falha, trava negou                    |

Tipografia: 11 (rótulos em caixa alta, +0,06em), 12,5 (secundário), 14 (corpo), 17 (títulos de cartão), 26 (números). Pesos 400, 500 e 600. Raio de 8 px nos cartões e 6 px nos controles. Espaçamento em múltiplos de 4.

## Componentes

- **Trilho**: as cinco etapas lado a lado. Um traço embaixo mostra o estado (tinta = feita, acento = atual); a etiqueta `auto` marca onde o tamanho dispensa o agente.
- **Agora**: o cartão de quem tem a vez. Na vez do dev, ganha a borda e o ponto no acento, as perguntas com a recomendada marcada e o comando equivalente no terminal.
- **Atividade**: o log ao vivo. Ponto de tinta = agente, acento = dev, cinza = Passo, vermelho = trava.
- **Métricas**: números grandes e uma barra por etapa (tokens, ou tempo quando não há transcrição).

## Na CLI

A mesma ideia no terminal: o texto é puro (os agentes leem a mesma saída), com cor só onde ajuda, e só num terminal. `✓` verde, `✗` vermelho, `▸` e `★` no acento, títulos em negrito. `NO_COLOR` desliga tudo.
