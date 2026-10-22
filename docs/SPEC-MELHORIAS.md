# Spec: a trava mais firme, o log sem segredos, e quatro coisas novas

> **Estado: executada em 2026-10-21**, nos commits `mel-01` a `mel-07` da main. Não rode de novo; o texto fica como registro das decisões.

Sete itens. Os três primeiros fecham buracos que a revisão apontou: a trava lê comandos de shell por `split`, o log leva segredos para o relatório, e um hook que falha desliga a trava em silêncio. Os quatro seguintes acrescentam: `passo learn`, orçamento de tokens, `passo stats --all` e cobertura do diff como prova.

Regras de execução iguais às de `docs/SPEC.md`: um item por vez, na ordem; um commit por item com prefixo `mel-NN`; `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` antes de cada commit; `CLAUDE.md` vale (inglês no código, português para quem usa, core puro, plataformas só em `passo-adapters`). Esta spec parte do código depois de `docs/SPEC-MAPA.md` e `docs/SPEC-REVISAO.md`. Se uma delas ainda não rodou, pare e pergunte: os itens 01 e 02 mexem em `lock.rs` e `service.rs`, que as duas também tocam.

Cada item termina com **Resultado esperado**: o que muda para quem usa, e o que os testes provam.

---

## 01 · A trava lê shell de verdade

**Por quê.** `lock::dev_command_in` e `git_write_in` quebram o comando em espaços e `;&|`. Com aspas ou substituição, a comparação `== "passo"` nunca acontece: `sh -c 'passo stop'`, `echo $(passo ok)`, `` `git commit -m x` `` e `"passo" ok` passam. Redirecionamento também passa: `echo x > src/a.rs` numa etapa só de leitura só é pego depois, pela conferência do git, que não vê `passo ok` nem `cat .env`.

**Como alterar.**

- Novo módulo puro `crates/passo-core/src/shell.rs`, sem dependência nova: um lexer POSIX pequeno, escrito à mão. Entrada: o texto do comando. Saída:

```rust
pub struct Script { pub commands: Vec<SimpleCommand>, pub readable: bool }
pub struct SimpleCommand {
    pub words: Vec<Word>,            // programa e argumentos, já sem aspas
    pub redirects: Vec<Redirect>,    // `> f`, `>> f`, `< f`, `2> f`
    pub assignments: Vec<(String, Word)>, // `VAR=x` antes do programa
}
pub struct Word { pub text: String, pub dynamic: bool } // dynamic: tinha `$`, `~`, `*`, `?` fora de aspas simples
pub struct Redirect { pub target: Word, pub write: bool }
pub fn lex(command: &str) -> Script
```

- O lexer entende: aspas simples e duplas; `\` de escape; separadores `;`, `&&`, `||`, `|`, `&`, nova linha, `(` e `)`; substituições `$(…)` e crases, que viram comandos próprios na lista (recursivo); `${…}` e `$VAR` marcam a palavra como `dynamic`; redirecionamentos com e sem número de descritor; comentários `#`; **heredocs** `<<` e `<<-` com delimitador com ou sem aspas, cujo corpo até a linha do delimitador é dado (nunca comando, nunca caminho) e deixa o comando legível. Heredoc importa porque é um caminho de entrega documentado: a skill manda escrever o JSON num arquivo temporário, e modelos baratos fazem isso com `cat > /tmp/x.json <<'EOF'` ou `passo done - <<'EOF'`. Aspas sem par, `$(` sem fechar, heredoc sem delimitador final e qualquer construção que o lexer não reconhece deixam `readable = false`, sem pânico.
- Programas que recebem outro comando viram comandos próprios na lista: `sh|bash|zsh|dash -c <palavra>` (a palavra é lexada de novo), `eval <palavras>`, `xargs <programa…>`, `env`, `sudo`, `nohup`, `time`, `command`, `exec`, `nice` (prefixos: o programa real é a palavra seguinte, pulando opções e `VAR=x`).
- `dev_command_in` e `git_write_in` passam a usar `lex`: o programa é `words[0].text` com o caminho tirado, comparado com `passo`/`git` em todos os comandos da lista, inclusive os nascidos de `$(…)` e `sh -c`. `git -C x` e `-c k=v` continuam sendo pulados como hoje.
- Regra do que não dá para ler, aplicada em `judge` **só com run ativo ou mapeamento em andamento**: se `readable == false`, ou se algum `words[0]` é `dynamic` (`$p ok`), e o texto inteiro contém a palavra `passo` ou `git`, nega com `não consigo ler este comando (aspas ou substituição no nome do programa); chame o passo e o git diretamente, sem variáveis`. Comando ilegível sem `passo` nem `git`: em etapa só de leitura e na vez do dev, nega com `nesta etapa só rodam comandos que a trava consegue ler`; em Construir, permite (a conferência do git cobre a escrita).
- Caminhos de argumentos e de redirecionamentos entram na trava: `ToolUse` ganha `cwd: Option<PathBuf>` (o adapter preenche com o `cwd` do payload, já canonicalizado como faz com a raiz). Em `judge`, para cada comando: os `targets` de redirecionamento de escrita são caminhos de **escrita** (resolvidos contra `cwd`, sem IO: só `join` e normalização de `.`/`..`) e passam pelas mesmas regras de `ToolKind::Write`; as palavras que parecem caminho (contêm `/` ou começam com `.`) são caminhos de **leitura** e passam pelas regras de `.passo/` e de `.env`. Assim `cat .env` e `echo x > src/a.rs` são negados na hora, pelo motivo certo.
- `rodar` (item 02 da `SPEC-REVISAO`) usa o mesmo lexer para a regra "uma chamada simples": `commands.len() == 1`, sem redirecionamento, `readable`, nenhuma palavra `dynamic`.

**Arquivos.** `crates/passo-core/src/shell.rs` (novo, com os testes), `crates/passo-core/src/lib.rs`, `crates/passo-core/src/lock.rs`, `crates/passo-core/src/spec.rs` (`rodar`), `crates/passo-adapters/src/payload.rs` (`cwd` no `ToolUse`), `crates/passo-app/src/hooks.rs`, `docs/ARQUITETURA.md` (seção "A trava": um parágrafo sobre o lexer e a regra do ilegível).

**Resultado esperado.** Para quem usa: nenhuma mudança no caminho feliz; o agente que tenta contornar lê um motivo que explica o que fazer. Testes no core, em tabela: negados durante um run `sh -c 'passo stop'`, `echo $(passo ok)`, `` x=`git commit -m a` ``, `"passo" no "x"`, `p=passo; $p ok`, `cat .env`, `cat ./.env.local`, `echo x > src/a.rs` em Refinar, `xargs passo ok`, `eval "passo ok"`; permitidos `passo done "ok"`, `git diff`, `grep passo README.md`, `echo "passo ok" > /tmp/nota.txt` (fora do projeto), `cat .env.example`, `npm test -- --grep 'ok'`, `passo done - <<'EOF'\n{…}\nEOF` em Refinar, `cat > "$(mktemp)" <<EOF\n{…}\nEOF` em Planejar; sem run ativo tudo é permitido menos `passo <dev>` como hoje. Robustez: um corpus fixo de 200 strings malformadas (aspas, `$(`, heredoc sem fim, gerado por um gerador com semente fixa dentro do teste) não entra em pânico; o teste é determinístico.

---

## 02 · Nada de segredo no log nem no relatório

**Por quê.** `hooks::describe` grava os primeiros 140 caracteres de cada comando no log; `ShellRunner` guarda a cauda de 40 linhas de cada checagem no `run.json`, no feedback ao agente e no relatório; e `report.html` é o que a documentação manda anexar num PR. `curl -H "Authorization: Bearer …"`, `export TOKEN=…` e uma saída de teste que imprime um `.env` vão parar lá.

**Como alterar.**

- Novo módulo puro `crates/passo-core/src/redact.rs`, sem `regex`: `pub fn redact(text: &str) -> String`. Regras, por palavra e por linha:
  - Chave seguida de valor: se uma palavra, sem distinguir maiúsculas, termina em `token`, `secret`, `password`, `passwd`, `pwd`, `api_key`, `apikey`, `api-key`, `auth`, `authorization`, `credential`, `credentials`, `cookie`, `session`, `private_key`, `access_key`, e é seguida de `=` ou `:` e um valor, o valor vira `«oculto»` (`TOKEN=abc`, `password: abc` em YAML/JSON, `Authorization: Bearer abc`, onde `Bearer abc` inteiro é o valor). Separado por espaço só em duas formas: a chave é uma opção (`--token abc`, `-p abc` quando a opção longa correspondente está na lista) ou um cabeçalho HTTP (`-H "Authorization: …"`). Uma palavra comum seguida de espaço nunca é chave: `passo done "token de sessão"` fica como está.
  - Prefixos conhecidos: palavras que começam com `ghp_`, `gho_`, `github_pat_`, `sk-`, `sk_live_`, `sk_test_`, `xox[abprs]-`, `AKIA` (16 maiúsculas/dígitos a seguir), `AIza`, `-----BEGIN` até `-----END` (bloco inteiro) viram `«oculto»`.
  - Linhas que são conteúdo de `.env`: uma linha `NOME_EM_MAIUSCULAS=valor` com valor de 16+ caracteres vira `NOME_EM_MAIUSCULAS=«oculto»`.
  - Nunca mexe no resto: nomes de arquivo, comandos, números pequenos.
- `Config` ganha `audit.secrets: Vec<String>`: palavras extras tratadas como chave (por exemplo `minha_chave`). Em `docs/EXTENDER.md`.
- Aplicação, em todos os pontos onde texto do agente ou da shell entra em algo persistente: `hooks::describe` e o `data.call` do evento `tool`; `ShellRunner::run` na cauda da saída (antes de virar `CheckResult.output`, que vai para `run.json`, feedback e relatório); `service::invalid` nos `problems`; `Git::output` nos erros que viram eventos. Cinto e suspensório: `HtmlReporter::write` e `/api/state` aplicam `redact` em todo `text` e `data` de evento e em todo `CheckResult.output` do snapshot, para um log antigo também sair limpo.
- `passo log` mostra o texto já gravado (redigido); não há "modo cru".

**Arquivos.** `crates/passo-core/src/redact.rs` (novo, com testes), `crates/passo-core/src/lib.rs`, `crates/passo-core/src/config.rs`, `crates/passo-app/src/hooks.rs`, `crates/passo-app/src/infra.rs`, `crates/passo-app/src/service.rs`, `crates/passo-app/src/snapshot.rs` (ou `passo-web/src/lib.rs`), `assets/passo.yaml` (bloco `audit:` comentado), `docs/EXTENDER.md`, `README.md` (uma frase em "Auditoria e métricas": o log e o relatório não guardam valores de token, senha ou chave).

**Resultado esperado.** Para quem usa: um relatório que pode ir num PR sem revisão manual. Testes: tabela no core com os casos acima e com falsos positivos que devem ficar intactos (`passo done "token de sessão"`, `src/auth/login.rs`, `AUTH_MODE=oidc`); no app, um hook `Bash` com `curl -H "Authorization: Bearer abc123def456"` gera evento com `«oculto»` e sem `abc123`; uma checagem que imprime `API_KEY=0123456789abcdef0123` deixa o `run.json` e o relatório sem o valor.

---

## 03 · Hook que falha não fica mudo

**Por quê.** Qualquer erro no hook vira "permitir", e nada avisa. Um `passo` fora do PATH do processo do agente (lançado de um atalho gráfico), um `passo.yaml` com chave errada ou um binário antigo no caminho absoluto gravado pelo `init` desligam a trava inteira, e o dev só descobre quando algo passa.

**Como alterar.**

- **Erros ficam registrados.** Em `hooks::handle`, todo `Err` grava uma linha em `.passo/hook-errors.log` (fora do git; `{t, event, platform, erro}`), além do `PASSO_DEBUG`. Essa gravação não passa por `App::at`, que é muitas vezes o que falhou (um `passo.yaml` inválido): usa só `Layout::find` e escrita direta, com a mesma trava de arquivo do log de auditoria, porque hooks rodam ao mesmo tempo. O arquivo é truncado a 200 linhas. `passo next`, `passo status` e o painel mostram, quando o arquivo tem linhas mais novas que o início do run: `⚠ 3 erros de hook neste run (último: configuração inválida: …). A trava pode não estar valendo; rode passo doctor.` O evento `hook.failed` entra no log do run uma vez por motivo distinto, escrito por `EventLog` direto a partir do `Layout` (sem `App::at`), quando há um `current` apontando para um run.
- **`passo doctor` testa o hook de ponta a ponta.** Para cada plataforma instalada, o adapter devolve o comando de hook gravado (`Platform::hook_command(root, event) -> Option<String>`, lendo o próprio arquivo que `install` escreveu). O `doctor`:
  1. Resolve o programa do comando: se é relativo (`passo`), procura no PATH do shell atual e diz onde achou; se é absoluto, confere que existe e roda `<programa> --version`, comparando com a versão deste binário.
  2. Roda o comando com `sh -c`, com `stdin` = um payload sintético de `PreToolUse` para `Bash` com `command: "passo ok"` e `cwd` = a raiz, num ambiente mínimo (`PATH` atual, `HOME`, `PASSO_HOME`), com tempo limite de 5 s. Espera na saída um JSON com `permissionDecision: deny`.
  3. Repete com `UserPromptSubmit` e `prompt: "status?"`, esperando saída vazia.
  4. Confere que o arquivo de hooks da plataforma tem os cinco eventos com o matcher certo.
  Imprime, por plataforma: `Hook (Claude Code): ✓ responde em 14 ms · binário /Users/x/.local/bin/passo 0.1.0 · 5 eventos` ou `✗ comando "passo" não está no PATH deste shell. Se o agente é aberto de um atalho, ele tampouco acha: rode passo init de novo, que grava o caminho absoluto` ou `✗ o hook respondeu "permitir" a "passo ok": a trava não está valendo (erro: …)`.
- **Pacto do `init`**: `hook_bin()` continua preferindo `passo` quando está no PATH; `doctor` é quem diz quando isso não basta. A mensagem final do `init` passa a terminar com `Confira com: passo doctor`.

**Arquivos.** `crates/passo-app/src/hooks.rs`, `crates/passo-store/src/layout.rs` (`hook_errors()`), `crates/passo-adapters/src/{lib,claude,copilot}.rs` (`hook_command`), `crates/passo-app/src/service.rs` (aviso em `next`/`status`, evento), `crates/passo-app/src/snapshot.rs`, `assets/web/app.js` (faixa de aviso), `crates/passo-cli/src/main.rs` (`doctor`, `init`), `crates/passo-app/tests/app.rs`, `README.md` ("Começar": `passo doctor` depois do `init`), `docs/ARQUITETURA.md` (seção "A trava": o hook falha para permitir, e como isso aparece).

**Resultado esperado.** Para quem usa: a trava nunca some em silêncio; `doctor` prova que o hook responde. Testes: no app, um hook chamado com `passo.yaml` inválido grava a linha em `hook-errors.log` e o `next` seguinte traz o aviso; `doctor` com a plataforma instalada e o binário de teste no PATH (`CARGO_BIN_EXE_passo`) imprime `✓ responde`; com o comando do hook apontando para um caminho inexistente, imprime `✗`.

---

## 04 · `passo learn`: o que o dev repete vira convenção

**Por quê.** Depois de alguns runs, as recusas (`não: …`), as mudanças no pedido, as dicas e as respostas diferentes da recomendada se repetem. Cada uma custou uma vez do dev. O Passo tem tudo registrado em `run.changes` e nas `observacoes` do revisor, mas nada fecha o ciclo.

**Como alterar.**

- **Colheita, pura e determinística**, em `crates/passo-core/src/learn.rs`: `collect(runs: &[Run], seen: &[String]) -> Vec<Lesson>`. Entrada: os runs concluídos ou interrompidos e as impressões digitais já tratadas. Uma `Lesson` é `{ fingerprint, kind: ChangeKind | Note, text, run_id, at, stage, files: Vec<String> }`. A impressão digital é FNV do `run_id + kind + text` (a mesma função de `fingerprint` do plano). `files` vem do passo em andamento na hora da mudança, quando houver: para isso `Change` ganha `step: Option<String>` (`#[serde(default)]`), gravado pelo motor em `record` com o id do passo em andamento. Mudanças antigas, sem o campo, ficam sem `files`. `group(lessons) -> Vec<Group>`: lições que compartilham duas ou mais palavras de 5+ letras (sem acentos, sem as palavras de parada de `make_id`) ficam no mesmo grupo; um grupo com lições de dois ou mais runs é **recorrente**. Nada de modelo aqui.
- **A tarefa para o agente**, como `passo map`: `passo learn` recusa se há run ou mapeamento ativo; grava `.passo/learn/.task.json`; imprime a tarefa para o agente `refinador`, delegada ao gêmeo `passo-refinador-forte` (o mecanismo do item 02 da `SPEC.md`): os grupos recorrentes primeiro, depois as lições soltas, cada uma com run, data, etapa e arquivos; o `context.md` atual; as áreas do mapa que os arquivos tocam (quando há mapa); e o JSON a entregar:

```json
{
  "propostas": [
    {
      "texto": "Listagens de CLI saem em ordem de criação, nunca alfabética",
      "destino": "context",
      "fontes": ["run prioridade-tarefas-3f2a: recusa: a lista veio em ordem alfabética", "run cor-botao-91ab: dica: ordem de criação"]
    },
    { "texto": "...", "destino": "mapa:tarefas", "fontes": ["..."] },
    { "texto": "...", "destino": "agente:construtor", "fontes": ["..."] }
  ]
}
```

- Validação (`spec::parse_learn`, puro): até 10 propostas; `texto` de uma frase, até 160 caracteres; `destino` é `context`, `mapa:<área existente>` ou `agente:<agente existente>`; `fontes` com pelo menos uma lição real (a impressão digital casa). Erro por campo como as outras entregas.
- **O dev decide**: a entrega abre a lista numerada no terminal e no painel. `passo learn --apply 1,3` (comando do dev: entra em `DEV_COMMANDS` como `learn --apply`, do mesmo jeito que `map --cancel`) aplica; `passo learn --cancel` descarta. Aplicar: `context` acrescenta a linha em `.passo/context.md` sob `## Aprendido com os runs` com um comentário HTML das fontes; `mapa:<área>` acrescenta em `## Anotações` da área (como a revisão faz); `agente:<nome>` cria ou acrescenta em `.passo/agents/<nome>.md` com `extends: <nome>` quando o arquivo não existe. Todas as lições das propostas aplicadas ou recusadas entram em `.passo/learn/seen.json` (fora do git? **não**: vai no git, para o time não rever o mesmo). Depois, `passo sync` é sugerido quando um agente mudou.
- **Gatilho**: `passo status` e o fim de uma entrega dizem `Há 4 lições novas de 3 runs; peça ao agente para rodar passo learn` quando `collect` devolve 3+ lições não vistas de 2+ runs.
- **Trava e Stop**: `Activity::Learning`, só de leitura, como o mapeamento.

**Arquivos.** `crates/passo-core/src/learn.rs` (novo, com testes), `crates/passo-core/src/spec.rs` (`parse_learn`, `LEARN_TEMPLATE`), `crates/passo-core/src/lock.rs` (`Activity::Learning`, `learn --apply|--cancel`), `crates/passo-store/src/layout.rs`, `crates/passo-store/src/runs.rs` (`seen.json`), `crates/passo-app/src/service.rs` (`learn_start`, `learn_done`, `learn_apply`, `learn_cancel`), `crates/passo-app/src/render.rs`, `crates/passo-app/src/hooks.rs`, `crates/passo-cli/src/main.rs`, `assets/skill/SKILL.md` (seção "Aprender com os runs"), `assets/agents/refinador.md` (um parágrafo sobre a tarefa de aprender), `README.md` (tabela de comandos; um bullet em "Estender"), `docs/EXTENDER.md`.

**Resultado esperado.** Para quem usa: depois de alguns runs, uma lista curta de frases que o dev aprova com um comando, e que os agentes passam a ler. Testes: no core, duas recusas com palavras em comum em runs diferentes formam um grupo recorrente; a mesma recusa duas vezes no mesmo run não; lições vistas são excluídas. No app: três runs sintéticos com recusas; `learn` imprime a tarefa com o grupo; `done` com `destino: "mapa:x"` inexistente é recusado; `done` válido seguido de `--apply 1` acrescenta a linha ao `context.md` com a fonte e grava `seen.json`; o `next` de um run novo mostra a linha em `Convenções do projeto`.

---

## 05 · Orçamento de tokens por run

**Por quê.** Há orçamento de ações por passo, mas o que custa dinheiro são tokens, e um run pode queimar muito em Refinar ou numa Verificação longa sem nenhum passo estourar. Um teto por tamanho, com a decisão do dev, é o mesmo desenho do limite de diff.

**Como alterar.**

- `Profile` ganha `max_tokens: u64` (faturáveis: entrada + saída + escrita de cache; `0` desliga). Padrões: `pp` 60k, `p` 150k, `m` 400k, `g` 1M. Em `docs/EXTENDER.md`, na tabela dos tamanhos, com a nota de que só vale onde a plataforma expõe transcrições (hoje o Claude Code).
- **Fato novo para o motor**: `Command::Spent { tokens: u64 }`, ator `Passo`. O engine guarda `run.tokens: u64` (sempre o total medido) e, se `tokens > max_tokens` e `!run.tokens_accepted`, abre um gate **próprio**, `GateKind::Tokens` (rótulo `limite de tokens`), com `Tokens: 412k de 400k (tamanho m). Siga assim (passo ok) ou suba o tamanho (passo size g).` e o evento `limit.tokens`. Não reaproveitar `GateKind::Limite`: o `ok` daquele gate chama `complete_step`, porque ele só abre de dentro de `step_done`, antes de o passo fechar; o gate de tokens abre em qualquer ponto, inclusive logo depois de um passo já fechado, e um `ok` que completasse o passo seguinte commitaria trabalho que o agente não fez. `Approve` em `Tokens` só marca `run.tokens_accepted = true` e fecha o gate; `Reject` responde `aqui não há o que recusar`, como em `Travado`; `Resize` fecha o gate quando o teto novo cabe. Nenhuma outra transição acontece em `Spent`.
- **Quem mede**: o app, em `done`, `ask` e `next`, **depois** de aplicar o comando do agente, na mesma transição (a entrega válida do agente entra, e só então o dev decide; medir antes recusaria um brief ou plano bom com `DevTurn` e o modelo barato perderia a entrega). Só para o run ativo: `next` pode estar mostrando um run terminado, e nele não há o que medir. Lê `self.usage(run)` e aplica `Spent` quando o total mudou. Para não reler transcrições grandes a cada chamada, `App::usage` ganha cache em `runs/<id>/usage.json` por arquivo (`{path, len, mtime, samples}`), relendo só o que cresceu. O hook `Stop` também mede: se estourou, abre o gate (pelo mesmo `Spent`) e deixa o agente parar, com a mensagem do gate no `reason`.
- `passo status`, `passo stats` e o painel mostram `Tokens: 120k de 400k` e a barra; o relatório mostra o total e se o teto foi aceito.

**Arquivos.** `crates/passo-core/src/config.rs`, `crates/passo-core/src/model.rs`, `crates/passo-core/src/flow.rs`, `crates/passo-core/tests/flow.rs`, `crates/passo-app/src/service.rs`, `crates/passo-app/src/hooks.rs`, `crates/passo-store/src/layout.rs` (`usage(id)`), `crates/passo-app/src/render.rs`, `crates/passo-app/src/metrics.rs`, `crates/passo-cli/src/main.rs`, `assets/web/app.js`, `docs/EXTENDER.md`, `README.md` ("Você decide só o que importa": o teto de tokens é um dos limites).

**Resultado esperado.** Para quem usa: um run que passou do teto para e pergunta, em vez de seguir gastando. Testes no core: `Spent` abaixo do teto não muda nada; acima abre `Tokens` com o texto; um `Spent` acima do teto logo depois de um `StepDone` que fechou o passo abre o gate e o `ok` não fecha passo nenhum nem emite `Commit`; `ok` aceita e o próximo `Spent` maior não reabre; `size g` fecha o gate quando cabe; com `max_tokens: 0` nunca abre. No app: um `sessions.json` apontando para uma transcrição sintética com 500k faturáveis faz `next` abrir o gate em `m`; o cache em `usage.json` evita reler (segundo `next` não reabre nem relê: afirmar pelo `len` gravado).

---

## 06 · `passo stats --all`: a série ao longo dos runs

**Por quê.** As métricas por run existem. A pergunta que decide se o harness vale o custo é outra: quantas vezes o dev é chamado por tamanho, quanto custa um `m`, e se isso está melhorando. Isso só aparece olhando vários runs.

**Como alterar.**

- `passo stats --all [--since <data>] [--json]`. Lê todos os runs (`store.list()`), calcula as métricas de cada um (reusando `metrics::compute` com o log de `runs/<id>/events.jsonl` ou, quando esse não existe mais, a cópia em `.passo/audit/<id>/`), e imprime:
  1. Uma tabela por run: data, tamanho, estado, prova, vezes do dev, espera do dev, passos, tentativas falhas, escaladas, tokens faturáveis, tempo.
  2. Um resumo por tamanho: quantidade de runs, mediana e p90 de vezes do dev, tokens e tempo; taxa de entregas aceitas na primeira vez.
  3. Tendência: os últimos cinco runs contra os cinco anteriores, nas mesmas três medidas, com setas.
  `--json` emite a lista e o resumo como objeto.
- A agregação é pura: `metrics::aggregate(runs: &[(RunSummary, Metrics)]) -> Aggregate`, em `passo-app/src/metrics.rs`, testável sem disco.
- Num clone novo, `runs/` está vazio e a cópia em `.passo/audit/<id>/` só tem eventos e planos. Para a série sobreviver ao clone, a entrega (e o `stop`) passa a copiar também um `run.json` final para `.passo/audit/<id>/` (o run não muda mais depois de terminar). `store.list()` ganha a leitura desses `run.json` arquivados quando não há o de `runs/`.
- O painel ganha, na lista de runs, as colunas de vezes do dev, tokens e tempo, e um bloco "Por tamanho" com o resumo. O relatório não muda.

**Arquivos.** `crates/passo-app/src/metrics.rs`, `crates/passo-app/src/service.rs` (`stats_all`), `crates/passo-app/src/snapshot.rs`, `crates/passo-cli/src/main.rs`, `assets/web/app.js`, `README.md` (bullet do `passo stats`), `docs/ARQUITETURA.md` (seção "Métricas": uma frase).

**Resultado esperado.** Para quem usa: uma tela que responde "está valendo a pena?" sem abrir run por run. Testes: `aggregate` com seis runs sintéticos dá a mediana e o p90 certos por tamanho e a tendência com o sinal certo; no app, três runs no disco produzem três linhas e um resumo com `m: 3 runs`; `--since` filtra; `--json` é válido e tem `runs` e `by_size`.

---

## 07 · Cobertura do diff como prova

**Por quê.** Prova "forte" hoje significa que a suíte passou e o revisor confirmou os critérios. Não diz se as linhas que mudaram foram executadas por algum teste. Para um modelo barato, que escreve o teste primeiro porque a instrução manda, é a medida que separa "escrevi um teste" de "o teste exercita o que mudei".

**Como alterar.**

- Configuração, nos presets e em `passo.yaml`:

```yaml
checks:
  coverage:
    command: cargo llvm-cov --lcov --output-path {out} --quiet   # {out} é o arquivo que o Passo lê
    min_diff: 70                                                # % de linhas mudadas executadas; 0 desliga
    required: false                                             # true: abaixo do mínimo vira passo de correção
```

  Presets: rust (`cargo llvm-cov`), node (`npx c8 --reporter=lcov --reports-dir {outdir} npm test`, lendo `{outdir}/lcov.info`), python (`pytest --cov --cov-report=lcov:{out}`). Go fica sem (formato próprio; fora desta spec). O `init` escreve o bloco comentado quando o preset tem.
- **Parser puro** em `crates/passo-core/src/coverage.rs`: `parse_lcov(text) -> BTreeMap<String, BTreeSet<u32>>` (arquivo → linhas executadas ao menos uma vez; `DA:<linha>,<hits>` com `hits > 0`; os `SF:` relativizados pela raiz quando vierem absolutos) e `diff_coverage(changed: &BTreeMap<String, BTreeSet<u32>>, executed: &…, instrumented: &…) -> DiffCoverage { percent, uncovered: Vec<(String, Vec<u32>)> }`, contando só linhas mudadas que aparecem no lcov como instrumentáveis (uma linha de `use`/comentário não conta contra).
- **No motor, o encaixe**: hoje `final_checks`, quando tudo passa, marca `reviewing` ou entra em Entregar na hora. Com cobertura configurada (`min_diff > 0`), em vez disso emite `Effect::MeasureCoverage` e espera; o fato volta como `Command::Coverage(DiffCoverage)`, ator `Passo`, aceito só em `Verificar` com `run.awaiting_coverage` (flag nova, `#[serde(default)]`), e é ele que segue para a revisão ou para Entregar. Sem cobertura configurada, nada muda.
- **Quem mede**: o app, ao executar `MeasureCoverage`: roda `coverage.command` com o mesmo tempo limite, lê o lcov, calcula as linhas mudadas do run com `git diff <base> -U0` (parser pequeno em `infra.rs`, só os cabeçalhos `@@` e as linhas `+`), e devolve `Command::Coverage`. Arquivos de teste e `.md` ficam fora das linhas mudadas, como no limite de diff. Com `git.mode: none` não há base: o app devolve `Coverage` vazio com `measured: false`, o motor registra `coverage.skipped` e segue. O comando de cobertura roda a suíte de novo, então a Verificação demora cerca do dobro; isso fica dito em `docs/EXTENDER.md`.
- **No motor, o resultado**: `run.coverage = Some(DiffCoverage)`; evento `coverage.measured` (`Cobertura do diff: 83% (12 linhas sem teste em 2 arquivos)`). Se `percent < min_diff`: evento `coverage.low`; com `required: false`, a prova fica no máximo em `média` e as linhas sem teste entram nas notas para o revisor (`Linhas sem teste: src/x.rs 40-52, src/y.rs 10`); com `required: true`, vira um passo de correção `Cobrir com teste: src/x.rs 40-52` pelo mesmo caminho de `back_to_build`, contando volta.
- `render` em Verificar mostra a cobertura e as linhas sem teste ao revisor; o gate `aceitar` mostra `Cobertura do diff: 83%`; relatório e painel mostram as linhas sem teste por arquivo.
- `proof()` passa a ter quatro entradas: checagens finais, revisão, cobertura quando configurada (`percent >= min_diff`). `Forte` exige as três quando a cobertura está configurada; sem configurar, como hoje.

**Arquivos.** `crates/passo-core/src/coverage.rs` (novo, com testes), `crates/passo-core/src/config.rs`, `crates/passo-core/src/model.rs`, `crates/passo-core/src/flow.rs`, `crates/passo-core/tests/flow.rs`, `crates/passo-app/src/infra.rs` (`changed_lines`), `crates/passo-app/src/service.rs`, `crates/passo-app/src/render.rs`, `assets/presets/*.yaml`, `assets/passo.yaml`, `assets/stages/verificar.md`, `assets/web/app.js`, `docs/EXTENDER.md`, `docs/ARQUITETURA.md` (seção "Decisões": o que cada grau de prova exige), `README.md` (tabela das etapas, "grau de prova").

**Resultado esperado.** Para quem usa: a entrega diz quantas das linhas mudadas algum teste executou, e lista as que nenhum executou; "forte" passa a significar isso também. Testes no core: `parse_lcov` com dois arquivos e `DA` zerados; `diff_coverage` ignora linhas não instrumentadas; com cobertura configurada, `FinalChecks` que passa emite `MeasureCoverage` e não marca `reviewing`; `Coverage` fora de `Verificar` ou sem `awaiting_coverage` é recusado; abaixo do mínimo com `required: false` capa a prova em `média` e gera a nota; com `required: true` cria o passo de correção. No app: uma checagem de cobertura falsa que escreve um lcov sintético (`printf ... > {out}`) sobre um diff conhecido produz o evento com a porcentagem certa e as linhas sem teste no `next` do revisor.

---

## Ordem e dependências

`01` e `02` primeiro, nesta ordem (os dois mexem em `hooks.rs` e `lock.rs`). `03` em seguida. `04`, `05`, `06` e `07` são independentes entre si; `04` depende do mapa (`SPEC-MAPA`) só para o destino `mapa:<área>`, que fica desligado quando não há mapa.

## Fora desta spec

- Um lexer completo de shell (heredocs, arrays, funções): o que o lexer não lê é tratado como ilegível, por desenho.
- Orçamento de tokens no Copilot: sem transcrições, sem medida.
- Cobertura para Go e para linguagens sem lcov.
- `passo learn` sem agente: a colheita é do Passo, a frase é do modelo.
