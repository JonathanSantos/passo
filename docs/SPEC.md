# Spec: Passo para modelos baratos

> **Estado: executada em 2026-10-21**, nos commits `spec-01` a `spec-11` da main. Não rode de novo; o texto fica como registro das decisões.

Esta spec reúne os ajustes que fazem o Passo render mais com modelos baratos, e uma mudança de formato: configuração em YAML e tudo o que é instrução em Markdown. Ela foi escrita para ser executada por um agente, item a item, na ordem em que está.

## Como executar esta spec

- Um item por vez, na ordem. Cada item vira um commit (ou mais, se ficar grande), com a mensagem começando pelo número do item: `spec-01: …`.
- Antes de cada commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Nada entra com warning ou teste quebrado.
- As regras do `CLAUDE.md` valem: código, nomes e comentários em inglês; textos para quem usa em português; o fluxo em `passo-core` é puro (nada de IO, relógio ou git ali; fatos entram num `Command`, efeitos saem como `Effect`); plataformas só em `passo-adapters`.
- Cada item diz quais documentos atualizar. README, `docs/ARQUITETURA.md` e `docs/EXTENDER.md` têm que continuar contando a mesma história que o código.
- Quando a spec disser "decisão" é porque há mais de um jeito e um foi escolhido. Siga o escolhido. Se algo na spec contradisser o código de um jeito que impeça o item, pare e pergunte, em vez de improvisar.
- Nomes de campos novos que o agente escreve (no JSON do plano, da revisão e do pedido refinado) ficam em português, como os que já existem. Nomes de campos de configuração ficam como estão (`default_size`, `max_steps`): eles já eram em inglês e trocá-los agora só gera migração.

## Estado de partida

O código já tem: fluxo determinístico em `passo-core`, log com cadeia de hashes ancorada nos commits (`Passo-Audit:`), trava por hooks, checagens rodadas pelo Passo, um commit por passo, `extends` em camadas, agentes e etapas em `.passo/*.md`, painel e relatório. Há 52 testes. Os itens abaixo acrescentam; nenhum remove uma etapa, um gate ou a aceitação da entrega.

Antes de começar, confira que `git status` está limpo. A âncora da auditoria, a guarda de branch e os outros ajustes recentes podem estar só no working tree: se estiverem, commite-os primeiro, num commit próprio, para que o `spec-01` não os engula.

---

## 01 · Configuração em YAML, instruções em Markdown

**Por quê.** A pessoa que mantém o projeto prefere YAML para dados e Markdown para tudo o que é instrução. Hoje a configuração é TOML (`passo.toml`, presets) e os `.md` usam um leitor de frontmatter caseiro que só entende `chave: valor` e `[a, b]`. Com um parser YAML de verdade, os dois mundos usam a mesma gramática: o frontmatter dos `.md` passa a ser YAML completo (listas em bloco, texto com dois-pontos entre aspas, mapas).

**Decisões.**

- O arquivo de configuração passa a ser `.passo/passo.yaml` (e `~/.passo/passo.yaml`). Os presets embutidos viram `assets/presets/<stack>.yaml`. O modelo que o `init` escreve vira `assets/passo.yaml`.
- Não há compatibilidade com TOML: o projeto ainda não teve release. Se existir um `.passo/passo.toml`, `passo init`, `passo doctor` e `App::open` falham com a mensagem: `o Passo agora lê .passo/passo.yaml; converta o passo.toml (mesmas chaves, sintaxe YAML) e apague o antigo`.
- Os nomes das chaves não mudam. A forma é a mesma, em YAML:

```yaml
# .passo/passo.yaml
extends: [preset:rust, ~/time/passo.yaml]
default_size: m

checks:
  step: [cargo check --all-targets --quiet]
  final: [cargo test --quiet]
  timeout_secs: 600

git:
  mode: branch      # branch | none
  delivery: merge   # merge | keep

limits:
  attempts: 3

sizes:
  p:
    verificar: auto
    approve_plan: false
  pp:
    max_files: 5

models:
  leve: { claude: haiku, copilot: Claude Haiku 5.5 }
  forte: { claude: sonnet, copilot: Claude Sonnet 5.5 }
```

- A regra de camadas continua: padrões do Passo < `~/.passo/passo.yaml` < cada item de `extends` < o arquivo do projeto. Mapas se juntam chave a chave; listas e valores substituem. Chave desconhecida dá erro com o nome dela (`deny_unknown_fields` fica).
- Biblioteca: `serde_yaml` original está arquivada. Use um fork mantido com a mesma API (`serde_yaml_ng` é o candidato; confira no crates.io qual está ativo e fixe a versão no workspace). O merge em camadas passa a operar sobre o `Value` (mapping) dessa biblioteca, com a mesma função `merge` de hoje.
- O frontmatter dos `.md` (`agents/`, `stages/`, `checklists/`) passa a ser lido como YAML pela mesma biblioteca. O leitor caseiro em `crates/passo-store/src/frontmatter.rs` é substituído: `Doc { fields: Mapping, body }`. `text(key)` e `list(key)` continuam existindo com a mesma assinatura, por cima do `Mapping`. Um erro de YAML no frontmatter dá erro com o caminho do arquivo e a linha.
- Valores com dois-pontos precisam de aspas em YAML. Os instaladores (`claude.rs`, `copilot.rs`) já escrevem `description` entre aspas; os assets embutidos devem ser conferidos e corrigidos onde houver `:` solto.
- O que é instrução continua em Markdown, e só nele: agentes (`.passo/agents/<nome>.md`), etapas (`.passo/stages/<etapa>.md`), checklists (`.passo/checklists/<nome>.md`) e convenções (`.passo/context.md`). O frontmatter desses arquivos carrega só os dados que pertencem àquele arquivo (`extends`, `model`, `tools`, `agent`, `checks`, `match`). Números do projeto e dos tamanhos ficam no `passo.yaml`. Esta divisão deve estar escrita no topo de `docs/EXTENDER.md`.
- Interpretação de "adicionar mais ações" (pedido da pessoa que mantém): agentes novos, mudanças em etapas e checklists, que já são `.md`. Se ao executar ficar claro que "ações" queria dizer outra coisa (por exemplo, comandos novos da CLI), pare e pergunte.

**Arquivos.** `Cargo.toml` (workspace: sai `toml`, entra o crate YAML), `crates/passo-store/Cargo.toml`, `crates/passo-store/src/config.rs`, `crates/passo-store/src/frontmatter.rs`, `crates/passo-store/src/assets.rs`, `crates/passo-store/src/layout.rs` (`config()` aponta para `passo.yaml`), `crates/passo-app/src/setup.rs` (modelo de config e substituição do `extends` do preset), `assets/passo.yaml`, `assets/presets/*.yaml`, `crates/passo-app/tests/app.rs` (escreve `passo.yaml`), `examples/simulacao/README.md`, `README.md`, `docs/EXTENDER.md`, `docs/ARQUITETURA.md`.

**Pronto quando.**

- `passo init` escreve `.passo/passo.yaml` com o preset detectado e comentários explicando cada bloco, como o TOML fazia.
- Os testes de `config.rs` passam com arquivos YAML: camadas, `extends` relativo e `preset:`, ciclo, chave desconhecida com o nome.
- O teste de `frontmatter.rs` cobre: lista em linha, lista em bloco, texto com `:` entre aspas, frontmatter ausente, YAML inválido com linha no erro.
- `grep -rn toml crates assets docs README.md` não acha nada além de `Cargo.toml` e `rust-toolchain.toml`.
- Um `.passo/passo.toml` presente produz a mensagem de migração acima.

---

## 02 · Escalar o modelo na falha, antes de chamar o dev

**Por quê.** O construtor é sempre `leve`; na terceira falha o dev é chamado. A alavanca mais barata para modelo barato é gastar o modelo forte só onde o barato travou.

**Desenho.**

- `Profile` ganha `escalate_after: u32`: depois de quantas tentativas falhas de um passo o próximo nível é pedido. `0` desliga. Padrões: `pp: 0` (um ajuste que falha é tamanho errado, não falta de modelo), `p: 1`, `m: 1`, `g: 1`. Configurável em `sizes.<t>.escalate_after`.
- `Step` ganha `tier: Option<String>` (o nível pedido para a tentativa atual; `None` = o do agente). No core, em `step_done`, no ramo de falha: se `escalate_after > 0 && attempts >= escalate_after && step.tier.is_none()`, define `step.tier = Some("forte")` e emite `step.escalated` (`"S2 sobe para o nível forte"`). O feedback ao agente ganha a linha `Esta tentativa usa o nível forte.` Quando o dev dá `ok` num `travado`, `attempts` volta a `0` e o `tier` fica como está: se o barato já travou ali, a próxima tentativa continua com o forte.
- `render::task`: quando o passo tem `tier`, o cabeçalho mostra `# Agente: construtor (nível forte)` e a delegação aponta para `passo-construtor-forte`.
- Instalação (`setup::install` e os adapters): para cada agente cujo nível é `leve`, a plataforma escreve dois arquivos: `passo-<agente>` (modelo do nível `leve`) e `passo-<agente>-forte` (modelo do nível `forte`). `Platform::delegation` recebe o nome já com o sufixo. `passo sync` reescreve os dois.
- Métricas: `step.escalated` conta em `Metrics.escalations`; `passo stats` mostra `Escaladas  N`. O painel mostra a chip no passo.
- Os níveis continuam sendo os de `passo_core::config::TIERS`; "forte" é o segundo. Um projeto com um nível próprio não escala para ele (fora de escopo).

**Arquivos.** `crates/passo-core/src/config.rs`, `crates/passo-core/src/model.rs`, `crates/passo-core/src/flow.rs`, `crates/passo-core/tests/flow.rs`, `crates/passo-app/src/render.rs`, `crates/passo-app/src/setup.rs`, `crates/passo-app/src/metrics.rs`, `crates/passo-adapters/src/{lib,claude,copilot}.rs`, `crates/passo-cli/src/main.rs`, `assets/skill/SKILL.md` (nomes dos subagentes), `assets/web/app.js`, `docs/EXTENDER.md` (tabela dos tamanhos), `README.md` (seção "Por que").

**Pronto quando.** Teste no core: em `m`, a primeira falha de um passo deixa `tier = Some("forte")` e emite `step.escalated`; em `pp` nunca escala; o `ok` no `travado` zera as tentativas e mantém o `tier`. Teste no app: `passo init --agent claude` escreve `.claude/agents/passo-construtor-forte.md` com o modelo de `forte`, e o `next` depois de uma falha em `m` contém `nível forte` e `passo-construtor-forte`.

---

## 03 · Validar a evidência do revisor contra o repositório

**Por quê.** Modelo barato inventa evidência. Hoje `evidencia` é texto livre e nada confere se o arquivo, a linha ou o teste existem.

**Desenho.**

- No core, `spec::evidence_refs(text) -> Vec<EvidenceRef>` (puro): extrai referências do texto da evidência. Formas aceitas: um caminho (`src/x.rs`, `README.md`, com ou sem pasta), um caminho com linha ou faixa (`src/x.rs:42`, `src/x.rs:42-50`), e nomes de teste entre crases (`` `nome_do_teste` ``). O parser só separa candidatos; o que decide se algo é um caminho é existir no disco, não a forma do texto. Um `EvidenceRef` é `{ path: Option<String>, line: Option<u32>, symbol: Option<String> }`.
- Regra: para cada critério com `ok: true`, a evidência precisa de pelo menos uma referência que o Passo confirme, e nenhuma referência errada: uma linha que não existe, um caminho fora do projeto ou uma pasta recusam a evidência inteira, mesmo ao lado de uma referência certa (revisão: `docs/SPEC-REVISAO.md`, item 01). Para `ok: false`, a evidência é a explicação e não é conferida.
- A conferência é IO e fica no app (`crates/passo-app/src/service.rs`, no ramo `Verificar` de `done`, depois de `parse_review`): um `path` precisa existir no projeto (relativo à raiz) e, com `line`, a linha precisa existir no arquivo; um `symbol` precisa aparecer em algum arquivo que case com `tests.patterns` ou nos arquivos que o diff do run tocou (use `git grep -l --fixed-strings` limitado a esses caminhos, ou leitura direta quando não há git).
- Erros são precisos e vão pelo mesmo caminho de `Rejected` das entregas inválidas, com um evento `output.invalid`: `` `criterios[1].evidencia` cita src/task.rs:420, mas o arquivo tem 180 linhas `` · `` `criterios[0].evidencia` não cita arquivo, linha nem teste; aponte onde está a prova (arquivo:linha ou `nome_do_teste`) ``.
- `assets/stages/verificar.md` e `REVIEW_TEMPLATE` passam a dizer o formato: `"evidencia": "tests/x_test.rs:12 `prova_tal`; src/x.rs:42"`.

**Arquivos.** `crates/passo-core/src/spec.rs` (parser + testes), `crates/passo-app/src/service.rs`, `crates/passo-app/tests/app.rs`, `assets/stages/verificar.md`, `assets/agents/revisor.md`, `README.md` (uma linha em "Por que").

**Pronto quando.** Teste puro do parser com as quatro formas e com texto sem referência. Teste no app: revisão com `ok: true` apontando linha inexistente é recusada com a mensagem acima e conta como `invalid_outputs`; revisão apontando um teste que existe passa.

---

## 04 · O Passo roda o teste do passo

**Por quê.** `require_tests` só confere que um arquivo de teste mudou. Se o passo trouxer o comando do teste, o Passo roda esse teste sozinho no `done`. É a versão leve dos testes escondidos do MOHs: ninguém esconde nada, mas quem diz que passou é o Passo.

**Desenho.**

- `PlanStep` ganha `rodar: String` (opcional): o comando que prova o passo, por exemplo `npm test -- --grep prioridade` ou `cargo test task::`. `teste` continua sendo a descrição. `PLAN_TEMPLATE` e `assets/stages/planejar.md` mostram os dois; a instrução do planejador diz: `rodar` é o comando mais estreito que roda só esse teste.
- `Step` ganha `rodar: String`. Em `measure`, quando o passo da vez tem `rodar`, o Passo roda esse comando antes das checagens do passo, com o mesmo `timeout_secs`, e o resultado entra em `facts.checks` na frente, com `command = rodar`. Falha conta como falha de checagem (mesma tentativa, mesmo feedback).
- **`rodar` é o primeiro comando escrito pelo agente que o Passo executa, então ele passa pela trava.** Em `submit_plan` (core, puro), cada `rodar` é julgado por `lock::judge` como um `ToolUse` de tipo `Exec` com aquele comando; se a trava nega (`passo ok`, `passo stop`, `git commit`, `.passo/`…), o plano é recusado com o motivo da trava: `` `passos[1].rodar` não pode ser "passo size g": `passo size` é decisão do dev ``. Além disso, um plano com qualquer `rodar` sempre abre o gate `aprovar`, mesmo com `approve_plan: false`, do mesmo jeito que perguntas em aberto já abrem. O texto do gate diz: `Aprovar também autoriza o Passo a rodar os comandos de teste listados nos passos.` O plano automático (`pp`) nunca tem `rodar`.
- `render` em Construir mostra `Teste (escreva primeiro): <teste> · o Passo roda: <rodar>`.
- Passos de correção criados pela verificação (`add_fix_steps`) herdam o `rodar` do critério quando o item 05 estiver feito; até lá ficam vazios.
- O relatório e o painel mostram o `rodar` junto do passo.

**Arquivos.** `crates/passo-core/src/spec.rs`, `crates/passo-core/src/model.rs`, `crates/passo-core/src/flow.rs` (copiar `rodar` ao aceitar o plano), `crates/passo-app/src/service.rs` (`measure`), `crates/passo-app/src/render.rs`, `assets/stages/planejar.md`, `assets/agents/planejador.md`, `assets/web/app.js`, `docs/EXTENDER.md` não muda, `README.md` (tabela das etapas: "cada um com o teste que o prova e o comando que o roda").

**Pronto quando.** Teste no app: um plano com `rodar: "test -f tests/x.txt"` falha o `done` enquanto o arquivo não existe (feedback mostra o comando) e passa depois. Testes no core: `rodar` sobrevive do `PlanStep` ao `Step`; um plano com `rodar: "passo stop"` é recusado com o motivo da trava; um plano com `rodar` abre o gate `aprovar` num tamanho com `approve_plan: false`.

---

## 05 · Critérios amarrados aos passos

**Por quê.** O plano lista critérios e passos soltos. Para modelo barato, planejar com a amarração explícita é mais fácil, o Passo detecta critério órfão, e o revisor recebe o mapa pronto.

**Desenho.**

- `PlanStep` ganha `criterios: Vec<String>`: ids dos critérios que o passo atende (`C1`, `C2`; aceitar também o número `1`, `2` e normalizar). `Step` ganha o mesmo campo.
- Validação em `parse_plan` (puro, recebe o número de critérios): referência a critério inexistente é erro (`` `passos[2].criterios` cita C5, mas há 3 critérios ``). Validação em `submit_plan` (core): quando o plano vem do agente, todo critério precisa de pelo menos um passo (`o critério C2 não tem passo que o atenda; acrescente um passo ou junte C2 a outro critério`). No plano automático (`planejar: auto`) o único passo atende todos.
- `submit_review`: a correção criada para um critério reprovado ganha `criterios = [id]` e herda o `rodar` do último passo que atendia esse critério, se houver.
- `render`: em Construir, o passo mostra `Atende: C1, C3` com o texto dos critérios; em Verificar, cada critério mostra os passos que o atenderam e os commits deles (`C1 ← S1 [a1b2c3d], S3 [e4f5a6b]`).
- Painel e relatório mostram a amarração nos dois sentidos.

**Arquivos.** `crates/passo-core/src/spec.rs`, `crates/passo-core/src/model.rs`, `crates/passo-core/src/flow.rs`, `crates/passo-core/tests/flow.rs`, `crates/passo-app/src/render.rs`, `assets/stages/planejar.md`, `assets/stages/verificar.md`, `assets/web/app.js`, `README.md`.

**Pronto quando.** Testes no core: critério órfão em `m` é recusado com a mensagem; referência inválida em `parse_plan` é recusada; correção da revisão carrega o critério. O teste de ponta a ponta do app usa um plano com `criterios` nos passos.

---

## 06 · Mapa do repositório no contexto

**Por quê.** `context.md` é manual e fica vazio. Modelo barato gasta a maior parte dos tokens descobrindo onde as coisas estão.

**Desenho.**

- `crates/passo-app/src/repomap.rs` (novo): `fn build(root, git: Option<&Git>, limit_lines) -> String`. Conteúdo, nesta ordem, até 60 linhas: (1) árvore de pastas até dois níveis com a contagem de arquivos em cada uma, a partir de `git ls-files` (ou `read_dir` quando não há git), sempre ignorando `.git`, `.passo` (o `audit/` fica no git e apareceria), `target`, `node_modules`, `dist` e `build`; (2) os 10 maiores arquivos de código por linhas; (3) onde ficam os testes (pastas que casam com `tests.patterns`); (4) as checagens configuradas (`checks.step`, `checks.final`). Nada de conteúdo de arquivo.
- O mapa é gerado no `start` e guardado em `.passo/runs/<id>/map.md` (fora do git, como o resto de `runs/`); regenerado em `change`. É um fato do mundo, então nunca é lido pelo core.
- `render::context` mostra `Mapa do repositório:` em Refinar e Planejar. Em Construir e Verificar não (o passo já diz os arquivos).
- `.passo/context.md` continua acima do mapa quando existe. O modelo de `context.md` que o `init` escreve ganha a frase: "o Passo já mostra a árvore de pastas e os testes; escreva aqui o que a árvore não diz".

**Arquivos.** `crates/passo-app/src/repomap.rs`, `crates/passo-app/src/lib.rs`, `crates/passo-app/src/service.rs`, `crates/passo-app/src/render.rs` (a `View` recebe o mapa), `crates/passo-store/src/layout.rs` (`map(id)`), `crates/passo-app/src/setup.rs`, `crates/passo-app/tests/app.rs`, `docs/ARQUITETURA.md` (uma linha na tabela do `passo-app`).

**Pronto quando.** Teste no app: depois de `start`, o `next` em Refinar contém `Mapa do repositório` e o nome da pasta `src`; o mapa tem no máximo 60 linhas num projeto grande sintético (crie 200 arquivos no teste).

---

## 07 · Menos contexto para o construtor

**Por quê.** O `next` em Construir repete o plano inteiro, todas as notas e todos os critérios em todo passo. É onde o modelo barato mais se perde.

**Desenho.** Em `render::context`, ramo `Construir`:

- Trocar `plan_overview` por: título do plano; o passo anterior feito (id, título, resumo); o passo da vez completo (com `Atende:` e `rodar`); o próximo passo (id e título). Mais nada do plano.
- Critérios: só os que o passo atende (item 05). Se o passo não declara nenhum, mostrar todos, como hoje.
- Notas: só as que começam com o id de um passo já feito e citam um arquivo do passo da vez, as que começam com `Revisão:` e as cinco mais recentes. Nunca mais que dez.
- `Decisões do dev` e `Mudanças do dev no pedido` continuam inteiras: são curtas e mandam mais que o pedido.
- O ramo `Verificar` não muda (o revisor precisa do todo).
- `passo plan` continua mostrando tudo: é a saída para o dev.

**Arquivos.** `crates/passo-app/src/render.rs`, `crates/passo-app/tests/app.rs`.

**Pronto quando.** Teste no app com um plano de quatro passos: o `next` do passo 2 não contém o título do passo 4 e contém o do passo 3; com doze notas, o `next` mostra no máximo dez.

---

## 08 · Orçamento de ações por passo

**Por quê.** Modelo barato entra em loop de exploração e nada corta. O log já conta cada chamada de ferramenta.

**Desenho.**

- `Profile` ganha `max_tool_calls: u32` por passo (`0` desliga). Padrões: `pp: 30`, `p: 60`, `m: 80`, `g: 120`.
- O hook `PreToolUse` conta: `Store::count_tool(run_id, key)` com `key = "<stage>:<step_id>:<attempts>"`, no mesmo molde de `count_stop` (arquivo `budget.json` em `runs/<id>/`, fora do git). Em etapas sem passo (`Refinar`, `Planejar`, `Verificar`), a chave é a etapa, com o mesmo limite.
- Canal do aviso: confira na referência de hooks do Claude Code se `PreToolUse` aceita `additionalContext` em `hookSpecificOutput`. Se não aceitar (hoje a referência lista só `permissionDecision`, `permissionDecisionReason` e `updatedInput` para esse evento), acrescente `PostToolUse` a `HookEvent` e aos instaladores, e mande o aviso por ele, que aceita `additionalContext`. O `Platform::render` decide o dialeto, como hoje.
- Ao passar do limite, o hook **não nega**: responde `Context` com `Passo: já foram N ações neste passo (limite M). Entregue com passo done, ou pergunte com passo ask se travou.` A cada 20 ações além do limite repete o aviso. Emite o evento `step.budget` uma vez por chave.
- Mudar de tentativa (falha de checagem) zera a conta, porque a chave inclui `attempts`.
- `passo stats` mostra `Ações por passo  média N · máximo M (Sx)`. O painel mostra a conta no passo da vez.

**Arquivos.** `crates/passo-core/src/config.rs`, `crates/passo-store/src/runs.rs`, `crates/passo-store/src/layout.rs`, `crates/passo-app/src/hooks.rs`, `crates/passo-app/src/metrics.rs`, `crates/passo-cli/src/main.rs`, `assets/web/app.js`, `docs/EXTENDER.md` (tabela dos tamanhos).

**Pronto quando.** Teste no app: com `max_tool_calls = 3`, a quarta chamada ao hook retorna o aviso, a terceira não; há um evento `step.budget` no log; a trava continua negando o que negava.

---

## 09 · Comandos do dev recusados no ambiente do agente, sem depender de hook

**Por quê.** A trava depende de hooks. Subagentes, na simulação, passaram por fora. O Claude Code define `CLAUDECODE=1` no ambiente das ferramentas e subagentes herdam.

**Desenho.**

- `Platform` ganha `fn agent_env(&self, env: &dyn Fn(&str) -> Option<String>) -> bool`: recebe a leitura do ambiente por função, para ser testável sem mexer em variáveis do processo (`std::env::remove_var` é `unsafe` na edição 2024 e o workspace proíbe `unsafe`). Claude: `CLAUDECODE` definida; Copilot: `false` por enquanto (confira se o VS Code define algo como `VSCODE_PID` dentro do terminal do agente; se não houver sinal confiável, deixe `false` e anote no código).
- A recusa fica **só na CLI**, nos subcomandos `ok`, `no`, `change`, `size` e `stop`, nunca em `App::decide`: a aprovação pelo chat roda dentro do hook `UserPromptSubmit`, que vive no ambiente do agente e negaria a si mesma. Se alguma plataforma do registry responde `agent_env()`, a CLI recusa com `Rejected` e a mesma mensagem da trava (`passo <cmd> é decisão do dev…`), gravando `lock.denied` no log com `data.via = "cli"`. A variável `PASSO_DEV=1` desliga a recusa, para o dev que roda a CLI de propósito dentro do terminal do agente; isso fica documentado no README em "Você decide só o que importa".
- `passo doctor` mostra `Ambiente de agente: sim (Claude Code)` quando detecta.

**Arquivos.** `crates/passo-adapters/src/{lib,claude,copilot}.rs`, `crates/passo-cli/src/main.rs`, `crates/passo-app/src/service.rs` (ou um helper no app para gravar o evento), `README.md`, `docs/ARQUITETURA.md` (seção "A trava").

**Pronto quando.** Teste puro em `claude.rs` da função com um ambiente sintético. Teste de integração na CLI (`crates/passo-cli/tests/`), rodando o binário com `Command::new(env!("CARGO_BIN_EXE_passo")).env_clear().env("CLAUDECODE", "1")`: `passo ok` sai com código 2 e a mensagem; com `PASSO_DEV=1` também no ambiente, passa. Os testes de `passo-app` não mudam: a verificação não está no app.

---

## 10 · Aceitar a entrega pelo chat pede uma palavra própria

**Por quê.** "ok" é a palavra mais comum do idioma e hoje faz merge em `main`. Para aprovar plano é ótimo; para entregar é permissivo demais.

**Desenho.**

- `parse_chat(message, gate: GateKind)` (core, puro). No gate `aceitar`, as formas de aceite são `aceito`, `aceita`, `aceitar`, `ok entrega`, `ok, entrega`, `pode entregar`. Um `ok` solto nesse gate retorna `ChatDecision::Hint("Para aceitar a entrega, diga `aceito`; para recusar, `não: o que mudar`.")`, e o hook responde isso como `Context`, sem decidir nada. Nos outros gates nada muda.
- `render::gate` no `aceitar` mostra `aceito` em vez de `ok` na lista de respostas. O painel e `passo status` idem.

**Arquivos.** `crates/passo-core/src/answers.rs`, `crates/passo-app/src/hooks.rs`, `crates/passo-app/src/render.rs`, `assets/web/app.js` (texto), `README.md` (bloco de respostas), `assets/skill/SKILL.md`.

**Pronto quando.** Testes em `answers.rs` com os dois gates. Teste no app: no gate `aceitar`, o prompt `ok` não encerra o run e a resposta do hook contém `aceito`; o prompt `aceito` encerra.

---

## 11 · Simulação repetível

**Por quê.** A tese do projeto ("plano forte, construção barata") está sustentada por uma rodada de cada braço. O número que vende o Passo é o de N rodadas.

**Desenho.**

- `examples/simulacao/rodar.sh <modelo> <n>`: para cada rodada, copia `examples/tarefas` para uma pasta temporária, `git init` + commit, `passo init --agent claude`, escreve o `passo.yaml` e o `context.md` do protocolo, e roda o agente de forma não interativa (`claude -p "<prompt do README da simulação>" --model <modelo>`; confira os flags atuais da CLI do Claude Code e deixe-os numa variável no topo do script). `claude -p` termina quando o agente para na vez do dev, então o laço principal é: rodar o agente, esperar sair, ler `passo status`; se o run terminou ou foi interrompido, acabou; senão, aplicar a regra do dev abaixo e rodar o agente de novo na mesma sessão (`claude -p --continue "o dev respondeu; rode passo next"`), até terminar. A regra fixa do protocolo: num gate `aprovar` ou `pergunta` responde `passo ok`; num `aceitar` responde `passo ok` se todas as checagens passaram e a revisão não reprovou critério, senão `passo no "<primeiro critério reprovado>"`; num `travado` responde `passo ok` uma vez e `passo stop` na segunda. Quando o run termina, roda a suíte escondida (`ARM_DIR=… node --test examples/simulacao/aceite.test.js`) e guarda `passo stats --json` e o resultado da suíte em `resultados/<modelo>/<n>.json`.
- `examples/simulacao/tabela.js`: lê `resultados/` e imprime a tabela do README (mediana e intervalo de cada linha), em Markdown.
- O README da simulação explica os dois scripts e diz que a tabela do README principal deve ser regenerada por eles, nunca escrita à mão.
- O script é "melhor esforço": se a CLI do agente mudar, ele é o único lugar a ajustar.

**Arquivos.** `examples/simulacao/rodar.sh`, `examples/simulacao/tabela.js`, `examples/simulacao/README.md`, `.gitignore` (`examples/simulacao/resultados/`).

**Pronto quando.** `sh -n rodar.sh` passa; `node tabela.js` com dois arquivos de resultado sintéticos imprime a tabela; o README do exemplo descreve o fluxo. Rodar a simulação de verdade não faz parte desta spec: depende de chaves e custo.

---

## Fora desta spec

- Compatibilidade com `passo.toml` (não há release).
- Um nível de modelo além de `leve` e `forte` na escalada.
- Rodar o teste do passo antes da implementação para ver falhar (o agente faz as duas coisas numa vez só).
- Detectar o ambiente do Copilot sem um sinal confiável.

## Ordem e dependências

`01` primeiro (muda a configuração que `02` e `08` leem). Depois `05` e então `04` (a correção da revisão herda o `rodar` pelo critério). `02`, `03`, `06`, `07`, `08`, `09` e `10` são independentes entre si e vêm em qualquer ordem. `11` por último.
