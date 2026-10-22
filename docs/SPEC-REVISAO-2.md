# Spec: correções da revisão das specs MAPA, REVISAO e MELHORIAS

> **Estado: executada em 2026-10-22**, nos commits `rev2-01` a `rev2-11` da main. Não rode de novo; o texto fica como registro das decisões.

As três specs foram executadas (`mapa-00` a `mapa-06`, `rev-01` a `rev-10`, `mel-01` a `mel-07`) e revisadas item a item, com reproduções. As gates passam (fmt, clippy, 150 testes). Vinte e um itens atendem, com ressalvas em treze; **um não atende**: o lexer da trava (`mel-01`), que é contornável de cinco jeitos e, num deles, derruba o próprio hook.

Regras de execução iguais às de `docs/SPEC.md`: um item por vez, na ordem; um commit por item com prefixo `rev2-NN`; as três gates antes de cada commit; `CLAUDE.md` vale. Os itens 01 a 03 são de segurança e vêm primeiro.

---

## 01 · O lexer da trava: linear, fechado por padrão, e com relógio (alto)

**O que há.** `crates/passo-core/src/shell.rs` e `lock.rs`, depois de `mel-01`:

1. `unwrap` toma dois candidatos por prefixo (`env`, `sudo`, `nice`…) e recursa nos dois sem olhar `MAX_DEPTH`. O custo cresce ~1,6× por palavra: `env` repetido 32 vezes antes de `passo ok` gera 11 milhões de comandos, 3,5 s e 5 GB de memória; `sudo nice` 17 vezes, 10,7 s e 8 GB. O hook tem 15 s; um hook morto por tempo ou memória não grava `hook-errors.log` nem `hook.failed`, e a chamada passa.
2. Construções compostas ficam `readable = true` com `{`, `!`, `then`, `do` como programa: `{ passo ok; }`, `if true; then passo ok; fi`, `for x in 1; do passo ok; done`, `f(){ passo ok; }; f` passam.
3. Shells com flag combinada ou script por stdin não são desembrulhados: `bash -lc 'passo ok'`, `sh -ec 'passo stop'`, `bash -c -- 'passo ok'`, `echo 'passo ok' | sh`, `sh <<'EOF' … EOF`, `bash <<< 'passo ok'`, `env -S 'passo ok'` passam.
4. Subcomando dinâmico: `passo $s`, `passo $(echo ok)`, `passo o${x}k` passam pelo hook (a CLI ainda recusa no ambiente do Claude Code; no Copilot e com `PASSO_DEV=1`, não).
5. Caminho com curinga ou variável escapa de `.env` e `.passo`: `cat .en?`, `cat .env*`, `cat "$PWD/.env"`, `cat .pass?/current`, `echo x > .pass?/current` passam. Redirecionamento com alvo dinâmico em etapa só de leitura também: `echo x > src/$f.rs`, `echo x > src/a.r?`.
6. Opções globais do git além de `-C`/`-c`: `git --no-pager commit`, `git --git-dir=.git commit`, `git -p commit`, `git update-ref`, `git symbolic-ref` passam.

**Correção.** A filosofia muda num ponto: o lexer deixa de tentar *entender* cada construção e passa a *achar* o que importa em qualquer posição, com custo linear, e tudo o que fica fora do que ele lê é fechado durante um run.

- **Varredura por pares, sem desembrulhar prefixos.** `dev_command_in` e `git_write_in` percorrem **todas** as palavras de **todos** os comandos simples, não só `words[0]`: um par adjacente `(passo, <dev>)` ou `(git, <escrita>)` em qualquer posição nega. Isso cobre `sudo -u x passo ok`, `{ passo ok; }`, `then passo ok`, `xargs passo ok`, `find . -exec passo ok \;`, sem recursão. O `unwrap` de prefixos é apagado; ficam só `sh|bash|zsh|dash|ksh -c` e `eval`, que relexam o texto com `depth + 1` e **respeitam `MAX_DEPTH` dentro de `unwrap`**. Para o git, a varredura pula qualquer palavra que começa com `-` entre `git` e o subcomando, e a palavra seguinte quando a opção é `-C`, `-c`, `--git-dir`, `--work-tree`, `--namespace` sem `=`. `GIT_WRITES` ganha `update-ref`, `symbolic-ref`, `reflog`, `filter-branch`, `gc`, `prune`, `replace`.
- **Tetos.** Entrada maior que 16 KiB, mais de 128 comandos simples depois de lexar, ou mais de 2 048 palavras: `readable = false`. O lexer é uma passada só sobre o texto; nenhuma função pode ser chamada mais de uma vez por byte. Teste de tempo: `env` repetido 200 vezes, `sudo nice` repetido 100 vezes e um `$(` aninhado 50 vezes passam por `judge` em menos de 5 ms cada, em `release` e em `debug`.
- **Palavras reservadas são separadores.** `{`, `}`, `!`, `if`, `then`, `elif`, `else`, `fi`, `for`, `in`, `do`, `done`, `while`, `until`, `case`, `esac`, `function`, `select`, `time`, e `nome()` no início de um comando não entram em `words`: o que vem depois é um comando novo. Chave de expansão (`{a,b}`, `pass{o,}`) fora de aspas marca a palavra como `dynamic`.
- **Shell sem `-c` é ilegível.** Um programa de shell (`sh`, `bash`, `zsh`, `dash`, `ksh`, `fish`) invocado sem uma flag que contenha `c` (`-c`, `-lc`, `-ec`, `-c --`) lê um script de stdin ou de arquivo: `readable = false`. Com flag que contém `c`, o script é a primeira palavra que não começa com `-` depois dela. `env -S` e `env --split-string` relexam o argumento como `sh -c`. Pipes e heredocs cujo consumidor é um shell entram no teste de menção (abaixo) com o corpo do heredoc e o texto dos comandos anteriores no pipe.
- **A regra do ilegível muda para fechada.** Durante um run ou um mapeamento, um script com `readable = false` é **negado em qualquer etapa** quando o texto completo (inclusive corpos de heredoc) contém a palavra `passo` ou `git`, e negado em etapas só de leitura e na vez do dev mesmo sem menção. Em Construir sem menção, permitido (a conferência do git cobre). Mensagem: `não consigo ler este comando (<motivo: aspas sem par | shell sem -c | muito longo | expansão no nome do programa>); escreva o comando de forma direta`.
- **Programa dinâmico é negado em etapa só de leitura e na vez do dev**, com ou sem menção (`$(printf 'pas%s' so) ok` em Refinar). Em Construir, negado só com menção, como hoje. **`passo` com subcomando dinâmico ou ausente é sempre negado** durante um run: `passo $s`, `passo "$(printf ok)"`, `passo o${x}k` → `diga o subcomando do passo por extenso`.
- **Caminhos dinâmicos e com curinga.** Uma palavra que parece caminho e é `dynamic` ou contém `*`, `?`, `[` tem a parte literal depois do último trecho dinâmico (`$PWD/.env` → `/.env`; `.pass?/current` → o padrão inteiro) submetida às regras de `.env` e `.passo`: se o literal termina em `.env`/`.env.*` ou cita `.passo`, nega; se o padrão com curinga **pode** casar `.env*` ou `.passo*` (o curinga está no lugar de letras de `.env` ou `.passo`: `.en?`, `.e*`, `.pass?`, `.*`), nega com `curinga ou variável num caminho protegido; escreva o caminho por extenso`. Em etapa só de leitura e na vez do dev, um redirecionamento de escrita com alvo `dynamic` ou com curinga é negado (`alvo do redirecionamento com variável; use um caminho fixo em mktemp -d`). Em Construir, permitido.
- **Atribuições perigosas.** `LD_PRELOAD`, `LD_LIBRARY_PATH`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `PATH` e `BASH_ENV` como atribuição de prefixo num comando que cita `passo` ou `git` → negado: `o Passo e o git rodam com o ambiente do projeto, sem PATH ou bibliotecas trocadas`.
- **Relógio no hook (app).** Em `hooks::handle`, uma thread de guarda: se `decide` não respondeu em 8 s, o processo imprime a resposta de **negar** (`a trava demorou demais com este comando; escreva-o de forma mais simples`), grava `hook-errors.log` e `hook.failed`, e sai. Vale só para `PreToolUse` com run ou mapeamento ativo; os outros eventos seguem permitindo em silêncio. Isso fecha o buraco para qualquer explosão futura, não só esta.
- **Fuzz com vocabulário.** O corpus determinístico passa a sortear também as palavras `env`, `sudo`, `nice`, `sh`, `-c`, `{`, `then`, `do`, `passo`, `ok`, `git`, `commit`, `$(`, `<<EOF`. Além de "não entra em pânico", afirma: nenhum caso leva mais de 50 ms, e nenhum caso que contenha a sequência de palavras `passo ok` fora de aspas simples é permitido durante um run.

**Decisão na execução.** A regra do redirecionamento com alvo de variável em etapa só de leitura contradiz a tabela da `SPEC-MELHORIAS`, que permite `cat > "$(mktemp)" <<EOF` em Planejar, e o caminho que a skill ensina (o agente do teste ao vivo usou `f=$(mktemp) && cat > "$f"`). Ficou assim: o alvo de variável é permitido quando a parte variável vem de `mktemp`, direto, por uma variável atribuída com `mktemp` no mesmo comando, ou de `$TMPDIR`. O `mktemp` sempre devolve um caminho fora do projeto. O resto continua negado.

Outras três leituras da execução: `passo` sem subcomando ou com subcomando dinâmico é negado na posição de programa e logo depois de um prefixo (`sudo`, `env`, `xargs`…), não em qualquer posição, para não negar `grep passo "$f"`; o par `passo <dev>` continua negado em qualquer posição. O fuzz afirma a negação quando os tokens sorteados `passo` e `ok` ficam vizinhos; `"passo ok"` entre aspas duplas é uma palavra só e segue permitido, como na tabela da `SPEC-MELHORIAS` (`echo "passo ok" > /tmp/nota.txt`). No item 02, o prefixo `sk-` exige 20 ou mais caracteres entre letras, dígitos, `-` e `_`, com pelo menos um dígito: assim `sk-learn-extra` fica e `sk-proj-…` (chave real, com hífens) continua escondida.

**Arquivos.** `crates/passo-core/src/shell.rs`, `crates/passo-core/src/lock.rs`, `crates/passo-core/src/config.rs` (`rodar` herda os tetos), `crates/passo-app/src/hooks.rs` (guarda), `docs/ARQUITETURA.md` (seção "A trava": varredura por pares, ilegível fechado, relógio), `docs/SPEC-MELHORIAS.md` item 01 (nota: substituído por este item).

**Testes.** A tabela do item 01 da `SPEC-MELHORIAS` continua passando. Novos negados durante um run: os 25 comandos listados em "O que há" (1 a 6), mais `find . -exec passo ok \;`, `sudo -u x git push`, `LD_PRELOAD=./x.so passo ok`. Novos permitidos: `env FOO=1 npm test`, `sudo apt list`, `{ npm test; }`, `if npm test; then echo ok; fi`, `bash -lc 'npm test'` em Construir, `passo done - <<'EOF'` com `passo ok` dentro do JSON (consumidor não é shell), `cat "$HOME/.config/x"` (fora do projeto), `git --no-pager log`. Tempo: os três casos de teto em menos de 5 ms. Teste do app: o hook com um comando que força a guarda (um hook de teste injeta um `decide` lento) responde `deny` em menos de 10 s e grava `hook.failed`.

---

## 02 · Redação: opções curtas, números, e o resto do snapshot (médio)

**O que há.** `crates/passo-core/src/redact.rs:184` só trata `--opção`; `mysql -p hunter2`, `mysql -phunter2`, `sshpass -p hunter2`, `curl -u user:hunter2` passam. A regra `chave:` não olha o valor: `session: 2` e `src/auth: 3 arquivos` viram `«oculto»`; `PWD=/Users/x` é ocultado porque `pwd` está na lista; `sk-learn-extra` vira `«oculto»`. `Snapshot::redacted` não cobre `run.request`, títulos de passos, `changes[].text`, `gate.detail`, `runs[]`.

**Correção.**

- Opções curtas por programa, numa tabela pequena: `mysql`/`mariadb` `-p` (colado ou separado), `psql` `-W`, `sshpass` `-p`, `curl`/`wget` `-u` (o valor depois de `:`), `openssl` `-pass`, `az`/`aws`/`gh` `--password`/`--token`. Fora da tabela, `-x valor` não é chave.
- Valor só é ocultado se tem 8+ caracteres ou contém um caractere que não é dígito nem ponto; `session: 2` e `auth: 3 arquivos` ficam.
- `pwd` só casa como chave inteira, sem distinguir maiúsculas, quando é `pwd` ou `passwd`; `PWD` e `OLDPWD` nunca são chave. Prefixo `sk-` só com 20+ caracteres alfanuméricos a seguir; o mesmo para `sk_live_`/`sk_test_`.
- `Snapshot::redacted` cobre todos os textos de autoria do agente ou da shell: `run.request`, `brief.*`, `plan.*`, `steps[].title/detail/summary/test/test_command`, `criteria[].text/evidence`, `questions[]`, `changes[].text/before/after`, `gate.title/detail`, `notes`, `feedback`, `runs[].request`, `map_nodes`. O caminho mais simples: serializar o `Run` para JSON, redigir todo valor string, desserializar.

**Testes.** Os casos acima, nos dois sentidos; idempotência mantida; teste do snapshot com um `request` contendo `TOKEN=0123456789abcdef`.

---

## 03 · `doctor` e o hook: sonda sem efeito colateral, antes da config (médio)

**O que há.** `passo doctor` chama `App::open` antes de testar o hook, então com `passo.yaml` inválido (o caso que motiva o item) ele morre na configuração e nunca chega ao `✗ o hook respondeu "permitir"`. A sonda manda `passo ok` ao hook vivo, e com run ativo isso grava um `lock.denied` real no `events.jsonl`, contado nas métricas como recusa do agente. `last_error` devolve a última linha do `hook-errors.log` sem olhar a data. Matcher errado é reportado como "não está instalado". `doctor.rs` codifica o formato de payload e a string de decisão do Claude Code dentro de `passo-app`.

**Correção.**

- `doctor` passa a ter duas metades: a primeira usa só `Layout` (plataformas instaladas, hooks, binário, versão, `hook-errors.log`) e roda sempre; a segunda abre o `App` e, se falhar, imprime `✗ configuração: <erro>` e segue com o que der.
- A sonda leva `PASSO_PROBE=1` no ambiente do hook. Com essa variável, `hooks::handle` julga e responde, mas não grava evento, não conta orçamento, não registra transcrição e não decide nada pelo chat. Teste: com run ativo, `doctor` não acrescenta linha ao `events.jsonl`.
- O payload da sonda e o reconhecimento da resposta saem de `doctor.rs` para o trait: `Platform::probe(event) -> Value` e `Platform::is_deny(&Value) -> bool`. `doctor.rs` fica sem nomes de plataforma, inclusive no doc-comment.
- `last_error` considera só linhas mais novas que o início do `doctor`, e a mensagem diferencia `não está instalado` de `instalado com matcher diferente do esperado (achei "Bash", esperava "Read|Write|…"): rode passo init`.
- `crates/passo-store/src/layout.rs`: o doc-comment de `hook_errors` volta para cima de `hook_errors()`; `learn_task()` recebe o seu.

---

## 04 · Mapa: remapear não apaga anotações, e o explorador pode ler o mapa (médio)

**O que há.** `MapDraft::into_map` reconstrói cada área a partir do JSON e `store::map::save` sobrescreve o arquivo: um novo `passo map` apaga as `## Anotações` do revisor (`mapa-05`) e as linhas de `## Aprendido` (`mel-04`), além de edições à mão. E a trava nega leitura de `.passo/` durante o mapeamento, então o explorador não consegue ler o mapa atual para atualizá-lo; a tarefa só mostra `id (paths)`.

**Correção.**

- `save` preserva, por área, as seções `## Anotações` e `## Aprendido` do arquivo existente, anexando-as ao corpo novo; o corpo novo substitui só o que vem antes da primeira dessas seções. Áreas que o novo mapa não traz são apagadas junto com as anotações, mas a saída do `map_done` lista: `áreas removidas (e as anotações delas): x, y`.
- A trava, em `Activity::Mapping` e `Activity::Learning`, permite **leitura** sob `.passo/map/` e `.passo/context.md`; escrita continua negada. Nos runs nada muda.
- `render::map_task` mostra o corpo das áreas atuais, inteiro, e marca as que `Staleness` apontou como mudadas.
- `.task.json.projects` lista as raízes anexadas; a árvore dos anexados na tarefa não tem teto de 40 linhas.
- `map_done` faz a conferência de árvore **antes** da validação do JSON, para o agente não corrigir o JSON e só então descobrir que a árvore mudou.
- Com `git.mode: none`, a entrega grava o `commit` das áreas confirmadas como `sem-git` em vez de não gravar.

**Testes.** Mapear duas vezes mantém a anotação da área A; um `Read` em `.passo/map/areas/a.md` durante o mapeamento é permitido e em `.passo/runs/` continua negado.

---

## 05 · Projetos anexados: escrita conferida pelo git, dependências validadas (médio)

**O que há.** A trava nega escrita num anexado só quando o caminho resolvido começa pela raiz anexada; `cd ../api && echo x > f` resolve `f` contra a raiz principal e passa, porque o lexer não segue `cd`. `map_done` e `check_read_only` só rodam `git dirty` na raiz principal. `depends_on: [api/naoexiste]` e `[xyz/foo]` passam calados. Um `areas/x.md` quebrado no anexado derruba o mapa do principal inteiro.

**Correção.**

- O lexer segue `cd <literal>`, `pushd <literal>` e `cd -` dentro de um script: o `cwd` dos comandos seguintes muda; `cd` com argumento dinâmico marca o script como `readable = false`. Caminhos relativos nos comandos seguintes resolvem contra o novo `cwd`.
- `check_read_only` e `map_done` conferem `git dirty` também em cada raiz anexada que é um repositório git; mudança lá é `lock.breach` com o nome do projeto.
- `load_all` valida `depends_on` com prefixo contra o mapa do anexado carregado: projeto inexistente ou área inexistente é erro com o arquivo. Um erro de leitura num anexado vira aviso (`mapa de api ignorado: …`) e o mapa principal segue válido. `attached_warnings` distingue `pasta ../api não existe` de `sem mapa`.
- `map_summary` no `doctor` usa `load_all`, para contar as áreas anexadas e mostrar o mesmo erro que `start` mostra.

---

## 06 · `rodar` e `tests.runners`: a lista amplia, e sem variáveis de ambiente (médio)

**O que há.** `tests.runners` no YAML substitui a lista padrão (listas substituem no merge), então quem acrescenta `vitest` perde `cargo` e `npm`, e `docs/EXTENDER.md` documenta isso como se fosse a intenção. Um prefixo `VAR=x` passa pela regra de `rodar` e pela trava: `LD_PRELOAD=./x.so cargo test` é aceito como comando de teste. A terceira mensagem de erro ("não é uma chamada simples…") não está documentada.

**Correção.**

- `Config::test_runners()` devolve os padrões **mais** `tests.runners`; para tirar um padrão, `tests.runners_only: true`. `docs/EXTENDER.md` corrige o texto.
- `rodar` com qualquer atribuição de prefixo é recusado: `` `passos[1].rodar` não pode ter variável de ambiente (FOO=1); configure em checks ou no comando de teste do projeto ``.
- As três mensagens de recusa de `rodar` vão para `docs/EXTENDER.md`.
- Teste no core que amplia via YAML (não via `push`).

---

## 07 · `passo learn`: lição não citada não some (médio)

**O que há.** `learning.rs:170-172` marca como vistas todas as lições mostradas na tarefa, inclusive as que o agente não citou; como a tarefa manda deixar de fora lições de um run só, elas somem sem ninguém decidir.

**Correção.** A entrega ganha `descartadas: [{ "fonte": "<linha da lição>", "porque": "..." }]`; só entram em `seen.json` as lições citadas em `propostas` aplicadas ou recusadas pelo dev, e as `descartadas`. As demais voltam na próxima tarefa. A tarefa diz isso ao agente. `docs/EXTENDER.md` acompanha. As palavras de parada de `learn.rs` passam a ser `make_id::STOP` mais a lista própria, numa constante só, em `passo-core`.

---

## 08 · Cobertura não medida não rebaixa a prova (médio)

**O que há.** Com cobertura configurada e `measured: false` (`git.mode: none`, comando que falhou, lcov vazio), `proof()` nunca dá `forte`; o teste fixa isso e o EXTENDER documenta. A spec dizia que `coverage.skipped` "segue" e que a cobertura só entra na prova como `percent >= min_diff` quando medida.

**Correção (decisão).** `proof()` considera a cobertura só quando `measured`. Não medida: a prova vem das checagens e da revisão, e a entrega mostra `Cobertura: não medida (<motivo>)`. Teste e EXTENDER ajustados. Quando o comando de cobertura **falha** (não zero), isso é um `coverage.failed` com a cauda da saída, visível ao dev no gate de aceite.

---

## 09 · Orçamento de tokens: detalhe do gate acompanha o gasto (baixo)

`flow.rs:781`: `spent` retorna cedo com o gate `Tokens` aberto; `run.tokens` atualiza mas `gate.detail` fica com o número antigo. Atualizar o `detail` quando o gate aberto é o de tokens. Documentar em `docs/SPEC-MELHORIAS.md` item 05 a decisão do hook `Stop` responder `Context`/`systemMessage` em vez de `reason`.

---

## 10 · `stats --all` sobrevive ao clone também sem git (baixo)

`deliver()` sem git e `leave_branch()` sem branch não arquivam o `run.json`. Arquivar (copiar para `.passo/audit/<id>/`) sempre; commitar só com git. Teste com `git.mode: none`.

---

## 11 · Testes e docs que ficaram atrás (baixo)

- O teste do gêmeo `-forte` do Copilot vai para `crates/passo-adapters/src/copilot.rs`; `crates/passo-app/tests/app.rs` deixa de citar modelos do Copilot. Teste de app para `measure` preenchendo `agent_tier` a partir do catálogo.
- `rev-07`: um teste que mistura as três regras de notas e afirma ordem cronológica sem duplicata.
- `rev-08`: o `append` do evento `tree.failed` não termina em `.ok()`; o teste de app afirma `target` fora da árvore. O `write_atomic(...).ok()` do cache de uso de tokens (`service.rs:~1183`) propaga o erro.
- `examples/simulacao/tabela.js`: comentários `//`, não `///`.
- `docs/SPEC-MELHORIAS.md` item 01: nota "substituído por `docs/SPEC-REVISAO-2.md` item 01".

---

## Fora desta spec

- Interpretadores que executam comandos (`python -c 'os.system(…)'`, `node -e`, `awk system`): a trava não os lê; as outras linhas são a recusa da CLI no ambiente do agente e a conferência pelo git. Registrado em `docs/ARQUITETURA.md`, seção "A trava", como limite conhecido.
- Programa ofuscado sem a palavra literal (`$(printf 'pas%s' so)`): coberto em etapas só de leitura pela regra do programa dinâmico; em Construir fica na recusa da CLI.
- O que o Claude Code faz ao vivo quando um hook estoura o tempo: presumido "permite" pela documentação; a guarda do item 01 torna isso irrelevante.

## Ordem

`01`, `02`, `03` (segurança), depois `04`, `05` (mapa), `06`, `07`, `08`, `09`, `10`, `11`. `05` depende do `cd` do lexer em `01`.
