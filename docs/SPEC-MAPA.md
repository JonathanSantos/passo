# Spec: o mapa do projeto

Um agente explora o projeto (e os projetos anexados) e entrega um mapa: a estrutura, as áreas, as práticas, as bibliotecas e como cada uma é usada. O Passo guarda o mapa em Markdown, confere a validade dele pelo git, e injeta em cada etapa só a parte que interessa ao que o agente está atacando.

O mapa é **opcional**. Sem ele, o Passo trabalha como hoje, com a árvore gerada (`repomap`). Com ele, a árvore dá lugar ao mapa nas etapas e o agente barato deixa de gastar tokens descobrindo onde as coisas estão.

## Como executar esta spec

As mesmas regras de `docs/SPEC.md`: um item por vez, na ordem; um commit por item com prefixo `mapa-NN`; `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` antes de cada commit; `CLAUDE.md` vale (inglês no código, português para quem usa, core puro, plataformas só nos adapters). Esta spec parte do código depois de `docs/SPEC.md` executada (82 testes).

Vocabulário, para não criar jargão: **mapa** é o que o agente explorador entrega; **área** é um nó do mapa; **árvore** é a lista mecânica de pastas que o Passo gera sozinho (hoje chamada de "mapa do repositório"). O item 00 arruma esse nome.

---

## 00 · A árvore mecânica chama-se árvore

**Por quê.** O item 06 da spec anterior chamou a lista de pastas gerada de "mapa do repositório". Esta spec usa "mapa" para o que o agente entrega. Dois mapas confundem.

**Desenho.** Renomear: `crates/passo-app/src/repomap.rs` → `tree.rs`; `Layout::map(id)` → `Layout::tree(id)` e o arquivo `runs/<id>/map.md` → `tree.md`; `View.map` → `View.tree`; o título no contexto passa de `Mapa do repositório:` para `Árvore do repositório:`. Texto do `context.md` modelo e docs acompanham.

**Pronto quando.** `grep -rni "mapa do reposit\|repomap" crates assets docs README.md` não acha nada; os testes do item 06 passam com os nomes novos.

---

## 01 · Tipos e seleção, no core

**Por quê.** A seleção do que entra no contexto é determinística e não depende de modelo nem de disco. Puro, cabe em `passo-core` e é testável sem IO.

**Desenho.** Novo módulo `crates/passo-core/src/map.rs`:

```rust
pub struct Map { pub project: Option<Node>, pub areas: Vec<Node>, pub libs: Vec<Node> }

pub struct Node {
    pub id: String,              // "tarefas", "api/armazenamento" (projeto anexado)
    pub kind: NodeKind,          // Project | Area | Lib
    pub paths: Vec<String>,      // globs relativos à raiz do projeto do nó
    pub depends_on: Vec<String>, // ids de outras áreas
    pub libs: Vec<String>,       // nomes de libs que a área usa
    pub tags: Vec<String>,
    pub sources: Vec<String>,    // arquivos que sustentam o que o nó afirma
    pub commit: Option<String>,  // commit em que o nó foi escrito ou confirmado
    pub body: String,            // o Markdown, sem o frontmatter
}

pub struct Target<'a> { pub stage: Stage, pub files: &'a [String], pub words: &'a str }

pub struct Selection<'a> { pub nodes: Vec<&'a Node>, pub uncovered: Vec<String> }

pub fn select<'a>(map: &'a Map, target: &Target, budget_lines: usize) -> Selection<'a>
```

`globset` volta a ser dependência de `passo-core` (foi removida por não ser usada; a seleção é pura, então cabe aqui).

Regras de `select`, nesta ordem, até estourar o orçamento de linhas (contando o `body` de cada nó):

- O nó `project` entra sempre, primeiro.
- `Refinar`: áreas cujas `tags` ou cujos `paths` aparecem como palavra inteira em `words` (o pedido). Sem salto de dependência.
- `Planejar`: áreas que cobrem algum caminho de `files` (os `arquivos` do pedido refinado) ou casam com `words`; mais um salto de `depends_on`; mais as `libs` citadas por essas áreas.
- `Construir`: só as áreas que cobrem `files` (os `arquivos` do passo da vez). Sem salto, sem libs além das dessas áreas.
- `Verificar`: as áreas que cobrem `files` (os arquivos do diff do run), sem salto, com as libs delas.
- `Entregar`: nada.
- Cobertura é `globset` sobre `paths`; um `files` que nenhuma área cobre vai para `uncovered`.
- Ordem de desempate: áreas que cobrem mais arquivos primeiro; libs por ordem de citação. Determinístico.

Orçamento em linhas por etapa, em `Profile`, como um struct `MapLines { refinar, planejar, construir, verificar }` (chave `map_lines` em `sizes.<t>` no YAML, com `deny_unknown_fields`). Padrões: `pp` tudo `0` (não injeta); `p`, `m` e `g`: Refinar 60, Planejar 120, Construir 60, Verificar 100. `0` numa etapa desliga o mapa só nela.

**Arquivos.** `crates/passo-core/src/map.rs`, `crates/passo-core/src/lib.rs`, `crates/passo-core/src/config.rs` (`map_lines`), testes no próprio módulo.

**Pronto quando.** Testes puros: cada etapa seleciona o que a regra diz num mapa sintético de cinco áreas e duas libs; o salto de dependência só acontece em Planejar; o orçamento corta pela ordem; `uncovered` lista o que ninguém cobre; a mesma entrada dá a mesma saída.

---

## 02 · O mapa em disco

**Por quê.** O mapa é Markdown com frontmatter YAML, por arquivo, no git. Uma pessoa pode editar à mão; o agente só escreve por meio do Passo.

**Desenho.**

```text
.passo/map/
  projeto.md         stack, como rodar, como testar, as convenções que mais importam
  areas/<id>.md      uma por área (módulo, camada, pasta com responsabilidade própria)
  libs/<nome>.md     uma por biblioteca que o projeto envolve de um jeito próprio
```

Frontmatter de uma área:

```markdown
---
paths: [src/tasks.js, src/cli.js]
depends_on: [armazenamento]
libs: [commander]
tags: [cli, tarefas]
sources: [src/tasks.js, test/tasks.test.js]
commit: 7033609
---
Regras de negócio das tarefas. Cada comando da CLI chama uma função pura daqui.
Testes em `test/tasks.test.js`, com `node:test`, um `describe` por comando, sem mock do disco.
Erro de domínio é `TaskError` com `code`; a CLI traduz para a mensagem.
```

- `crates/passo-store/src/map.rs`: `load(layout) -> Result<Option<Map>>` (lê os três lugares com `frontmatter::parse`; `None` quando `.passo/map/` não existe; erro com arquivo e linha quando um frontmatter está inválido, tem chave desconhecida, ou um `depends_on` aponta para área inexistente: a mesma regra de `deny_unknown_fields` do resto, e o que garante que `save` e `load` sejam ida e volta sem perda) e `save(layout, &Map) -> Result<Vec<PathBuf>>` (escreve com `write_atomic`, apaga áreas e libs que saíram do mapa, devolve os caminhos relativos para o git).
- `Layout::project_map()` → `.passo/map`. Não entra no `.gitignore` de `.passo/`: o mapa vai para o git.
- `.passo/context.md` continua existindo e continua acima de tudo: é o que a pessoa escreve à mão e o mapa não substitui.
- `Store::init` não cria o mapa. `passo doctor` mostra `Mapa do projeto: nenhum (opcional: peça ao agente "com o passo, mapeie o projeto")` ou `Mapa do projeto: 9 áreas · 4 libs · feito em <commit> (<n> dias)`.

**Arquivos.** `crates/passo-store/src/map.rs`, `crates/passo-store/src/lib.rs`, `crates/passo-store/src/layout.rs`, `crates/passo-cli/src/main.rs` (`doctor`), `docs/EXTENDER.md` (nova seção "O mapa do projeto", com o formato acima e a frase: edite à mão à vontade; o que o Passo valida está no item 03).

**Pronto quando.** Teste de ida e volta: `save` e `load` devolvem o mesmo `Map`; uma área com `depends_on` órfão dá erro com o caminho do arquivo; sem a pasta, `load` devolve `None` e nada falha.

---

## 03 · `passo map`: o agente explora, o Passo valida e escreve

**Por quê.** Mapear não é uma das cinco etapas: é um trabalho à parte, só de leitura, sem branch, sem commits de passo, sem aceite de entrega. Mas usa as mesmas peças: uma tarefa impressa, uma entrega em JSON com erro por campo, a trava e o hook de Stop.

**Desenho.**

- **Agente.** `assets/agents/explorador.md`: nível `forte`, `tools: [ler, terminal]`. Faz: lê a árvore e os manifestos; abre os arquivos mais centrais de cada área; procura como os testes são escritos; lista as bibliotecas e como o projeto as envolve. Nunca: escreve no projeto, instala dependências, roda scripts que mudem arquivos. Instalado nas plataformas como os outros (`passo-explorador`), com o sufixo `-forte` se o item 02 da spec anterior assim pedir para agentes `leve` (ele já é `forte`, então um arquivo só).
- **Começar.** `passo map` (CLI; no app, `App::map_start`):
  - Recusa se há run ativo: `termine ou interrompa o run antes de mapear`.
  - Grava `.passo/map/.task.json` com `{ "started_at", "commit", "projects": [...] }` (este arquivo entra no `.gitignore` de `.passo/`: é estado vivo, como `runs/`).
  - Imprime a tarefa (`render::map_task`): o papel do explorador, a árvore de cada projeto (a mesma função do item 00, sem limite de linhas), o mapa atual se existir (para atualizar em vez de refazer), as regras do formato e o JSON a entregar:

```json
{
  "projeto": {
    "stack": "Node 20, sem framework",
    "rodar": "node src/cli.js",
    "testar": "npm test",
    "convencoes": ["funções puras em src/, IO só em cli.js", "erros com code, nunca throw de string"],
    "fontes": ["package.json", "src/cli.js"]
  },
  "areas": [
    {
      "id": "tarefas",
      "caminhos": ["src/tasks.js", "src/cli.js"],
      "depende_de": ["armazenamento"],
      "libs": ["commander"],
      "tags": ["cli", "tarefas"],
      "resumo": "o que esta área faz, em 1 a 3 frases",
      "praticas": ["como se escreve código aqui, uma por linha, cada uma com a fonte"],
      "testes": "onde e como se testa esta área",
      "fontes": ["src/tasks.js", "test/tasks.test.js"]
    }
  ],
  "libs": [
    { "nome": "commander", "uso": "como o projeto a envolve e onde", "fontes": ["src/cli.js"] }
  ]
}
```

- **Entregar.** `passo done <arquivo>` sem run ativo e com `.task.json` presente é a entrega do mapa (`App::map_done`). Com run ativo, `done` é do run, como hoje. Validação, no mesmo estilo do pedido refinado e da revisão, com uma linha por problema:
  - `projeto.stack`, `projeto.testar` e `projeto.fontes` obrigatórios; `areas` com 1 a 12 itens; `id` é um slug (`[a-z0-9-]`), único; `caminhos` com pelo menos um item, cada glob casando ao menos um arquivo rastreado; `depende_de` cita ids existentes, sem ciclo; `libs` cita nomes presentes em `libs` ou nos manifestos; `fontes` com pelo menos um item, cada um existindo; `resumo` com 1 a 3 frases; `praticas` com até 8 itens; o corpo gerado de uma área tem no máximo 40 linhas.
  - A conferência de existência é IO e fica no app; o parser e as regras de forma ficam em `passo-core::spec::parse_map` (puro). Erros usam o caminho do campo: `` `areas[2].caminhos[0]` não casa nenhum arquivo rastreado `` · `` `areas[0].depende_de` cita `cache`, que não é uma área ``.
  - Aceita: o app monta o `Map` (o `commit` de cada nó é o HEAD de agora), chama `store::map::save`, apaga `.task.json`, escreve o evento num log próprio `.passo/map/log.jsonl` (mesma `EventLog`, mesma cadeia: `map.started`, `map.invalid`, `map.saved` com a contagem de áreas) e imprime `Mapa salvo: 9 áreas, 4 libs. Commite .passo/map/.` O Passo não commita o mapa sozinho: mapear não abre branch.
- **Trava e Stop.** Hoje `lock::judge` e `judge_stop` recebem `Option<&Run>`. Trocar por um `Activity` puro no core: `enum Activity<'a> { Idle, Run(&'a Run), Mapping }`. `Mapping`: escrita no projeto negada (`o mapeamento é só leitura: o mapa é entregue com passo done`), `.passo/` negada, comandos do dev negados, git de escrita negado. `judge_stop(Mapping)` bloqueia com `O Passo espera o mapa: entregue com passo done <arquivo>`, com o mesmo teto de três bloqueios (chave `map:<started_at>`). Os hooks em `passo-app/src/hooks.rs` montam o `Activity` a partir de `store.active()` e da presença de `.task.json`. Os eventos `tool` e `lock.denied` durante o mapeamento vão para `.passo/map/log.jsonl`.
- **Enquanto há um mapeamento.** `passo start` recusa: `há um mapeamento em andamento; entregue o mapa com passo done ou cancele com passo map --cancel`. `passo next` reimprime a tarefa do mapa. O hook `SessionStart` avisa `há um mapeamento em andamento; rode passo next`. A skill diz para o agente seguir o mesmo ciclo `next`/`done` do run.
- **Conferência pelo git na entrega.** Como nas etapas só de leitura, `map_done` confere que a árvore de trabalho não mudou desde o `map_start` (a mesma `baseline`/`dirty` de `check_read_only`, ignorando só `.passo/map/.task.json`). O explorador tem `terminal`, e um `npm install` reescreve um lockfile rastreado sem o hook ver. Se mudou: evento `lock.breach` no log do mapa e a entrega é recusada com a lista dos arquivos.
- **Cancelar.** `passo map --cancel` apaga `.task.json` e é decisão do dev. `map` sem argumento é do agente, então `DEV_COMMANDS` não recebe `map`; em vez disso, `dev_command_in` passa a reconhecer o par `map --cancel`, a trava nega com a mesma mensagem dos outros comandos do dev, e a recusa da CLI no ambiente do agente (item 09 da spec anterior) cobre `map --cancel` também.
- **Skill.** `assets/skill/SKILL.md` ganha a seção "Mapear o projeto": quando o dev pede "mapeie o projeto", rode `passo map`, delegue ao `passo-explorador` e entregue com `passo done`.

**Arquivos.** `assets/agents/explorador.md`, `assets/skill/SKILL.md`, `crates/passo-store/src/assets.rs`, `crates/passo-core/src/spec.rs` (`parse_map`, `MAP_TEMPLATE`), `crates/passo-core/src/lock.rs` (`Activity`), `crates/passo-app/src/service.rs` (`map_start`, `map_done`, `map_cancel`), `crates/passo-app/src/render.rs` (`map_task`), `crates/passo-app/src/hooks.rs`, `crates/passo-store/src/layout.rs` (`map_task()`, `map_log()`), `crates/passo-store/src/runs.rs` (o `.gitignore` de `.passo/` ganha `map/.task.json`; o `map/log.jsonl` fica no git junto com o mapa), `crates/passo-cli/src/main.rs` (`Map { cancel: bool }`), `crates/passo-app/tests/app.rs`, `README.md` (seção "Estender" ganha dois parágrafos sobre o mapa; tabela de comandos ganha `passo map`).

**Pronto quando.** Teste de ponta a ponta no app: `map_start` escreve `.task.json` e a saída contém `# Agente: explorador` e a árvore; um hook `Write` em `src/x.txt` durante o mapeamento é negado com a mensagem acima; `done` com `caminhos` que não casam nada é recusado com o campo; `done` válido escreve `.passo/map/projeto.md`, `areas/tarefas.md` e `libs/commander.md`, apaga `.task.json` e `load` devolve 1 área; o Stop bloqueia durante o mapeamento e libera depois. Teste no core: `parse_map` com ciclo em `depende_de` é recusado; `judge(Mapping)` nega escrita e permite leitura.

---

## 04 · O mapa no contexto de cada etapa

**Por quê.** É para isto que o mapa existe. A seleção do item 01 decide o quê; aqui decide-se onde e como aparece.

**Desenho.**

- `App` guarda o resultado da leitura, não o mapa: `map: Result<Option<Map>, String>`, preenchido em `App::at` sem nunca fazer `App::at` falhar. Os hooks constroem o `App` a cada chamada de ferramenta e qualquer erro vira "permitir em silêncio"; um erro de YAML num `areas/x.md` editado à mão não pode desligar a trava do projeto inteiro. O erro aparece como uma linha de aviso no `start`, no `next` e no `doctor` (`mapa do projeto ignorado: areas/x.md linha 3: …`), e o fluxo segue sem mapa. `View` ganha `map: Option<Selection>` já selecionada para a etapa e o alvo da vez.
- O alvo por etapa, montado em `service.rs` antes de renderizar: Refinar → `words = run.request` (mais as mudanças do dev); Planejar → `files = brief.files`, `words = brief.understanding`; Construir → `files = step.files`; Verificar → `files` = arquivos do diff do run (`git diff --name-only <base>`), lidos em `measure`-like antes do `next`.
- `render::context` mostra, logo depois de `Convenções do projeto` e antes do que a etapa já mostra:

```text
Mapa do projeto:
  projeto: Node 20 · rodar: node src/cli.js · testar: npm test
    - funções puras em src/, IO só em cli.js
  tarefas (src/tasks.js, src/cli.js) ← depende de armazenamento · libs: commander
    Regras de negócio das tarefas. …
    Testes em test/tasks.test.js, com node:test …
  commander: como o projeto a envolve …
Fora do mapa: src/novo/ (nenhuma área cobre; vale rodar `passo map` depois)
```

- Com mapa, a árvore do item 00 **não** aparece em Refinar e Planejar (o `projeto.md` a substitui). Sem mapa, tudo como hoje.
- Em Construir, com o item 07 da spec anterior, o contexto já é curto; o mapa entra só com as áreas do passo e nunca passa de `map_lines`.
- O painel mostra, na vez do agente, quais áreas foram injetadas (uma linha: `Mapa: tarefas, armazenamento`). O relatório não precisa.

**Arquivos.** `crates/passo-app/src/service.rs`, `crates/passo-app/src/render.rs`, `crates/passo-app/src/snapshot.rs`, `assets/web/app.js`, `crates/passo-app/tests/app.rs`, `docs/ARQUITETURA.md` (parágrafo "O mapa do projeto": o que é, onde mora, que a seleção é pura).

**Pronto quando.** Teste no app com um mapa de três áreas escrito direto em `.passo/map/`: o `next` em Refinar contém a área cujas tags casam o pedido e não contém as outras; em Construir, contém só a área que cobre o arquivo do passo; `Fora do mapa` aparece quando o passo toca um arquivo que ninguém cobre; sem a pasta `map/`, o `next` é o mesmo de antes (o teste existente continua passando).

---

## 05 · Validade: o mapa envelhece e o Passo avisa

**Por quê.** Um mapa errado atrapalha um modelo barato mais do que nenhum mapa. A validade vem do git, não de uma data.

**Desenho.**

- `App::map_staleness(&Map) -> Staleness` (app, IO): para cada área, `git diff --stat <commit>..HEAD -- <paths>` diz se mudou; para o projeto, se mudaram os manifestos (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, os mesmos de `detect_preset`) ou apareceu pasta nova de primeiro nível fora das áreas. Sem git: nada a dizer.
- **No `start`**: se os manifestos mudaram, ou mais de um terço das áreas mudou, a saída do `start` começa com uma linha para o dev e o agente: `Mapa do projeto feito em <commit> (<n> dias): 4 de 9 áreas mudaram desde então. Siga assim, ou peça ao agente para rodar passo map antes.` Não bloqueia. O evento `map.stale` entra no log do run.
- **Na verificação**: tocar uma área não prova que a descrição dela continua certa; o run pode ser justamente o que a mudou. Quem lê o diff e as áreas injetadas é o revisor, então a revisão JSON ganha `mapa: [{ "id": "tarefas", "ok": true, "nota": "" }]`, um item por área que o item 04 injetou em Verificar (`parse_review` recebe a lista de ids esperados e exige todos, como faz com os critérios). `ok: false` pede `nota` dizendo o que mudou. Não custa contexto extra: o revisor já tinha as duas coisas na frente.
- **Na entrega** (dentro de `deliver`, antes do commit do log de auditoria, que passa a se chamar `log de auditoria e mapa`): as áreas que o revisor confirmou (`ok: true`) ganham `commit: <último commit do run>`; as que ele marcou `ok: false` mantêm o `commit` antigo e recebem a `nota` no fim do corpo sob `## Anotações`, como `- (run <id>) <texto>`, no máximo cinco por área, as mais antigas saem. `store::map::save` grava e o caminho entra no `commit_paths` junto com `.passo/audit/<id>`. Com `git.mode: none`, só grava. Em tamanhos com `verificar: auto` não há revisor, e o mapa fica como estava.
- `passo doctor` lista as áreas com `✓` ou `mudou` e o conselho de remapear quando passar do terço.
- `passo map` com um mapa existente mostra ao explorador quais áreas mudaram, para ele atualizar essas e manter as outras (a entrega continua sendo o mapa inteiro: mais simples que diffs de mapa).

**Arquivos.** `crates/passo-core/src/spec.rs` (`Review.map`, `REVIEW_TEMPLATE`), `crates/passo-core/src/flow.rs` (o `Run` guarda as confirmações para a entrega), `crates/passo-app/src/service.rs`, `crates/passo-app/src/render.rs` (`map_task` mostra o que mudou; Verificar lista os ids esperados), `assets/stages/verificar.md`, `crates/passo-cli/src/main.rs` (`doctor`), `crates/passo-app/tests/app.rs`, `README.md` (um parágrafo em "Auditoria e métricas" ou em "Estender": o mapa tem validade por commit).

**Pronto quando.** Teste no app: com um mapa de três áreas, mudar e commitar um arquivo da área A deixa A `mudou` e as outras `✓`; mudar o `package.json` faz o `start` avisar; uma revisão com `mapa: [{id: A, ok: true}]` atualiza o `commit` de A e não o de B; uma revisão com `ok: false` e `nota` deixa o `commit` de A como estava e a nota aparece em `## Anotações` de A; uma revisão sem o item de A é recusada com `falta a área A em \`mapa\``.

---

## 06 · Projetos anexados

**Por quê.** Um pedido muitas vezes depende de outro repositório ao lado (a API, a biblioteca compartilhada). O agente precisa saber como aquele projeto funciona sem o Passo trabalhar nele.

**Desenho.**

- `passo.yaml` ganha:

```yaml
projects:
  - name: api
    path: ../api
```

- `store::map::load` lê também `<path>/.passo/map/` de cada projeto anexado, se existir, e prefixa os ids: `api/armazenamento`. Um `depends_on: [api/armazenamento]` numa área do projeto principal é válido. Os `paths` das áreas anexadas são relativos à raiz do projeto anexado; na seleção, um alvo do projeto principal nunca casa `paths` de um anexado (só chega lá por `depends_on` ou por `tags`/`words`).
- Projetos anexados são **só de leitura** para o Passo: a trava nega escrita neles durante um run e durante o mapeamento. Os caminhos entram no `judge` como raízes extras, só leitura, já canonicalizados pelo `hooks.rs`, do mesmo jeito que a raiz do projeto (no macOS, `/var` e `/private/var` têm que bater). O mapa deles é feito lá dentro, com o `passo map` daquele projeto. O `passo map` do principal mostra ao explorador a árvore de cada anexado e pede que `depende_de` cite as áreas deles quando houver dependência real.
- Sem `.passo/map/` num anexado, o `start` e o `doctor` dizem: `projeto anexado api sem mapa: rode passo map em ../api`. Não bloqueia.
- `render` marca a origem: `api/armazenamento (../api/src/store.ts)`.

**Arquivos.** `crates/passo-core/src/config.rs` (`projects`), `crates/passo-core/src/lock.rs` (raízes só de leitura em `judge`), `crates/passo-store/src/map.rs`, `crates/passo-app/src/service.rs`, `crates/passo-app/src/hooks.rs`, `crates/passo-app/src/render.rs`, `assets/passo.yaml` (bloco comentado `projects:`), `docs/EXTENDER.md`, `crates/passo-app/tests/app.rs`.

**Pronto quando.** Teste no app com dois diretórios temporários: o mapa do anexado é lido com prefixo; uma área do principal com `depends_on: [api/x]` é aceita; em Planejar o salto de dependência traz `api/x`; um hook `Write` num arquivo do anexado é negado durante o run; sem mapa no anexado, o `start` avisa e segue.

---

## Fora desta spec

- Mapear sem agente (gerar áreas por heurística): a árvore mecânica já cobre o que dá para fazer sem modelo.
- Diff de mapa na entrega do explorador: a entrega é sempre o mapa inteiro.
- Métricas do mapeamento em `passo stats`: o log do mapa existe; a tabela fica para depois.
- Escrever no projeto anexado.

## Ordem

`00`, `01`, `02`, `03`, `04`, `05`, `06`. Cada um depende do anterior.
