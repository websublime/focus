# Focus: plano de arquitetura e migração

O projeto chamava-se Sulca até 2026-10-09; o workspace, os crates e o binário passaram a `focus`.

Estado em 2026-11-12, com os marcos M0 a M4 fechados (secção 15 para o último) e M5 partido em fatias (secção 16). Referência: `awslabs/aidlc-workflows` no commit `2a555f7` (v2.11.0, licença MIT-0). Focus é um fork congelado nesse commit; o que o original publicar depois não é seguido.

## 1. Resumo

Focus é um engine em Rust para o AI-DLC, nascido como port do engine original em TypeScript. As decisões que enquadram este plano:

- **Âmbito:** engine core primeiro (state machine, orchestrator, audit, guards, hooks). O Markdown de `core/` é reutilizado, com os nomes de Focus.
- **Harnesses:** Claude Code, Cursor e GitHub Copilot.
- **Fork congelado:** a referência é o commit fixado em `fixtures/upstream/PIN`. Não há previews diárias a acompanhar.
- **Plugins mantêm-se** no âmbito.
- **Nome:** `focus` em tudo: comando, variáveis de ambiente (`FOCUS_*`), nomes em disco (`focus/`, `.focus-engine`, `focus-<nome>.md`) e conteúdo.
- **Repositório:** `github.com/websublime/focus`. Titular do copyright: Websublime.
- **Plataformas:** Linux, macOS e Windows, com distribuição por `cargo-dist`.
- **Licença:** MIT.

O que existe hoje são os três primeiros marcos. M0 deu o compile do grafo. M1 fechou o grafo: seleção de plugins e scopes compostos no compile, escrita no sítio sob um lock de workspace próprio, e todos os verbos de `focus engine graph` (`topo`, `cycles`, `scope`, `artifacts`, `producers`, `consumers`, `validate-scope`, `validate-grid`, `ars`, `resolve`, `export`). M2 trouxe a auditoria (o formato, os shards, o comando `audit`), as settings, o leitor de Markdown e o runtime graph (o comando `runtime`), mais três correções que apareceram pelo caminho (secção 13). Estado, guards, hooks e orchestrator ainda não existem.

Três conclusões condicionam tudo o resto:

1. **O port é guiado por um oráculo, não por leitura.** O engine original corre aqui (Bun), por isso cada marco termina com as mesmas entradas a passar pelos dois motores e as respostas comparadas como texto. Em M1: 1 825 comandos executados nos dois motores sobre árvores de projeto alteradas, com 0 diferenças fora de 16 verificações que são desvios deliberados (8 causas), e 208 030 casos de função com 0 diferenças (secção 10).
2. **`aidlc-lib.ts` não se porta de cima para baixo.** São 42 056 linhas importadas por 73 dos outros 110 ficheiros. Reparte-se por crates, à medida que cada marco precisa de uma fatia.
3. **O maior risco técnico era o Markdown.** O engine interpreta Markdown com o `Bun.markdown`, e o que cada consumidor pode ler depende de que linhas são visíveis. Em M1, `validate-grid` resolvia os ficheiros em que isso se decide linha a linha e recusava os restantes. A fatia 5 de M2 trouxe o leitor a sério (`focus-markdown`, secção 13): lê a estrutura com um parser, coincide com o original linha a linha nos documentos que as pessoas escrevem, e onde os dois se separam há um terceiro parser a arbitrar.

## 2. O que existe a montante

Tudo nesta secção foi medido no clone do commit fixado.

| Área | Ficheiros | Linhas TS | Papel |
| --- | ---: | ---: | --- |
| `core/tools` | 92 | 189 500 | Engine e CLI `aidlc` |
| `core/hooks` | 19 | 15 722 | 17 hooks e 2 helpers partilhados |
| `harness/*` | | 17 180 | Sete harnesses; os três de Focus somam 7 721 |
| `scripts` | | 13 471 | Packager e release |
| `tests` (quatro níveis) | 818 | 429 705 | 14 smoke, 535 unit, 186 integration, 83 e2e |
| Markdown em `core/` | 147 | | Stages, agents, knowledge, scopes, regras (424 `.md` no repositório inteiro) |

Características que pesam num port para Rust:

- **Um módulo central.** `aidlc-lib.ts` tem 42 056 linhas e 1 234 declarações exportadas. Os comentários de secção não delimitam responsabilidades: a maior "secção" cobre 16 428 linhas.
- **Semântica de JavaScript nos formatos em disco.** 1 947 regex literais (contados com o parser do TypeScript: 1 685 em `core/tools`, 262 em `core/hooks`) e 106 `new RegExp`; 68 chamadas a `localeCompare`; 691 ocorrências de `JSON.stringify`.
- **Built-ins do Bun.** Em `core/tools` e `core/hooks`: 6 chamadas a `Bun.markdown.render` e `.html`, 8 a `Bun.JSONC.parse`, 6 a `Bun.TOML.parse`, 25 a `Bun.spawnSync`. O `package.json` só declara `devDependencies`.
- **Configuração por ambiente.** 163 identificadores `AIDLC_*` distintos. A maioria são variáveis de ambiente, muitas delas costuras de teste (`AIDLC_STAGES_DIR`, `AIDLC_RULES_DIR`, ...); cerca de 20 são constantes ou marcadores com o mesmo prefixo.
- **Registo de auditoria.** 117 tipos de evento em 25 categorias (`core/knowledge/aidlc-shared/audit-format.md`).
- **O Markdown é cliente do CLI.** Os ficheiros de `core/` e dos três harnesses invocam o engine 408 vezes pelo token `{{INVOKE}}`: `engine orchestrate` 125 vezes, `engine log` 55, `engine worktree` 31, `engine state` 29, `engine config` 28, `engine bolt` 12, entre outros. Há ainda 48 invocações pelo caminho de uma ferramenta TS (26 pares ferramenta+verbo), sem contar a tabela de emissores de `audit-format.md`, que é documentação. Reutilizar o Markdown obriga o `focus` a expor esses comandos com os mesmos nomes.

### Como os três harnesses diferem

Lido dos ficheiros gerados pelo packager original (`harness.json`, `settings.json`, `hooks.json`, `.github/hooks/aidlc.json`). Os nomes da tabela são os do original; em Focus, `.aidlc` é `.focus` e `.github/hooks/aidlc.json` é `.github/hooks/focus.json`.

| | Claude Code | Cursor | GitHub Copilot |
| --- | --- | --- | --- |
| Diretório do engine | `.claude` | `.cursor` | `.aidlc` |
| Registo de hooks | `.claude/settings.json` | `.cursor/hooks.json` | `.github/hooks/aidlc.json` |
| Entrada dos hooks (distribuição nativa) | direta: `engine hook <alvo>` | adapter: `engine adapter cursor <alvo>` | adapter: `engine adapter copilot <alvo>` |
| Eventos registados | 8, com `SessionEnd` e `SubagentStop` | 8, com `postToolUseFailure`, sem eventos de subagente | 8, com `SubagentStart`, sem `SessionEnd` |
| Status line | sim | não | não |
| Limite de diretiva declarado | nenhum | nenhum | 19 000 bytes |

O limite declarado só baixa o teto comum do engine, que é de 28 672 bytes para todos (`DEFAULT_DIRECTIVE_MAX_BYTES` em `aidlc-orchestrate.ts`). Na distribuição por cópia, os registos chamam `bun` sobre os ficheiros TS em vez do binário.

Os adapters de Cursor (3 028 linhas) e Copilot (2 304) convertem o payload de cada host para o formato do Claude Code e traduzem a decisão de volta. O original descreve parte de cada harness como dados (`harness.json`, lido pelo engine), mas a maior parte das diferenças de comportamento são comparações com o nome ou o diretório do harness, espalhadas pelo engine. Em Focus essas diferenças vivem em `focus-harness`, como dados.

O grafo compilado só difere entre harnesses no caminho dos sensores (`.claude/sensors/...` contra `.aidlc/sensors/...`). O `scope-grid.json` é igual nos três.

## 3. O que conta como paridade

Paridade é com o que sai do engine, não com a sua estrutura interna, e é a menos de um mapa de nomes: onde o original diz `aidlc`, Focus diz `focus` (`tools/oracle/names.py`, secção 7). As superfícies são cinco:

1. **Ficheiros compilados**, comparados byte a byte pelo próprio original (`compile --check`).
2. **Estado e auditoria em disco** (`focus-state.md`, shards de `audit/`): o mesmo conteúdo que o original escreveria, com os nomes de Focus.
3. **Diretivas**: o JSON que o engine entrega ao agente em cada `next`.
4. **I/O dos hooks**: códigos de saída (nos guards, 0 permite e 2 recusa), JSON em stdout, texto em stderr.
5. **Texto de erros e recusas**, que os agentes leem e a que reagem.

Fora da paridade ficam acidentes de implementação e três coisas que Focus decide por si: o nome com que o engine fala de si próprio (`focus`, `FOCUS_*`), o lock do workspace, e a forma de encontrar o projeto (secção 10). Os desvios deliberados estão nas secções 9 e 10; cada um tem um teste marcado "Deliberate deviation".

Os princípios do original que o desenho em Rust tem de preservar (`AGENTS.md`):

- **Tools for determinism, LLM for knowledge, human for judgement.** O engine nunca interpreta o sentido das palavras da pessoa.
- **One owner per rule.** Cada invariante tem um dono e uma verificação partilhada; uma segunda versão independente da mesma regra é um defeito.
- **Guards falham a favor da pessoa.** Um guard que perde o seu registo de coordenação segue a última instrução registada; não pergunta outra vez nem bloqueia sem saída. A exceção é a evidência que liga uma decisão humana (um registo de aprovação, a fingerprint do plano): essa nunca recua para um palpite.

## 4. Arquitetura alvo

```
focus-cli ✔ ───────────► focus-hooks ──► focus-orchestrate ──► focus-state ◐ ─► focus-audit ✔
    │                        │                  │                   │               │
    │                        ▼                  ▼                   ▼               ▼
    ├──► focus-project   focus-guards ◐    focus-sensors       focus-graph ✔   focus-workspace ✔
    │                        │                  │                   │               │
    ├──► focus-intent ✔ ─────┴──────────────────┼───────────────────┤               │
    ├──► focus-lifecycle ◐ ──────────────────────┼───────────────────┤               │
    │                        │                  │                   │               │
    └────────────────────────┴──────────────────┴───────────────────┴───────────────┘
                                                │
                     focus-harness ✔    focus-markdown ✔  focus-frontmatter ✔    focus-js ✔
```

✔ assinala os crates que já existem; ◐ os que existem em parte.

| Crate | Responsabilidade | Estado |
| --- | --- | --- |
| `focus-js` | Semântica JS: espaço em branco, ordenações e colação, ordem de chaves, `JSON.parse` e `JSON.stringify`, impressão e arredondamento de números, regex ECMAScript, `Date.parse` para os timestamps que o engine escreve | Existe |
| `focus-frontmatter` | Leitores de frontmatter (subconjunto de YAML do original) | Existe |
| `focus-harness` | Descritores dos três harnesses; mais tarde, os adapters de payload | Descritores existem |
| `focus-graph` | Schemas, compile do grafo e do grid, scopes compostos, consultas, validação de planos, ARS | Completo para o CLI de `graph` |
| `focus-workspace` | Projeto e harness de um comando, caminhos, escrita atómica, lock do workspace, recusa de links, o scan do workspace; mais tarde, spaces e intents | Existe; settings em M2, scan em M3 |
| `focus-markdown` | Estrutura de blocos Markdown (`markdownBlocks`, `visibleMarkdownLines`) | M2 |
| `focus-audit` | Shards de auditoria, registo de eventos, runtime graph | Existe (M2) |
| `focus-state` | Ficheiro de estado, máquinas de workflow, fase e stage | Existem os campos, as checkboxes e o ficheiro (M3, fatias 1 e 2); as transições são M3 |
| `focus-intent` | Criação de intents: nomes, plano, template do estado, registo e cursores | Existe (M3, fatia 3b) |
| `focus-lifecycle` | Transições de stage: a cadeia de admissão, onde estão os artefactos de uma stage, o recibo de validade de uma conclusão, e as transições em si | Existem `advance`, `finalize`, `complete-workflow`, `skip` e `resume` (M3, fatia 4a); os verbos de gate são a fatia 4b |
| `focus-guards` | Lexer de shell, integridade de runtime, fences, decisões de guard | Existe (M3, fatia 1; M4): a Guard Policy e as fences, o lexer de shell, a integridade do runtime, e os leitores dos quatro guards. A avaliação de uma aprovação e os recibos são M5 |
| `focus-hooks` | Os corpos dos hooks e o dispatcher | Existe (M4): o dispatcher, o contexto de um hook, os observadores, o arranque de sessão, o turno humano, os quatro guards e o grupo que os corre num processo. `continue-workflow`, `deliver-stage-rules` e a status line são M6 |
| `focus-evidence` | A identidade da fonte, os recibos de revisão e de sumário lidos, o brief de revisão | M5: existe desde a fatia 1a, com a listagem de fonte (não está no diagrama) |
| `focus-log` | O comando `log`: as perguntas, as respostas, os handoffs e as revisões escritos na auditoria | M5: existe desde a fatia 2a (não está no diagrama) |
| `focus-orchestrate` | `next` e `report`, diretivas, plan approval, checkpoints | M5 |
| `focus-sensors` | Dispatcher e os seis sensores | M7 |
| `focus-project` | Projeção de `core/` para a árvore de cada harness, configuração, doctor | M8 |
| `focus-cli` | Binário, tabela de rotas, texto de ajuda | Existe, cresce em cada marco |

Regras de desenho, aplicadas desde M0 (as duas últimas, desde M1):

1. **`focus-js` é o único dono da semântica JS.** Código portado não chama `str::trim`, `sort` ou `regex` diretamente quando o resultado chega a disco.
2. **Regex portadas tal como estão**, sobre o crate `regress` (motor ECMAScript). Reescrever perto de 2 000 padrões para outra gramática seria re-derivar as regras em cada ponto de chamada.
3. **O erro é a mensagem.** `EngineError` transporta o texto que o original lançaria. Variantes tipadas entram quando um consumidor precisar de distinguir casos.
4. **Fontes explícitas em vez de estado global.** O original guarda caches de módulo e resolve caminhos por variáveis de ambiente. `Sources` e `Layout` recebem os diretórios como dados, o que dispensa as variáveis que só existem como costura de teste (`AIDLC_STAGE_GRAPH`, `AIDLC_SCOPES_DIR`, ...); ficam as que são contrato com o utilizador, com o prefixo `FOCUS_`.
5. **Diferenças entre harnesses são dados** em `focus-harness`.
6. **O que Focus não sabe decidir é recusado, não adivinhado.** Em M0 era o compile a recusar plugins e scopes compostos; em M1, é o leitor de memória a recusar ficheiros que só um parser de Markdown decide, e `resolve` a recusar escrever num registo que ainda não sabe localizar.
7. **Relógio, UUID, PID e hostname injetáveis** a partir de M2, para que a auditoria seja reproduzível no oráculo.
8. **Ter o lock é ter um valor.** Uma função que tem de correr sob o lock do workspace recebe `&WorkspaceLock`; o compilador verifica o que o original verifica com um contador de reentrância.
9. **Os nomes vivem num sítio, e são todos `focus`.** `focus-workspace::names` tem o comando, o prefixo `FOCUS_*`, o ficheiro de lock, o diretório de trabalho (`focus/`), o diretório do engine (`.focus-engine`) e o prefixo dos ficheiros e do plugin core (`focus-<nome>.md`, `focus`). O original diz `aidlc` em cada um desses sítios; nenhum nome do original é lido.

## 5. Mapa de migração

Atribuição feita pelo cabeçalho de cada ficheiro e pelo grafo de imports. É o ponto de partida de cada marco, a confirmar no seu arranque.

### Em âmbito

| Crate | Módulos de origem (linhas) | Marco |
| --- | --- | --- |
| `focus-graph` | `graph` (3 989), `stage-schema` (861), `sensor-schema` (282), `rule-schema` (128), `artifact-vocabulary` (31) | M0, M1 |
| `focus-workspace` | fatia de `runtime-paths` (744) | M1 |
| `focus-workspace` | `settings` (843), `recorded-switches` (403), `runtime-budget` (19), resto de `runtime-paths` | M2 |
| `focus-audit` | `audit` (2 058), `runtime` (1 476) | M2 |
| `focus-state` | `state` (8 867), `validity` (891), `jump` (925), `artifact-resolution` (340), `unit-walk-view` (69) | M3 |
| `focus-state`, `focus-cli` | `utility` (12 887): criação de intents, status, handlers `/aidlc --*` | M3 a M5 |
| `focus-guards` | `hooks/review-freeze-command` (2 083), `hooks/runtime-integrity` (1 427), `guard-switch` (1 464), `guard-operation` (468), `question-store` (412), `message-store` (246), `reply-reader` (119), `guard-fences` (55) | M4 |
| `focus-guards` | `testing-posture` (5 453), `plan-approval-ask` (2 196) | M4, M5 |
| `focus-hooks` | `plan-approval-guard` (2 793), `state-transition-guard` (1 275), `reviewer-scope` (1 161), `record-human-turn` (934), `session-start` (658), `review-freeze` (447), `rebuild-stage-graph` (355), `write-audit-log` (243), `sync-workflow-state` (147), `session-end` (130), `validate-state` (122), `log-subagent` (113), `hook-trace` (66) | M4 |
| `focus-evidence` | `review-brief` (2 365) | M5 |
| `focus-log` | `log` (4 486) | M5 |
| `focus-orchestrate` | `orchestrate` (16 133), `directive` (2 976), `construction-checkpoints` (1 618), `bolt` (1 511), `learnings` (1 252), `rules-held` (595), `includes` (481), `inline-context` (398), `steering` (128) | M5 |
| `focus-hooks` | `continue-workflow` (2 191), `statusline` (746), `deliver-stage-rules` (412) | M6 |
| `focus-harness` | `aidlc-cursor-adapter` (3 028), `aidlc-copilot-adapter` (2 304) | M6 |
| `focus-cli` | `aidlc` (3 735), `command` (670), `completions` (316), `color` (72), `version` (4) | M6 |
| `focus-sensors` | `sensor` (1 239), `sensor-claim-sources` (1 075), `sensor-traceability` (756), `sensor-linter` (423), `sensor-type-check` (341), `sensor-required-sections` (258), `sensor-upstream-coverage` (224), hook `run-sensors` (318) | M7 |
| `focus-project` | `init` (13 046), `config-diagnostics` (3 815), `distribution` (1 624), `runner-gen` (917), `model-policy` (891), `doctor` (657), `tiers` (286), `gitignore` (99); `manifest.ts` dos três harnesses, `emit.ts` do Copilot e a parte aplicável de `scripts/package.ts` | M8 |
| `focus-project` | Plugins: `plugin` , `plugin-validate`, `plugin-test`, `plugin-emit`, `plugin-create`, `plugin-build` (5 019 no total) | M8 |
| repartido | `lib` (42 056) | M0 a M6 |

Soma por módulo: 174 082 linhas de `core/tools` e `core/hooks` (169 063 antes de os plugins entrarem no âmbito), das quais 42 056 são `aidlc-lib.ts`. É um limite superior. Os módulos em âmbito trazem ramos para os quatro harnesses excluídos e para a instalação nativa, que não se portam; não tenho medição que diga quanto pesam.

### Adiado

31 140 linhas (15,2 % de `core/tools` e `core/hooks`).

| Tema | Linhas | Módulos |
| --- | ---: | --- |
| Construction em paralelo | 8 185 | `worktree`, `swarm`, `swarm-checkpoints` |
| Units de equipa | 3 769 | `unit`: `Unit Ownership: team`, as claims sobre refs de git, o board de equipa (decisão de 2026-11-12, secção 11) |
| Instalação nativa | 8 415 | `lifecycle`, `windows-uninstall`, `transaction`, `release`, `install-paths`, `uninstall-plan`, `machine-config`, `update`, `archive`, `channel`. Com `cargo-dist` a empacotar e instalar o binário, a maior parte disto é substituída, não portada |
| DocumentKB | 5 182 | `knowledge`, `documentkb-schema` |
| Custos e métricas | 2 500 | `usage`, `metrics`, hook `fold-usage` |
| Diagnóstico | 2 078 | `doctor-bundle`, `validate` |
| Proveniência de commits | 1 872 | `attest` |
| Multi-repo | 1 551 | `workspace-sync`, `workspace-manifest`, `workspace-doctor` |
| Kiro | 1 357 | `kiro-session`, `hook-front-gate`, `kiro-ide-terminal`, `kiro-ide-workflows` |

Adiado não quer dizer ausente. Quando um módulo em âmbito importa um adiado, porta-se só a fatia usada. Por import estático, as dependências adiadas com mais consumidores em âmbito são `install-paths` (6: `settings`, `utility`, `plan-approval-guard`, `init`, `doctor`, `config-diagnostics`), `channel` (5), e com 3 cada `usage`, `doctor-bundle`, `kiro-ide-terminal` e `kiro-ide-workflows`.

Novos workflows no original usam Construction em série por omissão (`Construction Execution: serial` no template de estado), o que permite adiar `swarm` e `worktree`. `bolt` fica em âmbito: o caminho em série usa `engine bolt checkpoint` e `set-autonomy` (`stage-protocol-construction.md`). O Markdown reutilizado também chama `engine worktree` (31 vezes) e `engine swarm` (8); enquanto estiverem adiados, esses comandos têm de existir e recusar com uma mensagem clara.

## 6. Marcos

Cada marco fecha com um critério verificável contra o oráculo.

**M0. Fundações e compile do grafo.** Feito. Critério cumprido: os dois ficheiros compilados são byte a byte iguais nos três harnesses; ver secção 9.

**M1. Grafo completo e workspace.** Feito. Critério cumprido: `focus engine graph compile` escreve no sítio, sob lock, e todos os verbos de `graph` passam o diferencial; ver secção 10. Ficou por fazer a pista paralela opcional (o lexer de shell), que passa para M4, e a fatia de settings de `focus-workspace`, que passa para M2 porque nenhum verbo de `graph` a exige além de um aviso.

**M2. Auditoria, Markdown e settings.** Feito. Critério cumprido: para os mesmos shards, `runtime-graph.json` sai byte a byte igual, e as linhas de auditoria que o compile escreve também, a menos do timestamp; ver secção 13.
- Settings (`focus.settings.json` nas três camadas, switches gravados). Fecha o limite de M1 nos kill switches.
- Leitura e escrita de shards, registo dos 117 eventos, compile do runtime graph. Este compile é determinístico mas não é puro: lê o audit log, o ficheiro de estado, o stage graph e os `memory.md`, e acrescenta linhas `MEMORY_EMPTY` à auditoria.
- `focus-markdown`, com a decisão de parser tomada por medição (risco R1). Substitui o leitor por linhas de `guard_memory` e deixa de haver ficheiros de memória recusados.
- Critério: para os mesmos shards, `runtime-graph.json` byte a byte igual; linhas de auditoria iguais com relógio injetado.
- Entrou em fatias, cada uma um pull request verificado por si (secção 13).

**M3. Estado e criação de intents.** Feito. Critério cumprido: as sequências de transições, verbos de gate e saltos geradas ao acaso (`fuzz/transitions`, `fuzz/jump`) deixam o mesmo ficheiro de estado e os mesmos shards nos dois motores, a menos do mapa de nomes; ver secção 14. Entrou em oito fatias (1, 2, 3a, 3b, 4a, 4b, 5a, 5b), cada uma um pull request. O que é de units, bolts e do orquestrador recusa-se pelo nome e vai para M4 e M5 ("Limites de M3").
- Ficheiro de estado, transições (`advance`, `approve`, `reject`, `skip`, `gate-start`, `park`), `intent-create`, `jump`, `validity`, `--status`.
- Critério: uma sequência de comandos deixa o mesmo ficheiro de estado e os mesmos shards nos dois motores, a menos do mapa de nomes.

**M4. Guards e hooks observadores.** Feito. Critério cumprido: os pedidos de hook do diferencial (`hook/*`) dão o mesmo código de saída, stdout, stderr e linhas de auditoria nos dois motores, a menos dos desvios declarados; ver secção 15. Entrou em dezasseis fatias (a tabela da secção 15). O que os guards leem do último passo do engine (o marcador de diretiva, a avaliação de uma aprovação, os recibos de revisão) recusa-se pelo nome e é de M5; os hooks `continue-workflow` e `deliver-stage-rules` são de M6 e `fold-usage` vem com os custos.
- O lexer de shell `review-freeze-command.ts` (2 083 linhas, só importa built-ins do Node); dependem dele dois guards e `runtime-integrity`.
- `runtime-integrity`, fences, os quatro guards de `PreToolUse` (um só processo, o grupo `guard-tool-call`), e os hooks que só registam.
- Critério: um corpus de payloads de hook dá o mesmo código de saída, stdout, stderr e linhas de auditoria.

**M5. Orchestrator.**
- `next` e `report`, diretivas, plan approval, testing posture, review brief, checkpoints de Construction em série.
- Critério: uma jornada completa produz a mesma sequência de diretivas JSON.
- Partido em doze fatias, com as decisões de âmbito e a forma de medir o critério: secção 16.

**M6. Adapters e ciclo de conversa.**
- Adapters de Cursor e Copilot, hook `Stop` (`continue-workflow`), `deliver-stage-rules`, status line, tabela de rotas do CLI.
- Critério: os payloads reais de cada host (o original tem amostras em `tests/fixtures/cursor-hook-payloads` e `copilot-hook-payloads`) dão as mesmas decisões.

**M7. Sensores.**
- Dispatcher e os seis sensores.
- Critério: para os mesmos ficheiros, cada sensor dá o mesmo veredicto.

**M8. Projeção e instalação.**
- Gerar a árvore de cada harness a partir de `core/`, reescrevendo as invocações do Markdown para `focus`; configuração de um projeto; doctor mínimo.
- Ferramentas de plugin (`create`, `build`, `validate`, `test`, `emit`). A seleção de plugins no compile já existe desde M1.
- Critério: a árvore gerada equivale à do packager original para os três harnesses; um workflow corre de ponta a ponta em cada host.

Tamanho: M5 é o maior bloco (31 943 linhas atribuídas a `focus-orchestrate` e `focus-evidence`, mais a fatia de `lib` e parte de `utility`). As 21 335 de `focus-project` são um limite superior largo: grande parte de `init` trata dos sete harnesses e da instalação nativa. M0 cobriu, pela minha soma dos intervalos de linhas portados, cerca de 3 000 linhas de TypeScript, e M1 cerca de 2 700; não há um mapa função a função que o confirme. Não dou estimativa em tempo: os dois marcos feitos são os mais autocontidos, e a partir de M2 o código lê e escreve estado partilhado.

## 7. Verificação

Quatro instrumentos, todos em `tools/oracle/`:

Os dois motores não partilham vocabulário: o original lê e escreve `aidlc`, Focus lê e escreve `focus`. `names.py` é o mapa entre os dois, uma substituição de texto (`aidlc`, `AIDLC`, `Aidlc`). O original é sempre interrogado nas palavras dele; as árvores, as perguntas e as respostas são traduzidas antes de chegarem a Focus ou a `fixtures/`. A saída de Focus nunca é traduzida, por isso um nome que tenha ficado por mudar aparece como diferença. Os dois nomes têm o mesmo comprimento, o que deixa intactos tamanhos, colunas e limites em bytes.

1. **Corpus de oráculo.** `gen_cases.py` gera casos com seed; `driver.ts` responde com o código do original; `focus dev oracle` responde com o de Focus; comparam-se como texto, incluindo a ordem das chaves. Cada crate declara os tipos de caso a que responde (`KINDS`, no seu módulo `oracle`): em `focus-graph`, frontmatter de stage, sensor e regra, os dois leitores primitivos, colação, números, padrões sem distinção de caixa, ciclos, registos de scopes compostos, fold-back, settings de proposta e Guard Policy em memória; os de `focus-audit`, `focus-workspace` e `focus-markdown` entraram com M2 (secção 13). Um corpus de 8 526 casos fica em `fixtures/oracle/`.
2. **Diferencial ao vivo.** `differential.py` constrói um projeto Claude Code a partir da distribuição do original, altera-o de uma forma específica, dá uma cópia a cada motor, corre o mesmo comando nos dois e compara o que um chamador observa: código de saída, stdout, stderr e, num compile, os ficheiros que ficam em disco. Com `--fuzz N` acrescenta propostas e pontuações aleatórias.
3. **Diferencial gravado.** Com `--record`, o mesmo script guarda o que o original fez em cada verificação (a árvore de partida, o comando, o resultado) em `fixtures/recorded/`. Um teste reconstrói cada árvore e exige de Focus o mesmo resultado.
4. **Goldens.** Os ficheiros compilados pelo packager original, em `fixtures/upstream/golden/`.

`cargo test` repete o corpus, o diferencial gravado e os goldens sem Bun e sem clone do original. `refresh.sh` regenera tudo a partir de um clone.

Há um tipo de caso do corpus que não se compara como texto: `markdown`, a estrutura de um documento linha a linha. O original aproxima-a e Focus lê-a de um parser, por isso cada linha é julgada com outros dois parsers a arbitrar (`markdown_rule.py`, `referee/`; secção 13, fatia 5). O mesmo juízo corre ao vivo sobre documentos hostis com `differential.py --markdown N`.

O que o diferencial normaliza antes de comparar, e nada mais: o nome com que cada motor se refere a si próprio, a localização da árvore temporária, e o texto que vem do runtime e não do engine (a mensagem do sistema operativo para um ficheiro em falta, a do parser de JSON).

Os instrumentos foram testados contra si próprios. Em M0, trocar `trim` pela versão do Rust acusou 106 diferenças em 14 695 casos. Em M1, cinco mutações deliberadas, todas detetadas e depois repostas:

| Mutação | Detetada por | Quantas verificações falharam |
| --- | --- | ---: |
| Contar dois gates por stage em `grid_cost_summary` | diferencial gravado | 479 de 1 219 |
| Banda LOW do ARS com `<=` em vez de `<` | diferencial gravado | 23 de 1 219 |
| Fold-back deixar de aceitar `stages: null` e listas | corpus | 7 de 4 454 |
| Trocar `_` e `-` na ordem de colação | corpus | 1 de 4 454 |
| Linhas dentro de um comentário passarem a visíveis | corpus | 93 de 4 454 |

As contagens são de quando o diferencial gravado tinha 1 219 verificações; hoje tem 1 221.

Duas leituras desta tabela. A mutação do fold-back passou no diferencial gravado: os cenários que a exercem são um desvio deliberado, e por isso saltados; foi o corpus que a apanhou. E a da colação só foi apanhada por um caso, o que é pouco; por isso a colação tem uma verificação exaustiva à parte (secção 10).

### Reutilizar a suite do original

- 366 dos 535 ficheiros unit e 72 dos 186 de integration importam `core/tools` diretamente, por isso testam funções TS em processo e não servem contra um binário.
- Os restantes 169 unit e 114 integration não o importam diretamente, mas muitos importam helpers de `tests/harness` (por exemplo `fixtures.ts`) que carregam `core/tools` em processo. A fração que invoca o engine só como processo, e que serviria de suite de conformidade apontada ao `focus`, é menor e está por medir.
- Há fixtures prontas a servir de entrada ao oráculo: 15 ficheiros de estado (`state-*.md`), `audit-sample.md`, os payloads de hooks de Cursor e Copilot, e `markdown-goldens/corpus.json` (1,5 MB), que fixa o comportamento do interpretador de Markdown.

## 8. Riscos

**R1. Markdown.** Medido e reduzido na fatia 5 de M2 (secção 13). O original interpreta Markdown com `Bun.markdown`, que não dá posições na fonte, e descobre a estrutura inserindo palavras-sonda e vendo onde saem; tem ainda código que repara o renderer onde este se afasta do CommonMark. Focus lê a estrutura com `pulldown-cmark`. Nos 391 documentos de `markdown-goldens/corpus.json` e nos 256 ficheiros de Markdown do repositório do original, não há uma linha colocada pelos dois em que a estrutura difira. Fora do que as pessoas escrevem, separam-se, e cada linha é julgada com um terceiro parser. O que fica do risco: os consumidores que ainda não estão portados (sensores, autoridade de review, perguntas) vão ler a mesma estrutura, e onde o original falha a colocar uma linha vão ver outra coisa; cada um entra com o seu diferencial. E o parser tem cantos de tabelas em que lê diferente do GFM (0,19% das linhas em documentos hostis), contados e com teto.

**R2. `aidlc-lib.ts`.** Não há fronteiras internas a seguir. Mitigação: em cada marco, extrair primeiro a lista de funções de `lib` que os módulos do marco importam (os imports são nomeados) e atribuí-las a um crate antes de portar.

**R3. `localeCompare`.** Resolvido para ASCII. O original chama `localeCompare` em 68 pontos: 64 com a locale por omissão do processo, 4 com `"en"` e ordenação numérica. Em M1 a ordem dos scopes passou a chegar à saída (`nearest_stock`), e um nome de scope pode ter maiúsculas e `_ . + @`. `focus-js` implementa agora a colação raiz do CLDR para ASCII imprimível, em dois níveis (caracteres sem distinção de caixa, depois minúscula antes de maiúscula), e coincide com o Bun em 528 491 pares, incluindo todos os pares de um carácter. Fora de ASCII continua a usar ordem de code units, que não é a da ICU. O único ponto portado onde isso se pode ver é a ordem dos agentes no `export`: o nome de um agente não tem gramática, e um nome com acentos ficaria noutra posição.

**R4. Alvo em movimento.** Fechado por decisão: Focus é um fork congelado no pin.

**R5. Coexistência.** Fechado por decisão: o lock é de Focus e não é compatível com o do original. Um projeto é conduzido por um motor ou pelo outro. Fica uma consequência a tratar em M2 e M3: os formatos de estado e auditoria continuam a ser os do original (é o que o Markdown reutilizado descreve), mas deixam de ter de ser escritos de forma que o engine original os aceite a meio de um workflow.

**R6. Números.** Resolvido. `focus-js::num` imprime e arredonda como o JavaScript (`String`, `toFixed`, `Number`, `Math.round`), e `JSON.stringify` usa-o. O corpus alargado encontrou um defeito na primeira passagem: quando um double fica exatamente a meio entre dois candidatos mais curtos, o JavaScript escolhe o de último dígito par e o formatador do Rust o de cima (`2126906910518472.25` imprime `...472.2` num e `...472.3` no outro). Corrigido; 5 casos em 124 818 antes, 0 em 208 030 depois.

**R7. Regex fora do comum.** O original usa lookbehind (9 ocorrências) e a flag `v` com `\p{RGI_Emoji}` (2). O `regress` compila ambos e aplica-os a uma amostra com o resultado esperado (há um teste em `focus-js`). A flag `v` passou pelo corpus em M5: as 3 953 sequências de emoji do Unicode 17 e o que está ao lado de cada uma, comparadas com o runtime do original (secção 16, "Fatia 2c-i"). A flag `i` passou: o `regress` compara caracteres de outra maneira que o JavaScript em quatro sítios, `focus-js` corrige-o e recusa o padrão que a correção não cobre (secção 13, "Entre a fatia 5 e a 6").

**R8. Plataformas.** Medido pela primeira vez em 2026-10-09, no CI do primeiro pull request: os lints e os testes passam em Linux, macOS e Windows com a toolchain fixada (1.97.0), e o workspace compila com a 1.89. O lock usa `std::fs::File::lock` (`flock` em Linux e macOS, `LockFileEx` em Windows), sem FFI, e os seus testes correm nas três. Fica por medir o que só corre em Unix: o diferencial gravado (as mensagens trazem caminhos com o separador da plataforma) e os testes de links. A repetição do rename em Windows, quando outro processo tem o ficheiro aberto, continua sem um teste que a force. A primeira execução no `main` falhou nos lints por outra razão: o CI instalava a `stable` do dia, mais recente do que a usada aqui. A toolchain passou a estar fixada em `rust-toolchain.toml`.

**R9. Nomes no conteúdo reutilizado.** Fechado pela decisão 10. Focus adotou o Markdown como seu e renomeou-o: `aidlc` passou a `focus` em nomes de ficheiros, de diretórios e no texto, com a mesma substituição que as ferramentas de paridade aplicam (secção 7). Ficam dois efeitos a vigiar. Primeiro, os dois nomes ordenam de forma diferente em relação a outros (`aidlc` < `bravo` < `focus`): uma árvore com um plugin ou um ficheiro cujo nome caia entre os dois pode ser percorrida noutra ordem por cada motor, e duas mensagens de erro dependem da ordem. Nenhuma árvore do diferencial cai aí, e o diferencial mostrava-o. Segundo, a parte do Markdown que M1 ainda não lê (skills, knowledge, o texto dos harnesses) entra em M8 pelo mesmo mapa; as 408 invocações `{{INVOKE}}` e as 48 por caminho de ferramenta TS precisam de mais do que o mapa, e já estavam no âmbito de M8.

**R10. Peso das fixtures.** `fixtures/` tem 19 MB (corpus 10, dos quais 2,4 são os documentos de Markdown e as respostas linha a linha; diferencial gravado 6,6; referência 1,9). É o preço de `cargo test` provar paridade sem Bun. Se pesar no repositório, o diferencial gravado comprime bem ou pode viver fora.

## 9. Estado de M0

### Entregue

- Cinco crates: 4 286 linhas de Rust fora de testes e 1 365 de testes (`wc -l`, contando como teste os diretórios `tests/`, `tests.rs` e tudo a partir de `#[cfg(test)]`).
- `focus engine graph compile`, com dois modos de leitura (`--core` para um checkout de `core/`, `--installed` para um diretório de harness) e três de saída (`--out`, `--check`, `--print`).
- `focus engine graph topo | artifacts | producers | consumers`.
- Dependências externas diretas: `serde`, `serde_json` e `regress`.

### Verificado

| Verificação | Resultado |
| --- | --- |
| Ficheiros compilados a partir de `core/`, três harnesses | 6 de 6 byte a byte iguais (`stage-graph.json`: 90 861 bytes em Claude e Cursor, 90 782 em Copilot; `scope-grid.json`: 13 497) |
| `--installed --check` sobre as árvores geradas pelo original | em sincronia nas três |
| Corpus incluído | 1 573 casos, 0 diferenças |
| Corpus alargado, seeds 11 a 16 à escala 60 | 88 170 casos, 0 diferenças |
| Cenários de compile | 27: 26 iguais, 1 desvio deliberado |
| Consultas: `topo`, `artifacts`, `producers` e `consumers` de `requirements`, `producers traceability`, `consumers architecture`, `producers` de um artefacto inexistente | 7 de 7 com saída igual |
| `cargo test` (59 testes), `cargo clippy --workspace --all-targets`, `cargo fmt --all --check` | limpos |

Tempo de execução, numa medição única nesta máquina (2 vCPU, Bun 1.4.2, média de 20 execuções): `compile --check` cerca de 13 ms em Focus contra 89 ms no original; `topo` cerca de 3 ms contra 67 ms. O binário tem 1,8 MB. Os hooks correm em cada chamada de ferramenta do agente, por isso o arranque conta.

### Desvios deliberados

| | Original | Focus |
| --- | --- | --- |
| Chave `__proto__` no frontmatter | Descartada em silêncio; o ficheiro é aceite | `unknown key: __proto__` |
| Chave `constructor` | Reportada como reservada, com o código-fonte da função `Object` do JavaScript como motivo | `unknown key: constructor` |
| Ordem de percurso das fases | A do sistema de ficheiros, o que muda o texto de dois erros de máquina para máquina | Ordem do ciclo de vida, texto igual em todo o lado |
| Número fixado que não é `<fase>.<índice>` | Aceite; a comparação dá `NaN` e o compile termina com sucesso | Recusado com mensagem |
| Escrita dos ficheiros compilados numa árvore instalada | No sítio, sob lock do workspace | Recusada até o lock existir (fechado em M1) |

Os quatro primeiros foram reproduzidos nos dois motores. O quinto deixou de existir em M1.

### Fora de M0, dentro de `focus-graph`

- Seleção de plugins e scopes compostos, e o resto do CLI de `graph`: feitos em M1.
- `input_schema` do manifesto de sensor, que só o dispatcher lê (M7).
- `parseRuleHeadings`, usado pelo doctor (M8).

## 10. Estado de M1

### Entregue

- Seis crates: 9 643 linhas de Rust fora de testes e 3 193 de testes (mesma contagem de M0). O crate novo é `focus-workspace`.
- **Compile completo.** Seleção de plugins (`harness.json` `plugins`): um stage de um plugin não selecionado fica no grafo com `enabled: false` e sai do grid; a seleção é recusada se deixar um stage ativo sem produtor para um input obrigatório. Scopes compostos: o registo em `focus/scopes/<nome>.md` é a fonte, o compile repõe a projeção no harness quando falta, e cria o registo a quem só tinha a projeção.
- **Escrita no sítio.** `focus engine graph compile` recompila a árvore instalada sob o lock do workspace, com escrita atómica dos dois ficheiros. `--check` não escreve. Um compile recusado não deixa nada para trás: os ficheiros de scope que tinha reposto são retirados, e um link num dos dois destinos é detetado antes de se escrever o primeiro. A forma de fora (`--core` ou `--installed`, com `--out`) recusa escrever no diretório de dados de qualquer árvore instalada, porque não tem lock nem reconciliação de scopes compostos.
- **Lock do workspace**, desenho próprio: um lock consultivo do sistema operativo sobre `focus/.focus-lock`, por `std::fs::File::lock`. O kernel liberta-o quando o processo termina, de qualquer maneira, por isso não há locks órfãos a detetar. Espera até 30 s (`FOCUS_LOCK_TIMEOUT_MS`) e, se desistir, diz quem o tem: PID, o que está a fazer e há quanto tempo.
- **Projeto e harness.** O projeto é `--project-dir`, senão `FOCUS_PROJECT_DIR`, senão o que o host indicar (`CLAUDE_PROJECT_DIR`), senão o diretório mais próximo, de onde o comando corre para cima, com um harness instalado. O harness é `--harness`, senão `FOCUS_HARNESS`, senão o único instalado; com vários instalados, Focus pede que se diga qual.
- **Todos os verbos de `graph`**: `topo`, `cycles`, `scope`, `artifacts`, `producers`, `consumers`, `validate-scope`, `validate-grid`, `ars`, `resolve`, `export`.
- **Distribuição.** `dist-workspace.toml` e o workflow de release gerado pelo `cargo-dist` 0.32 para cinco alvos (Linux x86-64 e arm64, macOS x86-64 e arm64, Windows x86-64), com instaladores shell e PowerShell. Um CI próprio corre lints e testes nas três plataformas. O pacote do binário passou a chamar-se `focus` para os artefactos saírem com esse nome.
- Dependências externas diretas: `serde`, `serde_json` e `regress`; em testes, `sha2`. (Desde M5: `sha2` e `flate2` em `focus-evidence`, secção 16; e o `regress` otimizado também nos testes, "Fatia 2c-i".)

### Verificado

| Verificação | Resultado |
| --- | --- |
| Diferencial ao vivo, compile no sítio | 95 cenários: 86 iguais, 9 desvios deliberados, 0 diferentes. Comparam-se os dois ficheiros compilados e todos os ficheiros de scope e registos escritos |
| Diferencial ao vivo, verbos | 1 130 comandos sobre 25 árvores: 1 123 iguais, 7 desvios deliberados (uma só causa), 0 diferentes |
| Diferencial ao vivo, aleatório | 900 comandos `ars` e `validate-grid` (seeds 7 e 11): 900 iguais |
| Diferencial gravado, em `cargo test` | 1 221 verificações sobre 110 árvores; 4 cenários com links simbólicos não se gravam |
| Corpus incluído | 4 454 casos, 0 diferenças |
| Corpus alargado, seeds 21 a 25 à escala 60 | 208 030 casos, 0 diferenças, depois de corrigido o defeito de R6 |
| Colação ASCII | 528 491 pares, idêntica à do Bun |
| Guard Policy em memória, ficheiros dentro do domínio do leitor | 45 035 casos: todos respondidos, todos iguais |
| Guard Policy em memória, ficheiros com Markdown fora do domínio | 30 000 casos: 17 457 recusados por Focus, 12 543 respondidos e iguais, 0 respostas diferentes |
| `cargo test` (113 testes), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check` | limpos |
| Revisão independente do código de M1 | 12 achados: 9 corrigidos, 1 corrigido em parte, 2 registados como limites (ver abaixo) |
| `dist plan`, com um URL de repositório provisório | 5 arquivos e 2 instaladores; o artefacto de Linux x86-64 construiu-se localmente |

Tempo de execução, numa medição única nesta máquina (2 vCPU, Bun 1.4.2, média de 20 execuções sobre a mesma árvore):

| Comando | Focus | Original |
| --- | ---: | ---: |
| `compile` no sítio (com lock e `fsync`) | 13 ms | 87 ms |
| `compile --check` | 12 ms | 88 ms |
| `scope bugfix` | 4 ms | 61 ms |
| `validate-grid --custom` | 5 ms | 101 ms |
| `ars` | 4 ms | 67 ms |

O binário tem 2,5 MB.

### Desvios deliberados

Os sete primeiros aparecem pelo nome no diferencial e têm um teste marcado "Deliberate deviation" (o do link só corre em Unix). Os restantes são diferenças de superfície, cobertas pelos testes do CLI sem essa marca.

| | Original | Focus |
| --- | --- | --- |
| `harness.json` ilegível | Lê como "sem seleção" e compila, o que ativa todos os plugins que a instalação tinha desligado | Recusa o compile no sítio até o manifesto ser reposto |
| Coluna do grid cujo `stages` não é um objeto (`null`, lista) e ficheiro de scope | Cria um registo com esse valor, que falha a leitura em todos os compiles seguintes | Não cria registo |
| Scope composto chamado `constructor`, `toString`, `valueOf`, ... | Reportado como nome duplicado, ou descartado em silêncio do grid | É um nome como outro |
| `compile --check` sem `stage-graph.json` | Mostra o erro do runtime (`ENOENT`) | Diz que o ficheiro está desatualizado e como o regenerar |
| `validate-grid --matched` com um ficheiro de scope que não carrega | Engole o erro, assume que a Guard Policy do scope é `strict` e emite um veredicto sobre Guard Policy | Reporta o ficheiro de scope |
| Compile recusado depois de repor o ficheiro de um scope composto | O ficheiro reposto fica; sem coluna no grid, o scope lê-se como um que salta todos os stages | Retira o ficheiro: nada muda em disco |
| Ficheiro compilado que é um link simbólico | Só verifica o diretório de dados; faz o rename por cima do link | Recusa antes de escrever qualquer dos dois ficheiros |
| Lock do workspace | Diretório carimbado com PID, hora de arranque e token; recupera de crashes provando que o dono morreu | Lock de ficheiro do sistema operativo. Os dois não se veem |
| Projeto de um comando | Pelo caminho do próprio script, que vive dentro do projeto | Pelo diretório de onde o comando corre, para cima |
| Variáveis de ambiente | `AIDLC_*`, muitas delas costuras de teste | Só `FOCUS_*`, e só as que são contrato |
| Nomes em disco e no conteúdo | `aidlc/`, `.aidlc-engine`, `aidlc-common/`, `aidlc-<nome>.md`, plugin core `aidlc`, `.aidlc` como árvore do Copilot, `/aidlc`, `aidlc.settings.json` | O mesmo com `focus` no lugar de `aidlc`, em todo o lado. Um projeto instalado pelo original não é um projeto Focus |
| O engine a falar de si | "AI-DLC" | "Focus" |
| Flags desconhecidas | Ignoradas | `Unexpected argument` |
| `resolve` sem `--stdout` | Escreve `plan.json` no registo do intent ativo | Em M1 recusava. Fechado em M2: escreve no mesmo sítio (secção 13, "Antes da fatia 6") |
| `export --check` | Compara com uma fixture num caminho relativo ao repositório do original | `--check <ficheiro>` |

### Limites de M1

O que M1 não faz, e o que acontece quando lá se chega:

1. **Guard Policy em memória lida por linhas.** Fechado na fatia 5 de M2: `focus-graph::guard_memory` lê as linhas visíveis com `focus-markdown` e já não recusa ficheiro nenhum (secção 13). Em M1 lia só os ficheiros em que a visibilidade se decide linha a linha e parava com erro nos restantes.
2. **Kill switches só pelo ambiente.** Fechado na fatia 4 de M2: `validate-grid` vê os switches gravados em `focus.settings.json` nas três camadas, e o ambiente responde primeiro (secção 13).
3. **Space por omissão.** A memória consultada é a do space de `--space`, senão a do cursor `focus/active-space`. Bindings de sessão e worktrees delegadas não são consultados (M3).
4. **Só a árvore Claude passou pelos verbos.** Os goldens de compile cobrem os três harnesses; os 1 130 comandos de verbos correram sobre a distribuição `claude`. O código não distingue harnesses fora do diretório e dos caminhos de sensores, mas não foi medido.
5. **`saveComposedScope`** (gravar um plano aprovado como scope, com rollback) não está portado: é chamado pelo fluxo de criação, não pelo CLI de `graph` (M5).
6. **Ferramentas de plugin** (`create`, `build`, `validate`, `test`, `emit`) não estão portadas (M8). O compile honra a seleção.
7. **O que só corre em Unix** (R8). O CI passa nas três plataformas, mas o diferencial gravado e os testes de links não correm em Windows, e nenhum teste força a repetição do rename quando outro processo tem o ficheiro aberto.
8. **`compile --check` num checkout com CRLF.** `stage-graph.json` é comparado byte a byte, como no original. Num clone feito pelo Git for Windows com `core.autocrlf=true`, o ficheiro chega com CRLF e o `--check` diz sempre que está desatualizado. O sítio certo para resolver é o instalador (M8), que deve escrever um `.gitattributes` para a árvore do harness; até lá, quem usar Windows acrescenta-o à mão.
9. **Lock herdado.** O original deixa um processo filho correr `graph compile` dentro do lock do pai (`AIDLC_WORKSPACE_LOCK_OWNER_PID`; `aidlc-utility.ts`, L911, fá-lo). Focus não tem esse caminho e não o vai ter: quem já tem o lock chama `write_compiled` em processo. Os pontos de chamada de M3 a M5 têm de ser portados assim, senão o filho espera 30 s e falha.
10. **Uma recusa nomeia um comando que ainda não existe.** A mensagem para uma chave de política antiga num ficheiro de scope manda correr `focus config`, que é de M8.

### Revisão independente

M1 foi revisto por um segundo agente, que não viu o código a ser escrito, com instruções para procurar defeitos concretos no lock, na escrita atómica, na escrita no sítio, na portabilidade e na configuração de CI e release. Correu os dois motores lado a lado em 48 cenários seus e num fuzz com outra seed. Doze achados:

| Achado | Estado |
| --- | --- |
| A verificação de links estava dentro do ciclo de escrita: um link no segundo ficheiro deixava o par meio escrito, com uma mensagem a dizer que nada tinha mudado | Corrigido; teste |
| Ficheiros de scope repostos ficavam em disco quando o compile falhava (como no original) | Corrigido; passa a desvio deliberado; teste |
| `--out` só recusava o diretório de dados da própria árvore lida, não o de outra árvore instalada | Corrigido; teste |
| O workflow de release corria `dist plan` em cada pull request e falhava sem URL de repositório | Corrigido: `pr-run-mode = "skip"` até haver URL |
| A repetição do rename em Windows decidia pelo `ErrorKind`, que provavelmente não cobre `ERROR_SHARING_VIOLATION` | Corrigido por leitura: passa a ler também os códigos 5, 32 e 33. Nunca executado |
| `.gitattributes`: `*.md text eol=lf` vinha depois de `fixtures/** -text` e ganhava-lhe | Corrigido |
| Os avisos do compile perdiam-se quando a escrita falhava a seguir | Corrigido; teste |
| Um argumento que não é Unicode fazia o processo abortar com código 101 | Corrigido: erro normal, código 1; teste |
| Teste do lock removia o diretório com o ficheiro ainda aberto; `refresh.sh` usava `sed -i` à maneira do GNU; nada exercita os ramos de Windows | Os dois primeiros corrigidos; a falta de cobertura é o limite 7 |
| `compile --check` com CRLF | Limite 8 |
| Lock herdado por processo filho | Limite 9 |
| README afirmava mais do que o código: marca "Deliberate deviation" em todos os desvios, e omitia `CLAUDE_PROJECT_DIR` | README corrigido |

O que a revisão deu como são, por medição em Linux: lock e escrita atómica sob 24 compiles e 24 leitores concorrentes; a resolução de projeto e harness; scopes compostos e seleção de plugins contra o original. As distribuições Cursor e Copilot do original passaram `compile --check`, compilaram no sítio para os mesmos bytes e responderam a `scope` e `validate-scope`, o que reduz o limite 4. Sobre macOS, Windows e a toolchain 1.89, a revisão leu e não mediu, como eu.

## 11. Decisões

### Tomadas em 2026-10-09

| | Decisão | O que mudou |
| --- | --- | --- |
| 1 | Sem previews diárias | Fork congelado no pin. R4 fechado |
| 2 | Manter plugins | Seleção de plugins portada em M1; ferramentas de plugin entram em M8 (5 019 linhas passam de adiado para âmbito) |
| 3 | Lock próprio, à maneira do Rust | Lock de ficheiro do sistema operativo, sem FFI e sem compatibilidade com o do original. R5 fechado |
| 4 | O nome é `focus` | Comando `focus`, variáveis `FOCUS_*`, pacote `focus` |
| 5 | Linux, macOS e Windows, com `cargo-dist` | CI nas três, release para cinco alvos |
| 6 | O repositório é criado por ti | Criado: `websublime/focus` (decisões 8 e 11) |
| 7 | MIT | `LICENSE` e metadados Cargo |
| 8 | Repositório `git@github.com:websublime/focus.git` | `repository` nos metadados Cargo; o workflow de release volta a planear em pull requests (`dist plan` dá 5 arquivos e 2 instaladores) |
| 9 | Titular: Websublime | `LICENSE` |
| 10 | O projeto chama-se focus e tudo reflete esse nome | Nomes em disco, conteúdo e mensagens renomeados de `aidlc` para `focus`. R9 fechado. O original continua a ser a referência através de um mapa de nomes (secção 7) |
| 11 | O repositório é a fonte, com GitHub Flow | Tudo vive em `websublime/focus`. O `main` está sempre verde; cada alteração é um branch curto e um pull request, que só entra com o CI verde nas três plataformas. As regras estão em `CLAUDE.md` |

A decisão 3 dizia "cargo toma posse". Li-a como: nada de compatibilidade com o lock em TypeScript, usar o que a toolchain do Rust dá. Se querias dizer outra coisa, é barato corrigir agora.

### Tomada em 2026-11-12

**`Unit Ownership: team` fica adiado.** As claims de `unit` (3 769 linhas: um registo atómico sobre refs de git, com um remote), o board de equipa e o progresso derivado das units servem várias equipas em vários checkouts. É da mesma família que a Construction em paralelo, com que partilha os branches e o merge para `main`, e verificá-lo pede um diferencial com repositórios git, remotes e vários clones, que mais nada em M5 precisa. Faz-se à frente, como marco próprio, com `worktree` e `swarm`. Até lá um estado com `Unit Ownership: team` recusa pelo nome, nunca corre como se fosse a solo.

### Em aberto

1. **`.gitignore` do lock.** `focus/.focus-lock` não deve ser commitado. O instalador (M8) é quem escreve o `.gitignore` do projeto; até lá, quem usar Focus num projeto real acrescenta a linha à mão.
2. **Peso das fixtures (R10).** 19 MB no repositório, ou diferencial gravado fora dele.
3. **O nome do método na prosa.** O Markdown reutilizado diz "AI-DLC" quando fala do método (por exemplo, "AI-DLC workflow"). Não mexi: é o nome de uma metodologia, não um identificador, e trocá-lo por substituição de texto estraga a gramática ("an AI-DLC" passaria a "an Focus"). Se o queres fora, é uma edição de conteúdo à mão, que cabe em M8 com o resto do Markdown.

## 12. Como correr

```sh
cargo test                          # corpus, diferencial gravado e goldens, sem Bun
cargo build --release

cd um/projeto/com/harness/instalado
focus engine graph compile          # recompila no sítio, sob lock
focus engine graph validate-scope feature
focus engine graph --help

focus engine audit append ERROR_LOGGED --field Stage=code-generation
focus engine audit history --limit 20
focus engine audit --help

focus engine runtime compile        # escreve o runtime-graph.json do registo ativo
focus engine runtime summary
focus engine runtime --help

# Com Bun e um clone do original no commit do PIN:
tools/oracle/refresh.sh ../aidlc-workflows                          # regenera fixtures e grava o diferencial
python3 tools/oracle/differential.py ../aidlc-workflows --fuzz 600  # os dois motores lado a lado
python3 tools/oracle/differential.py ../aidlc-workflows --only '^$' --markdown 2500   # só o Markdown hostil
```

## 13. Estado de M2

M2 está feito. O arranque confirmou o que R2 previa: os módulos de M2 não são folhas. `aidlc-audit.ts` importa 35 nomes de `aidlc-lib.ts` e `aidlc-runtime.ts` importa 27, e atrás deles estão a resolução do registo de um intent, o lock de auditoria, bolts, worktrees e a leitura do ficheiro de estado. Por isso M2 entrou de baixo para cima, em seis fatias; a segunda partiu-se em duas, e entre a quinta e a sexta entraram dois pull requests de correções.

| Fatia | O que traz | Como se verifica | Estado |
| --- | --- | --- | --- |
| 1. Formato do registo | Os 117 tipos de evento e os seus títulos, validação e bloco de uma entrada, leitores de blocos e de campos, timestamp. Crate `focus-audit` | Corpus de oráculo | Feita |
| 2a. Qual é o registo | Spaces, intents, registo de intents, cursor do intent ativo, binding de sessão e a seleção do workflow, em `focus-workspace` | Corpus: projetos descritos como conjuntos de ficheiros | Feita |
| 2b. O shard | Identidade do clone (`focus/.focus-clone-id`), nome e caminho do shard (`auditFilePath`, `auditShardName`), listagem e leitura de todos os shards de um registo (`auditShards`, `readAuditShardEvents`, `readAllAuditShards`), cópias lidas uma só vez | Corpus: projetos em disco e conjuntos de shards | Feita |
| 3. Escrita e leitura em disco | Append sob o lock do workspace, com as verificações de links e de identidade do ficheiro; `focus engine audit append`, `append-batch`, `append-raw` e `history`; relógio injetável | Diferencial de comandos: shards iguais a menos do timestamp | Feita |
| 4. Settings | `focus.settings.json` nas três camadas e os switches gravados. Fecha o limite 2 de M1 | Corpus e diferencial de `validate-grid` | Feita |
| 5. Markdown | `focus-markdown`: a estrutura de um documento linha a linha e as linhas que um consumidor pode ler, com `pulldown-cmark` (R1). Fecha o limite 1 de M1 | Corpus com os goldens do original, o Markdown que ele distribui e documentos gerados; fuzz ao vivo com um terceiro parser a arbitrar | Feita |
| Entre a 5 e a 6 | `focus-js`: a flag `i` sem `u` compara caracteres como o JavaScript | Corpus: todos os caracteres com caixa como padrão, e os padrões do original | Feita |
| Antes da 6 | A barreira de observador nas escritas duráveis; `resolve` escreve o plano no registo; a escrita atómica deixa de criar diretórios | Diferencial de comandos | Feita |
| 6. Runtime graph | `runtime-graph.json` a partir dos shards, do ficheiro de estado e dos `memory.md`; `focus engine runtime compile`, `read` e `summary` | Corpus, diferencial de comandos e fuzz de auditorias: `runtime-graph.json` byte a byte igual | Feita |

A fatia 6 precisava do ficheiro de estado, que o plano tinha em M3, e de bolts e worktrees, que estão adiados. A leitura revelou-se separável: o compile só lê o campo `Scope`, e o resumo as linhas de checkbox (`parseCheckboxes`, `parseStateStageSuffixes`), duas funções sem dependências. Portaram-se essas, para o crate `focus-state`, que assim começa aqui; o resto do ficheiro de estado continua em M3. Dos bolts, o compile só lê eventos da auditoria; o que mexe em worktrees (`fragment-fork`, `fragment-merge`) ficou recusado pelo nome.

### Fatia 1: formato do registo

O que existe em `focus-audit`:

- **Vocabulário.** Os 117 tipos de evento, pela ordem do original, cada um com o título do seu bloco. A lista foi tirada do original a correr, não copiada à mão: a recusa de um tipo desconhecido enumera-os todos, e cada título saiu de uma entrada escrita pelo original.
- **Escrever uma entrada.** A validação recusa um tipo fora do vocabulário, um campo chamado `Event` e um nome de campo que não seja uma etiqueta numa só linha. O bloco tem uma linha de timestamp, uma de evento e uma por campo; os terminadores de linha de um valor passam a `\n` e a localização do projeto passa a `<project-dir>`.
- **Ler.** Todos os blocos de um evento, do mais antigo para o mais recente; o campo de um bloco; o campo em lista de um ficheiro de estado; os argumentos `--field chave=valor`.
- **Timestamp.** UTC ao segundo, com o instante como parâmetro: quem escreve o registo pode correr com um relógio fixo.

O título de um shard é o único sítio em que o engine assina um ficheiro que escreve; em Focus é `# Focus Audit Log`.

| Verificação | Resultado |
| --- | --- |
| Corpus incluído, seis tipos de caso novos | 1 137 casos novos (5 591 no total), 0 diferenças |
| Corpus alargado, seeds 21 a 23 à escala 40 | 20 751 casos, 0 diferenças |
| Quatro mutações deliberadas (separadores de linha por escapar, redação sem o caso do espaço, empates reordenados, prefixo de lista por ignorar) | Todas detetadas pelo corpus: 6, 7, 4 e 5 casos |
| `cargo test` (à data da fatia 1) | 129 testes |

Cada crate passou a repetir no seu próprio teste os tipos de caso a que responde, e um teste do CLI garante que nenhum tipo fica sem dono.

O que a fatia 1 não tem: nada decide ainda para que shard vai uma entrada, não há lock de auditoria nem comando `audit`. `CLI_PROTECTED_EVENT_TYPES` e a lista de eventos que um merge não pode trazer entram com os comandos que as usam (fatia 3).

### Fatia 2a: qual é o registo

Quase nenhum comando diz em que space e em que intent atua. `focus-workspace::records` decide os dois, sempre pela mesma ordem: o que o comando disse (`--space`, `--intent`); o binding da sessão que chama, se houver sessão conhecida; os cursores do checkout (`focus/active-space` e, no space, `active-intent`); e o único registo vivo do space, quando há exatamente um. Sem nada disso, não resolve intent nenhum e a resposta é só o space.

O que existe:

- **O registo de intents** (`intents.json`) lido como o original o lê, sem tipos: uma linha junta-se a uma diretoria pelo `dirName` que guarda ou, nas linhas antigas, por `<slug>-<hex>` com o hex a ser a cauda do `uuid`. Um registo arquivado não volta sozinho.
- **O cursor do intent ativo**, que só conta quando nomeia um registo que existe. É texto livre, por isso o caminho que nomeia junta-se como o `path.join` do Node o junta, e não como o `Path::join` do Rust: um cursor que comece por `/` não substitui a base.
- **O binding de uma sessão** (`focus/.focus-sessions/<id>.binding.json`), que se ignora quando está malformado ou nomeia um registo que já não existe.
- **A seleção**, com a recusa de seletores que sejam caminhos (`../x`, `a/b`).

| Verificação | Resultado |
| --- | --- |
| Corpus incluído, um tipo de caso novo (`selection`) | 480 casos novos (6 071 no total), 0 diferenças |
| Corpus alargado, seeds 31 a 33 à escala 40 | 9 600 casos, 0 diferenças |
| Seis mutações deliberadas (estado sem `trim`, estado sensível a maiúsculas, `dirName` falso a decidir, uuid casado pela cabeça, primeiro de vários registos a ganhar, binding a seguir para outro space) | Todas detetadas. A primeira e a terceira passaram na primeira versão do gerador, que foi corrigido até as apanhar |
| `cargo test` | 135 testes |

Três diferenças face ao original, todas recusas onde Focus não tem ainda o que o original usa:

| | Original | Focus |
| --- | --- | --- |
| Linha do registo que a leitura sem tipos não consegue ler (`null`, `status` que não é texto, `uuid` que não é texto numa linha casada pelo slug) | Falha com a mensagem do runtime (`TypeError`) | Falha nas mesmas condições, com uma mensagem que nomeia o ficheiro. O corpus fixa quando falha; o texto é de cada um |
| Worktree delegada (`.focus/worktree-meta.json`) | Resolve para o intent do checkout principal | Recusa: worktrees estão adiadas |
| Sessões registadas por processo (`focus/.focus-sessions/pids/`) sem sessão nomeada | Percorre a árvore de processos para saber que conversa chama | Recusava e pedia a sessão ou `--space` e `--intent`. Fechado em M4 (fatia 3c-i): percorre a árvore, como o original |

As duas últimas têm um teste marcado "Deliberate deviation". Não aparecem no diferencial porque ainda não há comando que use a seleção: entram lá com a fatia 3.

O corpus deixa de fora, de propósito, nomes com `\`: é separador só em Windows, por isso a resposta dependeria da plataforma que repete o corpus.

### Fatia 2b: o shard

O registo de auditoria vai para o git, e cada clone escreve num ficheiro só seu (`<registo>/audit/<host>-<token>.md`, o shard) para que dois clones nunca entrem em conflito de merge. A história de um registo é a leitura de todos os seus shards.

O que existe em `focus-audit`:

- **Identidade do clone** (`ledger`). O token e o host vêm de `focus/.focus-clone-id`, que não vai para o git. Sem ficheiro, o clone cunha um token e grava-o com o nome da máquina. Um ficheiro só com token (formato antigo) é completado com o host que o shard desse token já tem: primeiro o que estiver sob o nome atual da máquina, depois o escrito mais recentemente, e o menor nome em caso de empate. O host fica gravado porque o nome de uma máquina muda (outra rede, uma VPN) e o shard não pode partir-se quando muda. A máquina (nome e gerador de tokens) é um parâmetro, e é assim que os testes e o oráculo a fixam.
- **Onde se escreve.** O shard do clone no registo selecionado; sem intent resolvido, no registo do space. É a seleção da fatia 2a que decide.
- **O que se lê.** Os shards do intent; com space dito e intent por dizer, também os do space, antes; sem intent nenhum, os do space, que é onde a escrita teria ido. Um leitor nunca escreve: sabe o nome do seu shard sem cunhar token nem tocar no ficheiro do clone.
- **Leitura que não confia no disco.** Um shard só é lido se for um ficheiro regular e não estiver atrás de um link: abre-se uma vez sem seguir links e sem bloquear (um FIFO sem escritor prenderia o leitor) e confirma-se que o que se abriu é o que o caminho ainda nomeia. Um shard recusado é listado e reportado como ilegível; uma diretoria atrás de um link é reportada e não é listada. Crescer durante a leitura é normal num registo vivo e não é erro. Um segundo hard link (um backup) não impede a leitura.
- **Vários shards como um registo** (`shards`). Cada evento guarda o shard de onde veio e a sua posição nele, porque a ordem de escrita só se conhece dentro de um shard. Uma nota é um bloco com timestamp, sem evento, com título acima do timestamp.
- **Cópias.** Uma cópia de conflito de uma ferramenta de sincronização, ou feita à mão, repete linhas que outro shard já tem. Só se comparam ficheiros que começam pelo mesmo bloco; entre esses, cada bloco repetido lê-se uma vez, do melhor ficheiro: o shard do próprio clone, depois um com nome de shard, depois o que tem mais blocos, depois a ordem dos nomes.

| Verificação | Resultado |
| --- | --- |
| Corpus incluído, dois tipos de caso novos (`shard_read`, `ledger`) | 660 casos novos (6 731 no total), 0 diferenças |
| Corpus alargado, seeds 7 e 23 à escala 40 | 8 800 casos, 0 diferenças |
| Trinta mutações deliberadas (ordem de preferência entre cópias, agrupamento pelo primeiro bloco, regras de nota e de evento, escolha do host, diretorias lidas, junção dos shards, filtro e ordem dos nomes, leitura do ficheiro do clone) | 27 detetadas. As três que passam não têm efeito observável num só processo: uma linha que não pode ser título e timestamp ao mesmo tempo, o host relido depois de uma corrida entre dois processos, e a identidade em memória quando o workspace não se pode escrever |
| Links, FIFO e hard link | Sete árvores perguntadas ao original em Linux e fixadas como testes unix. Não entram no corpus, que Windows também repete |
| `cargo test` | 152 testes |

O que o oráculo teve de resolver: o nome da máquina e o token cunhado não são do engine. Os dois lados escrevem-nos `{H}` e `{T}` na resposta, e um caso pode nomear a máquina como `{H}` num caminho ou no ficheiro do clone, que é como se pergunta "um shard sob o nome atual desta máquina" sem saber o nome. As datas de modificação dos shards candidatos são fixadas pelo caso.

Uma diferença face ao original:

| | Original | Focus |
| --- | --- | --- |
| Claim de uma unit (`focus/.focus-unit-scope.json`) | Quando o estado diz que as units têm dono e o claim é do intent ativo, o claim pode nomear o shard a escrever; fora disso, o ficheiro é ignorado | Recusa sempre que o ficheiro existe: units entram com o orquestrador, e um claim ignorado escreveria no shard errado |

Tem um teste marcado "Deliberate deviation" e entra no diferencial com a fatia 3, como as da fatia 2a.

O que a fatia 2b não tem: nada escreve ainda num shard. O append sob lock, as verificações do lado da escrita e o comando `audit` são a fatia 3.

### Fatia 3: escrever e ler em disco, e o comando `audit`

O que existe:

- **Abrir um ficheiro em cujo caminho não se confia** (`focus-workspace::files`). O workspace vai para o git: quem consegue fazer um commit consegue pôr um link, um pipe ou um dispositivo onde o engine espera um ficheiro. Três coisas valem em todas as aberturas: o último componente nunca é seguido (`O_NOFOLLOW`; em Windows abre-se o reparse point e não o alvo), abrir nunca espera (`O_NONBLOCK`: um FIFO sem escritor bloquearia no `open`), e o que se abriu é verificado no descritor e comparado com o que o caminho ainda nomeia. Há dois leitores: o de ficheiros só de append (shards), que tolera crescimento e um segundo hard link, e o estrito (ficheiro de estado), que recusa os dois. A identidade de um ficheiro vem de `dev` e `ino` em Unix e, em Windows, da crate `same-file`, porque a biblioteca padrão ainda não a dá.
- **Append** (`focus-audit::append`). Todas as escritas acabam numa função. O shard tem de ficar dentro do projeto, nenhuma diretoria do caminho pode ser um link (verificado antes e depois de criar a diretoria), abre-se uma vez sem seguir links, confirma-se que é um ficheiro regular que o caminho ainda nomeia, escreve-se o título (se o ficheiro está vazio) e o bloco numa só escrita, e confirma-se de novo. Um lote é validado inteiro antes de se escrever e escrito de uma vez: uma entrada recusada não deixa as anteriores gravadas.
- **O lock é o do workspace.** O original tem um lock de auditoria próprio, por intent, numa diretoria temporária. Focus usa o lock que já tem (decisão 3): quem escreve recebe `&WorkspaceLock`. É mais grosso (um por projeto, não um por intent), o que basta enquanto não houver bolts em paralelo.
- **O relógio é da máquina.** `Machine` passou a ter três coisas: o nome, o gerador de tokens e o relógio. Os testes fixam os três. O comando não tem variável de ambiente para fixar a hora: num registo que serve de prova, a hora não é coisa que quem chama possa escolher.
- **Notas.** `append-raw` escreve um bloco livre. Como os leitores encontram um evento pela linha `**Event**:` em qualquer sítio do bloco, um corpo que traga essa linha com um evento do vocabulário é recusado; fora do vocabulário passa.
- **O que o comando recusa escrever.** 17 tipos reservados aos seus donos (sempre) e 49 que carregam autoridade (a não ser com `FOCUS_ALLOW_DIRECT_AUDIT_EVENTS=1`, para fixtures). As duas listas são as do original e são verificadas tipo a tipo no diferencial.
- **História** (`focus-audit::history`, `audit history`). Eventos e notas do registo ativo, de todos os shards, por timestamp; dentro de um timestamp, por posição no shard e por shard. Duas linhas de shards diferentes no mesmo segundo ficam marcadas `unordered`, sem desempate inventado. Exige o ficheiro de estado do registo, lido pelo leitor estrito, e falha inteira se um shard ou uma diretoria não for de confiança. A saída leva sempre o aviso de que o texto do registo é dado e não instrução.

| Verificação | Resultado |
| --- | --- |
| Diferencial, comando `audit` | 125 verificações novas: 109 iguais, 16 desvios deliberados, 0 diferentes. Cada uma é um projeto construído do zero e um ou mais comandos em sequência; compara-se o que cada comando disse e todos os ficheiros do workspace que mudaram |
| Diferencial completo, com fuzz (`--fuzz 600`) | 2 150 verificações: 2 118 iguais, 32 desvios, 0 diferentes |
| Fuzz do `audit`, seeds 1, 5 e 11 | 950 sequências de 3 a 6 comandos aleatórios, cada uma a acabar numa leitura da história, 0 diferentes |
| Os 117 tipos de evento pela linha de comandos, com e sem a variável que levanta a recusa | Iguais, tipo a tipo |
| 24 mutações deliberadas (listas de tipos, a variável, validação de `--limit`, ordenação e empates da história, campos, filtros, notas, título do shard, verificações de links e do ficheiro de estado) | Todas detetadas pelo diferencial; três repetidas contra o fuzz, detetadas |
| Diferencial gravado | 1 339 verificações (118 do `audit`), repetidas em `cargo test` sem Bun, em Unix |
| Em Linux, macOS e Windows | Testes do CLI: append, nota, lote e história de ponta a ponta; 40 appends de 8 processos em paralelo sem um bloco partido |
| `cargo test` | 170 testes |

O que o diferencial teve de resolver para comparar um comando que escreve a hora e pode cunhar uma identidade: um timestamp escrito durante a execução passa a `<now>` (os das árvores de partida são de janeiro e ficam como estão), e o host e o token de uma identidade cunhada durante a execução passam a `<host>` e `<token>`. No diferencial gravado cada verificação do `audit` parte de uma árvore sua, porque uma árvore em que outra verificação já escreveu traria as horas da gravação.

Desvios deliberados, todos no diferencial pelo nome e com teste marcado:

| | Original | Focus |
| --- | --- | --- |
| Recusa lançada dentro de um verbo (tipo desconhecido, campo reservado, JSON inválido de um lote) | Chega ao runtime, que imprime um excerto do código e um stack trace à volta da frase | A mesma frase em `{"error": ...}`, como todas as outras recusas do comando. O diferencial compara a frase e conta estes casos à parte (23 nas verificações com nome) |
| `--field` que não se consegue partir em chave e valor | Descartado; a entrada é escrita sem ele | Recusa: um campo descartado não deixa rasto de ter sido pedido |
| Argumentos que o comando não conhece | Ignorados | `Unexpected argument` |
| `audit-fork`, `audit-merge` | Bifurcam e fundem o registo de um bolt | Recusados pelo nome: bolts e worktrees estão adiados. O texto de uso lista só os verbos que existem |
| Métricas (`FOCUS_METRICS_ENDPOINT`) | Envia uma métrica por entrada | Recusa o append: métricas estão adiadas, e um endpoint que deixa de receber em silêncio é pior do que uma recusa |
| Ficheiro onde devia estar a diretoria `audit` | Mostra o erro do runtime | Diz que shard não se conseguiu abrir |
| Claim de uma unit, worktree delegada, sessões por processo | (fatias 2a e 2b) | Entraram agora no diferencial, por já haver um comando que lá chega |

O que ficou de fora, e onde entra:

- **Observadores só de leitura.** O original recusa qualquer escrita quando o engine corre como sonda (o hook de paragem, a verificação de rota). As sondas são de M4 e M5; a barreira entra com elas.
- **Lock por intent.** Enquanto o lock for o do workspace, dois intents não escrevem em paralelo. Reavaliar com os bolts.
- **Fork e merge do registo de um bolt**, com o orquestrador.

### Fatia 4: settings

Três ficheiros guardam o que uma máquina, um projeto e o clone de uma pessoa decidiram, todos com a mesma forma: o da máquina (`focus.settings.json` na raiz de instalação), o do projeto (`focus.settings.json` na raiz do projeto, no git) e o local (`focus.settings.local.json`, de uma pessoa, fora do git).

O que existe em `focus-workspace::settings`:

- **Um ficheiro lê-se inteiro ou não se lê.** Chave desconhecida, valor do tipo errado ou chave só de máquina num ficheiro de projeto: erro que nomeia o ficheiro. Um erro de escrita não passa por uma definição ausente. Estão portadas as três secções: as chaves de máquina (`update-check`, `offline`, `release-base-url`, `ca-bundle`), `flags` e `models`.
- **Camadas.** O ficheiro mais próximo ganha valor a valor: local, depois projeto, depois máquina. A exceção é a lista de switches gravados (`flags.bypasses`), que é a união dos três: um ficheiro mais próximo acrescenta switches e nunca volta a ligar em silêncio uma verificação que outro desligou. Cada valor fica com a camada de onde veio.
- **O ambiente responde primeiro.** O valor de uma variável para um projeto é o que o ambiente diz, se disser alguma coisa, mesmo vazio; só depois o que os settings gravam. É assim que um processo volta a ligar, só para si, um switch gravado.
- **De onde vem um switch**, para dizer a uma pessoa onde o desligar: `env NOME`, ou `NOME in <ficheiro>` com o ficheiro mais próximo que o grava.
- **Raiz de instalação**: `FOCUS_INSTALL_ROOT`; senão `%LOCALAPPDATA%\focus` em Windows; senão `$XDG_DATA_HOME/focus` ou `~/.local/share/focus`.

Com isto, `validate-grid` passou a avisar quando um switch gravado força uma cerimónia a desligado (limite 2 de M1). Só lê os settings quando o plano tem alguma cerimónia ligada, como o original: um plano sem cerimónias nem dá por um ficheiro de settings inválido.

| Verificação | Resultado |
| --- | --- |
| Corpus incluído, dois tipos de caso novos (`settings_file`, `settings_layers`) | 960 casos novos (7 691 no total), 0 diferenças |
| Corpus alargado, seeds 41 e 42 à escala 40 | 12 800 casos, 0 diferenças |
| 35 mutações deliberadas | 33 detetadas. As duas que passam são a mesma ordenação aplicada em dois sítios: cada uma cobre a outra |
| Diferencial, `validate-grid` com settings | 30 verificações novas em 11 árvores: 29 iguais, 1 desvio deliberado. Três mutações do ponto de ligação, todas detetadas |
| Diferencial completo | 1 380 verificações com nome: 1 347 iguais, 33 desvios, 0 diferentes. 1 369 gravadas |
| `cargo test` | 174 testes |

O gerador da primeira versão era hostil de mais: 62 de 480 ficheiros falhavam logo no `schemaVersion` e quase nenhum chegava a `models.agents`. Passou a gerar metade dos ficheiros sem nenhum valor inválido e a ter um caso escrito à mão por cada recusa a que os gerados raramente chegam.

Uma variável mudou de nome além da regra geral: `AWS_AIDLC_DEFAULT_SCOPE` é a única que o original prefixa de outra maneira, e em Focus é `FOCUS_DEFAULT_SCOPE`, porque todas as variáveis que Focus lê são `FOCUS_*`. O mapa de nomes tem a exceção, e o Markdown de `focus-classic.md` que a menciona foi regenerado.

Um desvio deliberado:

| | Original | Focus |
| --- | --- | --- |
| `release-base-url` | Validado pelo parser de URL do runtime, que aceita muitas grafias do mesmo sítio (`HTTPS://`, uma só barra, espaços à volta, `?` vazio) | Lê uma grafia: esquema em minúsculas, duas barras, host de letras, dígitos, pontos e hífenes, porta e caminho opcionais. O resto é recusado, com a frase do original. Um parser de URL completo para uma chave que só o atualizador vai usar (M8) não se justifica agora |

O corpus deixa de fora, de propósito: números que não cabem num double e metades de pares de substituição (os dois parsers de JSON não concordam, e nenhum ficheiro de settings precisa deles), e valores de `ca-bundle` cuja condição de "absoluto" depende da plataforma.

O que a fatia 4 não tem: escrever settings (`focus config`, M8), e os consumidores das outras flags (`defaultScope`, `swarm`, `hookDebug`, os tempos) e de `models`, que entram com os módulos que as usam.

### Fatia 5: Markdown

O engine lê Markdown que pessoas e agentes escrevem: ficheiros de memória, de estado, de perguntas, reviews. O que pode usar é o texto que um leitor veria como texto. Um título dentro de um bloco de código não é um título, um campo dentro de um comentário não é um campo. Decidir isso pede a estrutura de blocos do documento, como o CommonMark e o GFM a definem.

O crate `focus-markdown` dá duas coisas:

- **`analyse`**: para cada linha da fonte, o tipo de bloco a que pertence (parágrafo, título, regra, código com fence, código indentado, bloco HTML, definição de referência, tabela ou em branco), as citações e itens de lista em que está, a coluna onde começa o seu conteúdo e os troços que são código inline, HTML ou comentário. Espelha `markdownBlocks`.
- **`visible_lines`**: o documento com o mesmo número de linhas, em que uma linha de código ou de bloco HTML fica vazia, um comentário é cortado da sua linha, e uma linha que só continua um span de código ou uma etiqueta fica vazia. Tem as duas opções do original (manter código indentado, manter marcas onde havia comentários). Espelha `visibleMarkdownLines`.

**Como o original o faz, e porque Focus não faz igual.** O `Bun.markdown` renderiza sem dizer onde cada coisa estava na fonte. O original contorna isso: insere uma palavra-sonda em cada linha, renderiza, e vê em que bloco a palavra saiu. Quando uma sonda muda o resultado, divide o conjunto ao meio e tenta outra vez, até 256 renders por documento. Tem ainda três reparações para sítios em que o renderer se afasta do CommonMark (uma tabela que não acaba num título, um fence fechado fora do seu contentor, uma etiqueta indentada sob um parágrafo). A intenção é ler CommonMark; o método é uma aproximação. Focus tem um parser que dá posições (`pulldown-cmark` 0.13.4) e lê a estrutura diretamente.

**A escolha do parser.** Mediu-se o `pulldown-cmark` contra o que o original responde. Nos 391 documentos de `markdown-goldens/corpus.json` (11 052 linhas: 11 048 iguais, 4 que o original não coloca) e nos 256 ficheiros de Markdown do repositório do original (`core/`, `docs/` e a raiz; 68 695 linhas: 59 397 iguais, 9 224 que o original não coloca, quase todas em 16 documentos onde o orçamento de renders se esgota, e 74 em que não chegou a registar um span que lá está) não há uma linha em que os dois leiam estruturas diferentes. Nas 9 228 que o original não coloca, o árbitro lê como Focus. O `comrak` não chegou a ser medido: o primeiro candidato não deixou diferenças por explicar. O `markdown` (markdown-rs) ficou de fora por outra razão: é o mesmo algoritmo do micromark, que faz de árbitro, e um árbitro não pode ser parte.

O parser tem defeitos, e cinco estão contornados em `analysis.rs`, todos com um caso no corpus ou um teste:

- Um bloco HTML aberto por `<script>`, `<pre>`, `<style>` ou `<textarea>` acaba, no CommonMark, na primeira linha com a etiqueta de fecho de qualquer das quatro, em qualquer caixa. O parser só o fecha com a que corresponde à de abertura, em minúsculas; `<pre>` seguido de `</style>` escondia o resto do documento. Focus deteta o bloco que passou do fim, troca a etiqueta nessa linha pela que o parser espera e volta a ler.
- Uma segunda definição do mesmo rótulo continua a ser uma definição, mas o parser guarda a primeira e não diz nada sobre as outras. Focus não toma o silêncio por texto.
- Tabulação vertical e form feed são, para o CommonMark, caracteres como os outros no que toca a blocos; o parser toma-os por espaço. Focus entrega-lhe outro carácter de controlo no lugar deles.
- **O parser entra em pânico** num documento de 16 bytes: um item de lista só com uma definição, seguido de uma linha de espaços indentada para lá do item (`- [a]: b` e depois seis espaços). Uma linha de espaços e uma linha vazia são a mesma coisa para o CommonMark, por isso Focus entrega-lhe a linha vazia. E para o que não se conhece: um pânico dentro do parser é apanhado e vira uma recusa do documento, sem mensagem do runtime.
- Uma declaração `<!x ...` no meio de uma linha acaba, para o parser, no `>` seguinte, mesmo que esse `>` seja o marcador de citação da linha de baixo. Um span que acabe antes de o texto da sua linha começar não é um span, e Focus não o regista.

**Um documento que Focus não consegue ler é recusado, não adivinhado.** `analyse` devolve erro em dois casos: o parser falhou, ou não disse nada sobre uma linha que não é em branco, nem marcadores de contentor, nem uma definição pelo aspeto. `validate-grid` recusa então a proposta a nomear o ficheiro. Em mais de 45 000 documentos (corpus, fuzz e 30 000 de ruído puro) não aconteceu nenhuma das duas.

**Como se compara.** A resposta de Focus não tem de ser igual à do original: tem de ser igual, ou diferente por uma razão que se possa verificar. `tools/oracle/referee/referee.ts` lê cada documento com dois parsers que dão posições, e as respostas ficam ao lado da do original: o micromark (via mdast), com tabelas GFM, é o árbitro; o commonmark.js, a implementação de referência do CommonMark, é a segunda opinião, pedida só onde o árbitro e Focus diferem e só em linhas a que nenhum chama tabela, porque não as conhece. A segunda opinião existe porque o micromark tem leituras próprias em listas aninhadas (uma lista ordenada que não começa em 1, logo a seguir a código indentado, para ele não é lista), e aí é a referência que decide. Cada linha de cada documento recebe um veredicto (`tools/oracle/markdown_rule.py`; o teste do corpus repete a regra em Rust):

| Veredicto | O que quer dizer | Conta como |
| --- | --- | --- |
| igual | o registo de Focus para a linha é o do original | igual |
| por colocar | o original não respondeu nada para a linha, e o árbitro lê-a como Focus | desvio `markdown/commonmark` |
| sem texto | a linha é só espaço e marcadores de contentor, e ou os dois diferem em chamar-lhe em branco ou dentro de um bloco de código ou HTML, ou o original não a colocou | desvio `markdown/commonmark` |
| só o original | os dois diferem e o árbitro lê como Focus | desvio `markdown/commonmark` |
| coluna de reparação | um span continua para uma linha que abre com uma etiqueta indentada; o original dá-o uma coluna antes | desvio `markdown/commonmark` |
| só Focus, ou os três diferentes | o parser de Focus lê de outra maneira | limite `markdown/reading`, com teto |
| port | mesma estrutura, nada por perto que explique a diferença | falha, sempre |

No corpus incluído só os cinco primeiros são aceites. No fuzz ao vivo, o sexto é aceite até 5% dos documentos.

| Verificação | Resultado |
| --- | --- |
| Corpus incluído: 798 documentos (382 dos 391 goldens e 70 dos 71 ficheiros de `core/` já em `fixtures/`, 46 escritos à mão, 300 gerados como um autor cuidadoso escreve) | 27 149 linhas: 26 665 iguais, 443 por colocar, 5 sem texto, 35 só o original, 1 coluna de reparação. 0 por explicar |
| Corpus alargado, seeds 41 e 42 à escala 40 | 4 992 documentos, 141 144 linhas: 135 515 iguais, 5 404 por colocar, 207 só o original, 16 sem texto, 2 colunas de reparação. 0 por explicar |
| Guard Policy em memória, com o leitor novo | 913 casos `guard` e 600 `guardwild`, 0 diferenças; alargado, 20 026 casos, 0 diferenças |
| Fuzz ao vivo, 4 seeds de 2 500 documentos escritos sem cuidado (aninhamento ao acaso, construções por fechar, caracteres trocados) | 239 195 linhas: 216 682 iguais (90,6%), 18 806 por colocar (7,9%), 2 597 só o original (1,09%), 645 sem texto, 3 colunas de reparação, 456 só Focus e 6 com os três diferentes (0,19% no total, em 1,3% a 1,8% dos documentos). 0 de port, 0 recusados |
| Ruído: 30 000 documentos de pedaços de sintaxe ao acaso, com NUL, controlos e caracteres de vários bytes | nenhum pânico, nenhum recusado |
| Mutações deliberadas no crate e no leitor de Guard Policy | 49, em duas rondas: 44 detetadas pelos testes; 2 revelaram código que não fazia nada, que saiu; 2 são equivalentes; 1 é a recusa defensiva, a que nenhuma entrada conhecida chega |
| Mutações contra o fuzz ao vivo | 4, todas detetadas: 3 pelo teto, 1 como diferença de port |
| Diferencial, `validate-grid` com memória | 65 verificações novas em 13 árvores: 56 iguais, 9 desvios deliberados (3 causas) |
| Diferencial completo | 1 445 verificações com nome: 1 403 iguais, 42 desvios, 0 diferentes. 1 434 gravadas |
| `cargo test` | 198 testes |

A primeira ronda de mutações deixou passar 11 de 31. Seis eram linhas que o corpus não tinha (um span de três linhas, uma etiqueta de três linhas, um NUL antes de um span, um rótulo definido três vezes, um item de lista três colunas dentro de uma citação, um bloco fechado na primeira linha): entraram no corpus como documentos escritos à mão, respondidos pelo original. Duas eram a mesma resposta em que a regra aceita mais do que uma, a coluna de um span numa célula de tabela: tem um teste com os valores do original (`tests/reading.rs`). Duas mostraram código que não fazia nada (aparar as linhas em branco depois de código indentado, que o parser já faz; pôr em minúsculas as etiquetas de fecho, que a releitura já cobre), e saiu. Uma é equivalente.

Nove goldens e um ficheiro de `core/` ficam fora do corpus incluído porque contêm uma das três frases que o mapa de nomes muda de comprimento (secção 7): as colunas que o original respondeu deixariam de bater com o documento traduzido. Foram medidos nos nomes do original, com os restantes.

**Revisão independente.** Antes do pull request, um revisor sem acesso ao raciocínio do port leu o crate, a regra e os testes, e correu documentos construídos de propósito contra a biblioteca. Encontrou dez coisas; todas estão corrigidas, com caso no corpus ou teste; a que é do `focus-js` foi corrigida a seguir, num pull request seu ("Entre a fatia 5 e a 6", abaixo):

- o pânico do parser (acima), que o fuzz de 10 000 documentos não tinha apanhado e que um gerador com outra seed apanhava;
- tempo cúbico numa só linha com milhares de spans (8 000 spans: 58 s; agora 0,1 s), por se voltar a percorrer a linha para cada sonda de cada span;
- num item de lista sem parágrafo, uma linha sem texto próprio (o título de um link na linha seguinte, `- ![](x)`) ficava sem bloco e era dada como definição. O conteúdo inline de um item passou a incluir links, imagens e ênfase do princípio ao fim;
- texto de um item depois de uma citação só com código ficava dentro da citação;
- `</ſtyle>` fechava um bloco aberto por `<pre>`: um padrão sem distinção de caixa toma `ſ` por `s`. As etiquetas passaram a comparar-se em minúsculas ASCII;
- o span da declaração que acabava no marcador de citação (acima), com início depois do fim;
- as colunas de um span numa célula de tabela contadas em bytes onde o original conta unidades UTF-16;
- `structured_field` tirava um `\r` solto no fim da última linha, que o original mantém;
- form feed e tabulação vertical (acima).

A regra também ficou mais apertada com a revisão: uma linha que o original não coloca passou a ter de ser lida pelo árbitro como Focus a lê (antes passava sem juízo), e "só marcadores" deixou de aceitar `-->`, `---` e `***`.

**Desvios deliberados.** Um de cada em `crates/focus-markdown/tests/deviations.rs`, com a resposta do original ao lado.

| | Original | Focus |
| --- | --- | --- |
| Linha onde não cabe uma sonda (um marcador de lista sozinho, um `>` sozinho) | Fica `unknown` | Todas as linhas são colocadas; não há esse tipo |
| Documento longo | Pára ao fim de 256 renders e lê o resto como texto. Sessenta tabelas pequenas bastam; um `Mode: strict` dentro de um comentário, depois delas, passa a declarar uma Guard Policy | Lê o fim de um documento como lê o início |
| `> [Answer]:` seguido de `> ---` | Definição e regra, e `ANSWER` na lista de rótulos definidos (Bun 1.4.2; desde 1.4.4 lê como Focus) | Título setext, como no CommonMark |
| `<!x y>` (declaração com letra minúscula) | Parágrafo com uma etiqueta; a linha fica visível, e o que vier a seguir até ao próximo `>` também (Bun 1.4.2; desde 1.4.4 lê como Focus) | Bloco HTML (CommonMark 0.31, condição 4) |
| `[Answer]: <!--<!---->` | Definição (Bun 1.4.2; desde 1.4.4 lê como Focus) | Parágrafo: um destino entre `<>` não pode conter `<` |
| Span de código que fecha no início de uma linha | Acaba o span uma linha antes; a última fica visível | O span cobre a linha onde fecha |
| Span aberto depois de uma crase escapada | Não regista o span; as linhas que cobre ficam visíveis | A crase escapada é texto e o span começa na seguinte |
| Etiqueta indentada quatro colunas sob um parágrafo | Não a regista, e dá um span que continue para essa linha uma coluna antes (Bun 1.4.2; nos dois documentos do teste, desde 1.4.4 lê como Focus) | Regista-a onde está |
| Linha só com um espaço que não é espaço nem tabulação (um espaço não separável) | Em branco | Parágrafo: para o CommonMark é texto |

Em todos, o árbitro lê como Focus. Os três que mudam o que `validate-grid` decide (o documento longo, a crase escapada e a declaração em minúsculas) estão também no diferencial, com nome, e em testes de `guard_memory.rs`. Quatro destes desvios eram do renderer de Markdown do Bun 1.4.2, por onde o original lê: com o Bun 1.4.4 o original passou a ler como Focus (as linhas marcadas acima), os testes deixaram de estar marcados como desvio, e a declaração em minúsculas saiu de `DEVIATIONS` na fatia 4e-ii-c, quando as árvores do diferencial foram regravadas e as suas verificações (`verbs/memory_lowercase_declaration`) passaram a comparar igual.

Um limite, que não é escolha: em cantos de tabelas o `pulldown-cmark` lê diferente do GFM (uma linha de cabeçalho sem `|` sobre um delimitador com `|`; um delimitador que começa como item de lista; uma linha indentada depois de uma tabela), e lê de outra maneira HTML inline que atravessa linhas dentro de contentores. Aí o original e o árbitro têm razão. `tests/deviations.rs` fixa o que Focus faz hoje, para que uma versão nova do parser que o mude apareça, e o fuzz recusa uma subida.

Uma coisa é do original de propósito, embora esteja errada: a coluna de um span de código numa célula de tabela com `\|`. O original mede o span pelo texto renderizado, onde `\|` é um carácter e não dois, e dá-o mais curto. Os sensores leem essas colunas, por isso Focus dá as do original.

**O limite 1 de M1 fechou.** `guard_memory` lê as linhas visíveis com `focus-markdown`: já não há ficheiro de memória recusado por ter Markdown a mais, e os 600 casos `guardwild` do corpus, que antes podiam ser recusados, respondem todos como o original. Do gerador desses casos saiu uma construção, a do span depois de uma crase escapada: é um dos desvios acima.

O que a fatia 5 não tem:

- **Os outros consumidores.** Títulos, campos, perguntas e respostas, e as fontes de afirmações dos sensores leem esta estrutura; entram com os módulos que os usam (M3 em diante), cada um com o seu diferencial.
- **As reparações do original** (`markdownRepairs`). Existem para corrigir o `Bun.markdown`; um parser que lê CommonMark não precisa delas.
- **O que a regra não vê.** O registo comparado não leva a identidade do bloco de uma linha (salvo em blocos HTML), nem os contentores de linhas de código, em branco, de definição e de regra, nem a projeção com as duas opções ligadas. Nenhum consumidor do original os lê. E numa linha julgada "só o original", os spans e as projeções de Focus não têm contra o que comparar.
- **Uma coluna a meio de um carácter.** Numa célula de tabela, quando a conta do original cai a meio de um par de substituição UTF-16, Focus dá o início do carácter. Um `\|` seguido de um emoji dentro de código, numa tabela.
- **Tempo em documentos patológicos.** O contorno do bloco HTML relê o documento uma vez por cada bloco que acabe com a etiqueta de outro ou em maiúsculas: mil desses blocos custam cerca de 0,1 s, quatro mil cerca de 1,4 s. Nenhum documento real tem um.
- **`focus-js`: padrões sem distinção de caixa.** Ficou para um pull request à parte e está feito: é a secção seguinte.

### Entre a fatia 5 e a 6: a flag `i` em `focus-js`

A revisão da fatia 5 encontrou `</ſtyle>` a fechar um bloco aberto por `<pre>`. A causa não estava no Markdown. Os padrões do original são portados tal como estão e corridos no `regress` (decisão 2 da secção 4), e esse motor, na versão 0.12.0, não ignora a caixa como o JavaScript.

**O que o JavaScript faz.** Com `i` e sem `u` nem `v`, dois caracteres são iguais quando têm a mesma forma maiúscula, e só se essa forma for uma única unidade de código; e um carácter fora do ASCII nunca é igual a um de dentro. Por isso `ſ` (s longo) não é `s`, `K` (o sinal kelvin) não é `k` e `ı` (i sem ponto) não é `i`, embora as maiúsculas deles sejam `S`, `K` e `I`. Com `u`, as regras são outras (o *case folding* do Unicode) e aí `ſ` é `s`.

**Onde o `regress` faz diferente.** Mediu-se contra o Bun com todos os caracteres do plano básico que têm outra caixa (2 355), cada um como padrão sozinho, como classe de um só elemento e em sequência com o vizinho, sobre um texto que os contém todos. São quatro sítios:

| Onde | O que o `regress` faz | O JavaScript |
| --- | --- | --- |
| 65 caracteres do plano básico | Toma `ı`, `ſ` e `K` por `i`, `s` e `k` (num literal, numa classe, numa classe negada e numa backreference); toma uma pela outra a minúscula e a maiúscula das 54 letras gregas com iota subscrito; e, só dentro de uma classe, junta o sinal ohm a `ω`, o angstrom a `å`, `ẞ` a `ß`, o símbolo theta a `θ`, duas letras do grego estendido às suas gémeas do bloco grego, e as duas ligaduras de s longo uma à outra | Cada um dos 65 só é igual a si próprio |
| Letras fora do plano básico (Deseret, Osage, Adlam e outras: 520) | Junta a maiúscula à minúscula | São dois *code units* cada, e um *code unit* de um par não tem caixa |
| `\W` dentro de uma classe (`[\W]`, `[^\W\d_]`) | Leva `s` e `k` consigo, porque `ſ` e `K` estão em `\W` | `\W` é o complemento de `[A-Za-z0-9_]` |
| Intervalo de classe que sai do ASCII (`[Ā-ž]`) | Fecha o intervalo sobre os mesmos pares | Não |

Fora destes quatro, as respostas são iguais: letras acentuadas, grego, cirílico e o resto dos alfabetos com caixa comparam-se como no Bun.

**A correção.** Um padrão compilado em `focus_js::re` com `i` e sem `u` nem `v` nunca chega a ver os 65: no texto que lhe é dado, cada um é trocado por um carácter sem caixa com o mesmo comprimento em UTF-8 (`ĸ`, U+0138, para os de dois bytes; `℩`, U+2129, para os de três), e a procura corre sobre esse texto. As posições não mudam, por isso os intervalos de um resultado valem no texto original. A troca só acontece quando o texto tem um dos 65.

Isto é exato enquanto o padrão não conseguir distinguir um carácter sem caixa fora do ASCII de outro, e não tocar nos outros três sítios. Um padrão que o faça é recusado ao ser compilado, com um pânico que diz porquê (um padrão é uma constante vinda do original, e recusá-lo é um erro de port, não uma condição de execução):

- nomeia um dos 65, um dos dois substitutos, ou um carácter fora do plano básico;
- tem numa classe um intervalo que sai do ASCII, ou `\W`;
- tem uma backreference e um grupo que não é uma escolha de palavras ASCII (`(script|style)` passa; `(\S+)` não): o que esse grupo captura podia conter dois caracteres que a troca tornou um.

O original tem 226 padrões com `i` sem `u` nem `v` em `core/`, `harness/` e `scripts/`, contados com o parser do TypeScript (196 literais e 30 construídos; nenhum com `i` e `u`). Quatro nomeiam caracteres fora do ASCII (pontuação, sem caixa), um tem uma backreference (a um grupo de nomes de etiquetas), nenhum tem `\W` numa classe. Os 165 literais distintos de `core/` e `harness/` estão no corpus (`tools/oracle/patterns.ts` extrai-os) e nenhum é recusado. Os 30 construídos interpolam nomes de campos, de agentes e de ferramentas em pedaços do mesmo género; cada um é verificado quando o módulo que o constrói for portado, porque compilar um padrão recusado falha no primeiro teste que o use.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, tipo `icase` | 31 casos, 5 682 padrões: cada padrão responde com tudo o que encontra no texto, e a resposta é comparada com a do Bun. 0 diferenças |
| O que os padrões são | Cada letra ASCII e cada carácter com caixa do plano básico, salvo os 65 (2 342), como literal e como classe; 781 pares de vizinhos; 43 padrões com classes, escapes, classes negadas, `\b` e backreferences; cada letra ASCII seguida de cada carácter com caixa, para a comparação que uma backreference faz; um texto com as 520 letras fora do plano básico; e os 165 padrões do original, sobre o mesmo texto e sobre linhas com `ſ`, `ı` e `K` no lugar das letras que os padrões esperam |
| Sem a correção | Diferem do Bun: `i`, `s`, `k` e as classes que os contêm (`[a-z]`, `\w`); os 54 literais gregos; 18 classes de um elemento; os 520 literais fora do plano básico; `[\W]` e `[^\W]`; os três intervalos fora do ASCII que se experimentaram |
| Mutações em `re.rs` | 31, 30 apanhadas. A que passa é uma guarda de limites que nenhum padrão válido alcança |
| `cargo test` | 203 testes |

A lista dos 65 não pode estar no corpus como padrões, porque Focus os recusa. Está fixada num teste de `focus-js`: são 65, cada um é recusado pelo nome e trocado por um substituto do mesmo comprimento.

**O que isto não tem.**

- **Outra versão do motor.** A lista é a do `regress` 0.12.0, fixado em `Cargo.lock`. Uma versão que compare de outra maneira aparece no corpus: se passar a errar noutro carácter, um dos 2 342 padrões falha; se deixar de errar nos 65, a troca fica a mais e não faz mal.
- **Os modos `u` e `v` com `i`.** O JavaScript usa aí o *case folding* do Unicode e o `regress` também; o original não tem nenhum padrão assim, e o corpus não os mede.
- **Meio par de substituição.** Continua como estava (docs do módulo): sem `u`, um padrão do JavaScript pode consumir metade de um par, e no `regress` não. Nenhum padrão portado o faz sobre UTF-8 válido.

### Antes da fatia 6: o que as fatias anteriores tinham deixado

Ao ler `aidlc-runtime.ts` para a fatia 6 apareceram três coisas que as fatias 1 a 4 e M1 não tinham, todas nas escritas. Entram num pull request seu, antes do runtime graph, que escreve pelos mesmos caminhos.

**A barreira de observador.** Há invocações do engine que só servem para saber uma coisa: o hook de Stop pergunta-lhe o que faria a seguir, e `unit start` pergunta que unit encaminharia. Cada uma marca-se no ambiente (`FOCUS_STOP_HOOK_PROBE=1`, `FOCUS_ROUTE_CHECK=1`), e a partir daí o processo é um observador: não pode mudar um byte do projeto. No original, cada primitiva de escrita durável começa por verificar isso (`refuseEngineObserverWrite`) e recusa com uma frase que diz que primitiva foi alcançada. Focus não tinha a barreira: um observador que chegasse a `audit append` ou a `graph compile` escrevia.

`focus-workspace::observer` tem agora a verificação, e chamam-na os sítios que espelham as primitivas do original que Focus já tem: a escrita de um shard (`appendAuditBlockAtPath`) e cada escrita atómica de dados do projeto (`writeFileAtomic`: os dois ficheiros compilados, o ficheiro de identidade de um scope composto reposto, o registo de um scope criado, o plano de `resolve`, a atualização do ficheiro de identidade do clone). A frase é a do original, com os nomes que ele dá às primitivas, para se ler igual dos dois lados.

Dois pormenores que o diferencial mostrou e que ficam iguais: um observador que faz `audit append` num clone sem identidade ainda a cria (essa escrita não passa pela barreira no original), e a atualização de um ficheiro de identidade antigo é recusada em silêncio (a recusa cai no mesmo `catch` de um disco só de leitura) antes de o append ser recusado com a frase.

**`resolve` escreve o plano.** Era o último desvio de M1 que dependia de saber qual é o registo. `focus engine graph resolve <scope>` escreve agora `plan.json` no diretório do engine do registo que o projeto resolve (o intent ativo, o único registo, o da sessão; sem intent, o do space), cria esse diretório, e imprime o caminho. `FOCUS_PLAN_PATH` continua a ganhar, e aí o diretório não é criado. O original calcula o destino antes de olhar para `--stdout`, por isso um registo de intents que não se consegue ler pára as duas formas; Focus faz o mesmo.

**A escrita atómica não cria diretórios.** `write_atomic` criava os diretórios em falta até ao ficheiro; `writeFileAtomic` não cria nenhum, e quem chama é que os cria onde o original cria (`mkdirSync` antes de repor um scope, de criar um registo, de escrever o plano por omissão). Com `FOCUS_PLAN_PATH` a apontar para um diretório que não existe, Focus escrevia e o original falhava; agora falham os dois. Um valor vazio também conta como caminho, como no original (`??`), e escrever em lado nenhum falha.

| Verificação | Resultado |
| --- | --- |
| Diferencial, `verbs/resolve_writes/*` (14) | Sem intent, um registo, intent ativo, dois registos sem cursor, outro space, plano já existente, scope desconhecido, `--stdout`, caminho no ambiente (existente, em diretório em falta, vazio): iguais, ficheiros incluídos. 2 desvios, abaixo |
| Diferencial, `verbs/observer/*` (18) e `audit/observer/*` (15) | Cada marcador com `resolve`, `compile`, `compile --check`, a reposição e a criação de registos de scopes, `topo`, `append`, `append-batch`, `append-raw`, `history`, criar e completar a identidade do clone: iguais. Um valor que não é `1` não faz um observador |
| Diferencial completo | 1 493 verificações com nome: 1 449 iguais, 44 desvios, 0 diferentes. 1 482 gravadas |
| Mutações | 18, 17 apanhadas. A que passa é a barreira na criação do registo de um scope: num compile, a escrita dos ficheiros compilados recusa primeiro |
| `cargo test` | 207 testes |

Desvios, os dois já declarados noutros comandos e agora também em `resolve`:

| | Original | Focus |
| --- | --- | --- |
| `resolve` num projeto com sessões registadas por processo | Percorre a árvore de processos para saber que conversa chama, e escreve no registo dela | (Fechado em M4, fatia 3c-i.) Recusava escrever; com `--stdout` imprimia o plano, que não precisa de registo |
| `resolve` com uma linha do registo de intents que não se lê | Falha com o que o runtime diz do valor (`undefined is not an object`) | Diz que ficheiro e o que uma linha precisa. Os dois falham antes de imprimir |

O que isto não tem: as outras primitivas com barreira no original (`writeStateFile`, `writeBufferAtomic`, `removeRecordFileNoFollow`, `clearSummaryAuthorization`, a transação da diretiva ativa) pertencem a módulos por portar e trazem a sua chamada quando entrarem. A regra fica em `CLAUDE.md`.

### Fatia 6: runtime graph

`stage-graph.json` diz que stages existem e como dependem uns dos outros. O runtime graph é o seu par para um workflow em curso: que stages começaram e quando, quais foram aprovados, quem os conduziu, quantas entradas tem o diário de cada um, que sensores dispararam e como acabou cada disparo, o que se aprendeu em cada gate e, num stage construído por vários bolts em paralelo, uma linha por bolt. Os gates, o ciclo de vida e o resumo da sessão leem-no em vez de percorrerem a auditoria.

Nada nele é decidido: cada campo é uma leitura da auditoria, do ficheiro de estado e dos diários, e compilar duas vezes a mesma auditoria dá os mesmos bytes. O "agora" nunca é o relógio; é o timestamp mais recente que a auditoria tem.

**O comando.** `focus engine runtime` é o `aidlc-runtime.ts` do original:

- `compile` lê todos os shards do registo ativo, o ficheiro de estado, os `memory.md` dos stages e o bloco de units, e escreve `runtime-graph.json` no registo. Escreve ainda um evento `MEMORY_EMPTY` por cada stage aprovado com o diário vazio, uma vez por aprovação, e as duas escritas acontecem dentro do mesmo lock, o evento primeiro.
- `read <stage>` imprime a linha de um stage.
- `summary [--json]` imprime os números do grafo: stages por resultado e por fase, entradas de diário por tipo, sensores por desfecho, aprendizagens, duração. As skills de sessão tiram daqui todos os números que mostram, para que nenhum modelo conte nada.

**O que existe.**

- `focus-audit::runtime_graph`: a leitura. Não toca em ficheiros: recebe a auditoria como texto e duas maneiras de perguntar (onde está um stage no stage graph, o que tem o diário de um stage) e devolve o grafo, os diários vazios a registar e o que o original diria no stderr. É aqui que estão as regras: o workflow é o último iniciado; a linha de um stage é o seu início mais recente e a conclusão que se lhe segue; um `--single` não entra; num stage de Construction com dois ou mais bolts bifurcados dentro da sua janela, cada bolt tem a sua linha e o stage herda o pior resultado; um disparo emparelha com o seu desfecho pelo `Fire id`, nunca pela posição; num stage fechado, um disparo sem desfecho é `incomplete`, e num stage aberto só o é passado um minuto.
- `focus-audit::memory`: as contagens de um diário (`parseMemoryHeadings`).
- `focus-audit::units`: o bloco `units:` de `unit-of-work-dependency.md` e a ordem em que as units podem ser construídas (`parseBoltDag`), com o scanner do original, que não usa um parser de YAML.
- `focus-audit::summary`: os números e a página.
- `focus-state`, o crate do ficheiro de estado, começa aqui com o que o resumo precisa: as linhas de checkbox de Stage Progress (`parseCheckboxes`, `parseStateStageSuffixes`) e o que um workflow completo diz dos seus stages. O resto do ficheiro de estado é M3.
- `focus-js::date`: `Date.parse`, para os timestamps que o engine escreve (ver abaixo).

**`Date.parse`.** O original pergunta ao JavaScript há quanto tempo disparou um sensor e quanto durou um workflow. O `Date.parse` lê o formato de data do ECMAScript e, quando o texto não está nele, passa-o a um segundo parser, permissivo, cujas regras são do runtime e cuja resposta pode depender do fuso da máquina. Medido no Bun: `2026-02-29T00:00:00Z` é o dia 1 de março; um `t` minúsculo serve; um texto sem fuso é lido na hora local; uma fração de dez ou mais dígitos é lida por regras próprias (`.0674309079` dá 674 milissegundos).

Focus responde por uma forma só, a de todos os timestamps que o engine escreve: `AAAA-MM-DDTHH:MM[:SS[.fração]](Z|±HH:MM)`, numa data que o calendário tem, com fração de um a nove dígitos. Sabe mais uma coisa: um texto sem nenhum dígito não é data para parser nenhum, o que cobre o que as pessoas de facto deixam no lugar de um timestamp (uma palavra, nada). Para tudo o resto não tem resposta, e quem precisar dela recusa.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, tipo `instant` | 19 casos, 1 831 textos (1 096 são datas, 735 não): instantes da forma, com datas de fim de mês e de 29 de fevereiro, anos de 0000 a 9999, frações de 1 a 9 dígitos e fusos até ±23:59; textos partidos de onze maneiras; palavras sem dígitos. 0 diferenças do Bun |
| Corpus, tipo `memory_counts` | 242 diários, com os quatro títulos e as suas variantes, fences, citações, comentários, BOM, CRLF e as doze linhas que são nomes de membros de `Object.prototype`. 0 diferenças |
| Corpus, tipo `bolt_dag` | 333 documentos: os legíveis, com uma ou várias levas e com `kind`, e cada uma das catorze recusas. 0 diferenças |
| Corpus, tipo `progress` | 180 ficheiros de estado. 0 diferenças |
| Diferencial, `runtime/*` | 138 verificações com nome: 95 iguais, 43 desvios, 0 diferentes. Comparam o que cada comando diz e todos os ficheiros do workspace que cria ou muda, `runtime-graph.json` byte a byte |
| Fuzz, `fuzz/runtime` | 3 600 auditorias aleatórias em seis seeds, corridas depois das correções da revisão (workflows como um engine os regista, com ruído por cima: qualquer evento a qualquer hora, rótulos escritos de todas as maneiras, vários shards, CRLF), cada uma compilada duas vezes, resumida e lida: 0 diferentes. Tinham 13 269 stages (2 143 construídos por bolts, com 5 557 bolts), 22 389 disparos de sensores dos quatro desfechos, 3 187 stages com aprendizagens, 4 205 com diário, e os dois motores escreveram 2 891 eventos `MEMORY_EMPTY` cada. O fuzz imprime estes números, para se ver o que comparou |
| Mutações | 114, em três rondas (62, 23 e, depois da revisão, 29). Passaram 18, 1 e 2; cada uma deu um teste, um caso de corpus ou a remoção de código que estava a mais, e repetida é apanhada. Fica uma, equivalente: no leitor do bloco de units, retomar a procura na linha que fecha o bloco em vez de na seguinte (essa linha não abre bloco nenhum) |
| Diferencial completo | 1 631 verificações com nome: 1 544 iguais, 87 desvios, 0 diferentes. 1 618 gravadas, sobre 259 árvores |
| `cargo test` | 262 testes |

**O limite de desvios na gravação mudou.** Um desvio é gravado e não é repetido por `cargo test`, por isso uma gravação feita sobretudo de desvios provaria pouco, e há um teste que o vigia. Exigia menos de um vigésimo do total; com esta fatia a gravação tem 83 em 1 618, acima disso, porque o `runtime` tem muitas recusas declaradas (41 das suas 136 verificações gravadas: cada forma de ficheiro que o engine não escreveu, cada timestamp que não lê). O teste passou a exigir menos de um décimo no total e, comando a comando, no máximo um terço. Foi o limite que mudou, não as verificações: cada desvio continua a ter o seu teste marcado, que corre sem Bun.

**Pormenores do original que ficam iguais**, porque o grafo é comparado byte a byte:

- Num diário, uma linha que seja exatamente `constructor`, `toString` ou outro nome que todo o objeto JavaScript herda fecha a secção em que está: o original procura a linha num objeto simples com `in`. As entradas a seguir não contam para nada até ao próximo título.
- O caminho do diário numa linha do grafo, quando não há intent, é o do space `default`, seja qual for o space ativo.
- `**Stage**:` sem valor lê o valor da linha seguinte (`\s*` atravessa a quebra de linha).
- O ficheiro de identidade de um clone novo é criado por um compile que tenha um `MEMORY_EMPTY` a escrever, como por qualquer append.

**Desvios deliberados.** Cada um tem um teste marcado, uma entrada em `DEVIATIONS` e uma linha aqui.

| | Original | Focus |
| --- | --- | --- |
| Sensor sem desfecho num stage aberto, com um timestamp (o dele ou o mais recente da auditoria) que tem dígitos e não é um instante com fuso | Pergunta ao `Date.parse`: lê muitas formas pelas regras do runtime e, sem fuso, na hora local da máquina | Recusa o compile e diz que timestamp não leu. Uma palavra no lugar do timestamp não é data para nenhum dos dois, e o disparo fica de fora nos dois |
| `summary` com um timestamp desses no início do workflow ou na última conclusão | A duração é a diferença de dois `Date.parse` | Recusa, pela mesma razão |
| `runtime-graph.json` que não tem a forma que o engine escreve | Falha com o que o runtime diz do valor (`undefined is not an object`), conta debaixo de um nome que não é resultado, ou lê sem dizer nada (uma contagem que é texto, aprendizagens ou contagens de diário que não são um objeto, contagens cuja soma passa do maior inteiro exato) | Diz que o ficheiro não é um runtime graph, o que tem de errado e como o escrever de novo |
| Uso | Lista `fragment-fork` e `fragment-merge` | Lista os verbos que tem; recusa esses dois pelo nome (vêm com bolts e worktrees) |
| Argumento desconhecido, flag repetida, flag sem o seu valor | Ignorados (sem valor, `--project-dir` é o diretório corrente) | Recusados |
| `FOCUS_METRICS_ENDPOINT` definido e um `MEMORY_EMPTY` a escrever | Envia a métrica | Recusa o append, e com ele o compile: não escreve evento nem grafo. Sem evento a escrever, compila |
| Sessões registadas por processo; worktree delegada; claim de unit | Resolve cada um | Recusa, como no comando `audit` (os mesmos três desvios, pelas mesmas funções). As sessões por processo fecharam em M4 (fatia 3c-i) |
| `stage-graph.json` que não tem a forma que o compile escreve: um stage que o workflow usou sem fase em texto; num resumo, uma fase que não é texto; uma linha `null`; um ficheiro que não é uma lista | Falha com o que o runtime diz do valor (`The "paths[1]" property must be of type string, got undefined`, `null is not an object (evaluating 's.enabled')`) ou, no resumo, conta o stage debaixo do valor escrito como texto (`7`) | Diz que o stage graph não é o que o compile escreveu e como o compilar de novo. O que o compile podia ter escrito lê-se como no original: um slug em duas linhas é a segunda; um stage sem fase que o workflow não usou não impede nada; num resumo, sem fase é `unknown` |
| Fase com nome de membro de objeto (`constructor`) num resumo | Guarda as fases num objeto simples: conta o stage no valor herdado e não lista a fase | Lista-a como qualquer outra |
| JSON que o `JSON.parse` lê e o leitor de Focus não guarda: um número que não cabe num double (`1e400`), metade de um par de substituição escrita com escape (`"\ud800"`), mais de 127 níveis | Lê | Diz que o ficheiro não é JSON que leia. Vale para todo o JSON que Focus lê e está fixado em `focus-js` |
| O diretório do workspace é um link | Deixa de fora os shards atrás do link, não vê workflow nenhum e escreve um grafo vazio através dele | Não toma o lock nem escreve através de um link, e diz qual é |
| Barra invertida no caminho do projeto | O leitor de shards toma-a por separador, não encontra nenhum e compila um grafo sem stages (Bun 1.4.2; desde 1.4.4 lê a auditoria, e o desvio foi levantado na fatia 4e-ii-c) | É um carácter do nome, e a auditoria é lida |

**O que a fatia 6 não tem.**

- **`fragment-fork` e `fragment-merge`.** Copiam o grafo para a worktree de um bolt e retiram-no. Entram com bolts e worktrees.
- **Quem chama o compile.** No original é o hook de `PostToolUse` depois de cada evento que faz avançar um workflow; os hooks são M4 e M6. Até lá o comando existe e corre à mão.
- **A métrica de cada evento** (`tapAuditMetric`), como no comando `audit`.
- **`Date.parse` fora da forma.** As outras formas que o Bun lê não estão portadas e não vão estar: as regras são do runtime e, sem fuso, da máquina. Um timestamp escrito pelo engine nunca cai aqui.
- **O stage graph de outro projeto.** O original lê o `stage-graph.json` que está ao lado da ferramenta (ou o que `AIDLC_STAGE_GRAPH` indicar, uma costura de testes que Focus não tem), seja qual for o `--project-dir`; Focus é um binário e lê o do projeto que lhe indicam. Num projeto instalado são o mesmo ficheiro. Lido, não medido: o diferencial corre sempre a ferramenta do próprio projeto.
- **O registo é escolhido uma vez.** O original volta a resolver o registo ativo em cada caminho que calcula; Focus resolve-o no início do comando. Só se nota se o cursor mudar a meio de um compile.
- **Caminhos gravados com letra de drive e sem raiz** (`C:wt\a`, no Windows). O original resolve-os contra o diretório corrente desse drive; Focus contra o projeto. Lido, não medido: o diferencial corre em Linux.
- **As palavras de um erro do sistema.** Quando um ficheiro do registo não se consegue ler (um diretório no lugar dele, falta de permissão), os dois motores falham no mesmo passo e cada um diz o erro com as palavras do seu runtime. O diferencial compara o passo, o código de saída e os ficheiros, não essas palavras.

**Revisão independente.** Antes do pull request, a fatia foi lida por um revisor que não a escreveu, com o original a correr ao lado. Reproduziu dezanove diferenças e apontou mais cinco só por leitura; nenhuma estava no que o corpus, o diferencial e o fuzz comparavam, todas em entradas que eles não geravam. Correu ainda 283 casos seus sobre campos e separadores, diários, bolts e sensores, units, ficheiro de estado, auditorias grandes ou em codificações erradas e ficheiros que são diretórios, sem encontrar outra diferença.

- **Corrigidas** (Focus fazia diferente do original sem razão): um slug em duas linhas do stage graph (ganha a segunda); o caminho do diário quando a fase parece um caminho absoluto ou o slug é `..` (as partes juntam-se como no `join` do Node, sem recomeçar na raiz); `--project-dir` com um `..` depois de um link, que fazia Focus escrever um grafo vazio por cima do grafo certo (o caminho do projeto passa a ser resolvido no texto, como no original, antes de qualquer leitura); um observador deixava `focus/.focus-lock` criado (um observador já não toma o lock, porque vai ser recusado na escrita); `learnings_captured` e `memory_breakdown` com um valor que não é objeto nem `null`, e contagens que somadas passam do maior inteiro exato, eram lidos em silêncio (são recusados como grafo que o engine não escreveu); uma linha `null` no stage graph era saltada (é recusada).
- **Declaradas**: as seis linhas do fim da tabela de desvios, cada uma com o seu teste, a sua entrada em `DEVIATIONS` e a sua verificação no diferencial.
- **Anotadas** em "o que a fatia 6 não tem": quatro das que só se leram. A quinta é o parágrafo a seguir a este.

Da revisão saíram ainda verificações para o que estava certo e ninguém media: o "agora" da auditoria (move-o um início de stage ou uma linha de sensor, e mais nenhuma), disparos sem desfecho dentro de bolts abertos e fechados, dois desfechos no mesmo instante, nomes de bolts fora do plano básico (a ordem é por unidade UTF-16), e um diretório no lugar de cada ficheiro que o compile lê ou escreve. Uma das verificações que já existiam para o "agora" não media nada (o disparo tinha trinta segundos nos dois casos) e foi corrigida.

**Os ficheiros do registo abrem-se pelo nome.** O ficheiro de estado, os diários, o bloco de units e o grafo compilado são lidos como o original os lê aqui, com `readFileSync`, seguindo links. O que contêm não decide nada: o grafo é uma leitura, e tudo o que tem autoridade volta a ser lido da auditoria, cujos shards são abertos sem confiar no caminho. A regra de `CLAUDE.md` sobre ficheiros do workspace diz agora a que ficheiros se aplica.

## 14. Estado de M3

M3 está feito. O que o plano chamava "estado e criação de intents" é, no original, `aidlc-state.ts` (8 867 linhas, 33 subcomandos), `aidlc-jump.ts`, `aidlc-validity.ts` e a parte de `aidlc-utility.ts` que cria intents e mostra o estado; atrás deles, outra vez, está `aidlc-lib.ts`: a resolução da Guard Policy, as fences e a autoridade de uma ação, que as transições de stage consultam antes de mexer no ficheiro de estado. Entrou de baixo para cima, como M2.

| Fatia | O que traz | Como se verifica | Estado |
| --- | --- | --- | --- |
| 1. Guard Policy e fences | `focus-guards`: a resolução da política de um workflow (a linha do estado, o default do scope, as camadas de memória), as linhas `Guards Off` e `Guards On`, os kill switches, o que cada política baixa, a decisão de uma fence e a autoridade de uma ação sem marcadores de hooks | Corpus | Feita |
| 2. O ficheiro de estado como dados | `focus-state`: campos, checkboxes, listas de refs, progresso de fase; ler e escrever o ficheiro (barreira `writeStateFile`, ficheiro só de leitura); `focus engine state get`, `set`, `checkbox` e `count`, com a porta das transições e o `ERROR_LOGGED` de cada recusa | Corpus e diferencial | Feita |
| 3. Criação de intents | `intent-create`: o registo, o cursor, o ficheiro de estado a partir do template; antes dele, o scanner do workspace que lhe classifica o projeto (`detect`) | Diferencial | Feita (3a `detect`, 3b `intent-create`) |
| 4a. Transições de stage | `advance`, `finalize`, `complete-workflow`, `skip` e `resume`, em série e sem units: `focus-lifecycle`. Cada transição passa pela cadeia de admissão do original (`admitStageAction`): o workflow não arquivado, os artefactos declarados da stage, o trabalho de código de uma stage `workspace_requires`, a evidência da confirmação do sumário, os recibos do reviewer, os handoffs de um pipeline. As evidências são registos que `aidlc-log` escreve e que comparam a identidade da fonte (a atestação): são M5, e uma transição que as deva recusa-se pelo nome até lá; com a cerimónia `summary_confirmation` desligada, a classe de review `none`, uma stage sem reviewer ou os kill switches, o original não deve nenhuma e a transição corre inteira. Uma stage de Construction (checkpoints, units, swarms: M4) recusa-se pelo nome. A conclusão deixa o recibo de validade (`Validation Basis`) | Diferencial e fuzz de sequências | Feita |
| 4b. Os verbos de gate | `gate-start`, `approve`, `reject`, `revise`: o gate de aprovação de uma stage, a presença da pessoa (os turnos humanos da auditoria contra as decisões que os consomem), a leitura da resposta (`--user-input`, `--feedback`: a boilerplate de cancelamento, a decisão que o agente atribui a si próprio), os campos do pedido de aprovação (a stage seguinte, o digest do brief, o estado dos sensores, as decisões em aberto), o backstop de revisão, a recusa "para o agente" com a orientação que o original imprime. Os sensores de gate (a ferramenta de sensores), as palavras do gate (o hook de turno), os findings de uma review (o brief) e os gates de unit são M4/M5: recusam-se pelo nome | Diferencial e fuzz de sequências | Feita |
| 5a. Parar e saltar | `park` e `unpark` (onde o trabalho parou e de quem foi a paragem: a presença da pessoa contra as aprovações que a sua mensagem sobrevive), `jump resolve` e `jump execute` (para onde um salto leva e o que passa à frente ou repõe, as linhas de fronteira de fase, o `STAGE_JUMPED` com os artefactos e as reviews que um salto para trás invalida). `reopen`, `execute --units`, os gates de units de uma equipa, o marcador de diretiva, as linhas pendentes de um chat e os registos de review são M4/M5: recusam-se pelo nome | Diferencial e fuzz de saltos | Feita |
| 5b. Validar e mostrar | `validity` (a segunda metade: ler os recibos de volta, compará-los com a árvore, propagar a invalidação pelas dependências observadas, a frase que a pessoa ouve) e `utility status` (o workflow como a pessoa o lê: projeto, scope ou plano, tipo de projeto, profundidade, fase e stage, quem está nela, Guard Policy e checks desligados, cerimónias, contagem e barras de fase, uma stage atrasada, última e próxima stages, outro trabalho aberto). O board de equipa e o marcador de diretiva são M4/M5: recusam-se pelo nome | Diferencial | Feita |

### Fatia 1: Guard Policy e fences

Antes de mexer no ficheiro de estado, cada transição do original pergunta se pode: `aidlc-state.ts set` ou `advance`, chamados diretamente e não pelo orchestrator, passam pela fence `state-transition`, e só avançam se ela estiver em baixo. Saber se está em baixo é resolver a Guard Policy do workflow e, a partir dela, o estado de cada fence. É uma leitura sem efeitos, e por isso a primeira fatia.

**O que existe.** O crate `focus-guards`, que começa aqui:

- `policy`: a resolução da Guard Policy (`resolveGuardPolicy`). A linha do estado (`Guard Policy`, ou o nome retirado `Change Control`, lida mais uma versão) ganha ao default do scope; uma camada de memória que declare `strict` ganha a tudo e fica com o nome do ficheiro como fonte; uma camada `relaxed` ou `off` (a mais estreita) substitui um valor que veio do scope ou não estava definido, nunca o que a pessoa escolheu; duas linhas que discordem resolvem para `strict`. O default do scope vem dos ficheiros de scope do harness, `off` para um que não declare nada e `strict` para um que não exista. A escrita da linha (`setGuardPolicyLine`) e dos rótulos (`off (from scope classic)`).
- `fences`: as cinco fences e a escada que decide cada uma (`resolveFences`): o kill switch do ambiente ou das settings, depois `Guards Off` salvo quando a memória segura `strict`, depois `Guards On`, depois a palavra da política (`off` baixa quatro, `relaxed` duas, `strict` nenhuma), depois em cima. As linhas `Guards Off` e `Guards On` lidas e escritas.
- `authority`: a autoridade que cobre uma ação e a decisão (`decideGuard`): uma fence em baixo afasta-se, uma em cima segura, e nem uma mensagem da pessoa nem uma instrução do engine a baixam. A frase que a pessoa ouve quando uma fence se afasta.
- `workflow`: `Guards::read` junta o que uma decisão precisa de um projeto (o estado do registo, o default do scope, a memória do space, as settings), e `decide` responde por uma fence.
- `focus-state::fields`: os campos do ficheiro de estado lidos e escritos como texto (`getField`, `setField`, `setFieldStrict`, `setOrInsertField`, `removeField`, `setPhaseProgress`, `appendUnderHeading`, as listas de refs), e em `progress` a escrita de uma checkbox (`setCheckbox`, `setStageSuffix`, `countCheckboxes`). As fences escrevem linhas do estado, e é por aqui que o fazem.
- `focus-js::re::replace`: o `String.prototype.replace` do JavaScript com uma string de substituição. Os escritores do estado colam o valor por essa via, e os padrões `$` que o valor tenha (`$1`, `$&`, `$$`) são expandidos como lá. O original sabe-o: cita o `$` do comando de verificação antes de o guardar. Focus faz o mesmo em vez de corrigir o que seria uma diferença.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, tipo `guard_policy` | 360 casos: um ficheiro de estado (linhas `Guard Policy` e `Change Control` com os treze rótulos e as formas partidas, `Guards Off` e `Guards On`, CRLF, ruído), os defaults de até cinco scopes, até três camadas de memória com os dois títulos, os cinco kill switches e `FOCUS_UNATTENDED`. Para cada caso, a resolução inteira, as cinco fences e a decisão de cada uma. 0 diferenças |
| Corpus, tipo `guards_line` | 150 casos: ler e escrever as três linhas. 0 diferenças |
| Mutações | 38, em `focus-guards`, nos campos e checkboxes de `focus-state` e no `replace` de `focus-js`. 36 apanhadas; as duas que passam são equivalentes (o `\b` depois da palavra da política, que o fim da linha já impõe; `none` comparado sem baixar a caixa, que a leitura dos nomes a seguir também não reconhece). Uma terceira equivalente foi retirada do código |
| `cargo test` | 282 testes |

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| Um registo com um marcador de autoridade (`active-directive.json`, `human-turn`, `engine-touch` no diretório do engine) | Lê neles quem cobre a ação | Recusa decidir: os marcadores são escritos pelos hooks (M4) e pelo orchestrator (M5), que não existem. O teste está marcado; a verificação no diferencial entra com o comando `state` (fatia 2), que é o primeiro a decidir uma fence. Desde a fatia 3b os marcadores de turno são lidos, e só o `active-directive.json` recusa |
| A frase de uma recusa nomeia o skill de entrada | `/aidlc` nos três harnesses de Focus (`$aidlc` só no Codex, que Focus não tem) | `/focus` |

**O que a fatia 1 não tem.**

- **A autoridade com marcadores.** `authorityFor` lê o marcador da diretiva ativa, os marcadores de turno e o ledger de dispatches em curso; sem eles, a autoridade é "nada cobre a ação", e é isso que Focus responde. Com eles, recusa (desvio acima).
- **Quem age sobre a decisão.** Os hooks que seguram uma fence e a linha `GUARD_STOOD_ASIDE` na auditoria entram com quem os chama: o comando `state` na fatia 2, os hooks em M4.
- **As ceremónias por scope** (`sensors`, `learnings`, `summary_confirmation`, `plan_approval`, `collaborators`) e os switches do `config set`: leem-se ao lado disto no original e entram com os comandos que os usam.

### Fatia 2: o ficheiro de estado como dados, e o comando `state`

`focus engine state` é o `aidlc-state.ts` do original nos verbos que leem e escrevem o ficheiro pelo nome: `get <campo>`, `set <campo=valor>…` (com `NOW`, `+1` e `-1`), `checkbox <stage=estado>…` (que acerta o contador `Completed`) e `count <estado>`. Os outros 29 verbos do original são recusados pelo nome, cada um com uma frase que diz o que faz; os de M3 entram nas fatias 4 e 5.

**O que está antes de uma escrita.** O workflow não pode estar arquivado. E uma escrita genérica é do engine: um `set` ou `checkbox` que não venha do orchestrator (`FOCUS_STATE_TRANSITION_OWNER` com o pid do processo pai, ou `FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS=1`) passa primeiro pela fence `state-transition`, que a fatia 1 decide. Com a fence em baixo, a escrita segue com uma linha no stderr (salvo com a política `off`, que já a disse quando foi posta) e uma linha `GUARD_STOOD_ASIDE` na auditoria; em cima, é recusada com a frase do original, que diz como a pessoa baixa a fence, ou porque não pode (a memória da equipa segura `strict`; uma política que não se lê).

**O que uma recusa deixa.** Cada recusa imprime `{"error": …}` no stderr, sai com 1 e, quando o registo tem ficheiro de estado, escreve uma linha `ERROR_LOGGED` na auditoria com a ferramenta, o comando e o erro, com o caminho do projeto redigido nos dois (`redactProjectDirPrefix`, agora em `focus-workspace::project`). A recusa de uma seleção que Focus não resolve, que o original faz antes de tudo isto, é impressa sozinha.

**O que existe.** `focus-state::file` (`readStateFile`, `writeStateFile`: a barreira de observador, a recusa de um ficheiro só de leitura, que uma escrita atómica passaria por cima sem dar por isso), `focus-cli::state`, e a redação do caminho do projeto.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `state/*` | 68 verificações com nome: 52 iguais, 16 desvios, 0 diferentes. Comparam o que cada comando diz e todos os ficheiros do workspace: o ficheiro de estado byte a byte, as linhas `GUARD_STOOD_ASIDE` e `ERROR_LOGGED` campo a campo |
| Diferencial completo | 1 699 verificações: 1 596 iguais, 103 desvios, 0 diferentes. 1 685 gravadas, sobre 285 árvores |
| Mutações | 28 no comando, na escrita do ficheiro e na redação; 24 apanhadas. Duas eram equivalentes e o código foi simplificado (a redação do caminho, que o renderizador das linhas da auditoria já faz desde M2, e uma flag que dizia se uma recusa se grava, quando uma recusa de seleção nunca encontra registo onde se gravar); uma é equivalente por construção (`!= Hold` para `== StandAside`); e a da recusa de um ficheiro só de leitura só se apanha sem ser root, como o CI corre |
| `cargo test` | 294 testes |

**Pormenores do original que ficam iguais.**

- `set` num campo que não existe não faz nada e conta o campo como atualizado; `+1` num campo que não é número dá `NaN`, e `-1` também (`Math.max(0, NaN)`).
- Um valor com padrões `$` é colado como o `replace` do JavaScript o cola (fatia 1).
- O lote de um `set` ou `checkbox` é verificado inteiro antes de qualquer escrita.
- O `GUARD_STOOD_ASIDE` é escrito só quando o shard deste clone existe, e a sua falha não pára a escrita; a linha no stderr diz "Recorded in the audit trail" na mesma.
- Com um observador na porta, a primeira escrita que o pára é a do ficheiro de estado: a linha `GUARD_STOOD_ASIDE` é de melhor esforço.

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| Uso, e um verbo desconhecido | Lista os 33 verbos | Lista os quatro que tem |
| Um dos 29 verbos que Focus não tem | Passa-o pela fence `state-transition` (afastando-se, com `GUARD_STOOD_ASIDE`) e corre-o ou imprime o seu uso | Recusa-o pelo nome, sem consultar a fence |
| `--project-dir` no fim, sem valor | Fica a fazer parte do nome do campo | Recusa a flag, antes de saber o registo |
| `FOCUS_METRICS_ENDPOINT` definido numa recusa | Escreve o `ERROR_LOGGED` e envia a métrica | Não escreve a linha (a recusa é impressa na mesma) |
| Um registo com o marcador `active-directive.json` | Lê nele a instrução do engine e a sequência de turnos, e guarda ao lado o registo das escritas do estado | Recusa decidir a fence e recusa escrever o estado: o orchestrator que o escreve é M5. Os marcadores de turno (`human-turn`, `engine-touch`) são lidos desde a fatia 3b |
| Sessões por processo; worktree delegada | Resolve | Recusa, como nos outros comandos (as sessões por processo fecharam em M4, fatia 3c-i) |
| `FOCUS_STATE_TRANSITION_OWNER` em Windows | Compara com o pid do pai | Recusa: Focus não lê o pid do pai aí ainda. Em Linux e macOS compara |

**O que a fatia 2 não tem.** O registo das escritas do estado (`state-writes.json`), que o orchestrator lê para dizer o que pôs um passo fora de data; entra com ele (M5). O lock é o do workspace, não um por intent (decisão 3, R5).

### Fatia 3a: o scanner do workspace, e o comando `utility`

A criação de um intent começa por classificar o projeto: Greenfield ou Brownfield, e com que linguagens, frameworks e sistema de build. O original fá-lo com regras sobre os ficheiros que estão lá (`detectWorkspace`, em `aidlc-utility.ts`): conta ficheiros por extensão na raiz e, até seis níveis, em `src`, `app`, `lib`, `pages`, `components` e `tests`; lê alguns manifestos (`package.json`, `Gemfile`, `pom.xml`, `pyproject.toml`); e, quando a raiz não mostra código, desce até três níveis à procura de projetos aninhados, nomeando também cada repositório git que passa. Um `.gitmodules` com uma entrada válida declara código, mesmo por buscar. O que depende da ordem em que o sistema lista um diretório (qual das linguagens empatadas vem primeiro) segue essa ordem, como no original; o passeio aninhado ordena as entradas, como no original.

`focus engine utility` é o `aidlc-utility.ts`: `detect [--json]` imprime o scan e onde vivem os scopes (para o composer não ter de o derivar), `now` imprime a hora do engine. Os outros 32 verbos são recusados pelo nome, cada um com uma frase que diz o que faz; `intent-create` entra na fatia 3b. Os argumentos leem-se como o original os lê (`parseArgs`: tudo é flag, `--json` sozinho vale `true`), e uma recusa deixa o `ERROR_LOGGED` ao lado do registo que `--intent` ou `--space` nomeiam, senão do ativo. A recusa e a linha que deixa passaram a ser de `focus-cli::report`, partilhadas com `state`.

**O que existe.** `focus-workspace::scan` (`detect`, `parse_gitmodules`, os ficheiros que o engine escreveu inteiros, lidos do descritor de projeção de cada harness instalado), `focus-cli::utility`, `focus-cli::report`, `Parsed` em `focus-cli::args`.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `utility/*` | 144 verificações com nome: 131 iguais, 13 desvios, 0 diferentes. Cobrem cada extensão, cada framework e cada sistema de build, o limiar das linguagens secundárias, os diretórios excluídos, a profundidade, os projetos aninhados e a sua ordem, os repositórios git, o `.gitmodules` (entradas inválidas, CRLF, caminhos com `\`), os manifestos que são diretórios ou JSON inválido, os links (não gravados) e os ficheiros escritos inteiros pelo engine |
| Diferencial completo | 1 832 verificações: 1 716 iguais, 116 desvios, 0 diferentes. 1 814 gravadas, sobre 398 árvores: as do `utility` gravam o projeto inteiro, porque o scan lê-o inteiro. Um empate entre linguagens segue a ordem em que o sistema lista os ficheiros, e um nome que só difere na caixa é outro ficheiro em Linux e o mesmo em macOS: essas verificações comparam-se ao vivo e não se gravam, ou evitam o caso |
| Mutações | 47 no scanner, no comando e no leitor de flags; 40 apanhadas, 11 delas só depois de o diferencial e os testes ganharem o caso que faltava (um diretório `.oculto` no passeio, um ficheiro chamado `.py`, `..` com `\\` no `.gitmodules`, a sonda do `.git`, o limiar de um quinto, a ordem UTF-16, um repositório acima de um hit, seis submodules, `--space` sem valor, um valor que começa por `-`, um valor em branco). Sete são equivalentes: o teste de link redundante com `symlink_metadata` (simplificado); `src` na lista do passeio aninhado (um diretório com `src` é Brownfield e nunca é entrado); `package.json` presente antes de o ler; `.` na lista dos ficheiros inteiros (um diretório nunca é igual a um ficheiro); os diretórios a saltar no varrimento de profundidade 0 (que não entra em nenhum); as linhas `;` e `#` do `.gitmodules` (a chave levaria o marcador); e `\\` para `/` no caminho da proposta, que só em Windows se apanha |
| `cargo test` | 310 testes |

**Pormenores do original que ficam iguais.**

- `.env.py` tem extensão: o ponto só não conta no início do nome.
- Um `src` que é um link é um sinal de código e é lido através do link; um ficheiro que é um link nunca é contado.
- `{...dependencies, ...peerDependencies}.react`: a entrada de peer tem a última palavra, e `""`, `0` e `null` não são React. `dependencies` como string não vazia conta como dependência (`Object.keys`).
- O Bun lê o `package.json` do diretório de trabalho ao arrancar e queixa-se no stderr de um que não consegue ler; o diferencial retira essa queixa, que é do runtime e não da ferramenta.

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| Uso, e um verbo desconhecido | Lista os 34 verbos; um verbo em falta é `"undefined"` | Lista os dois que tem |
| Um dos 32 verbos que Focus não tem | Corre-o | Recusa-o pelo nome |
| `detect --project-dir` num diretório sem instalação | Faz o scan do diretório e nomeia os scopes da instalação de onde o próprio script corre | Recusa: Focus não corre de lado nenhum, e o projeto nomeado tem de ter a instalação cujos scopes reporta |

**O que a fatia 3a não tem.** `reclassify` (a pessoa diz que é projeto novo ou código existente), que mexe no estado e na auditoria e entra com as transições.

### Fatia 3b: `intent-create`

`focus engine utility intent-create` é o `handleIntentCreate` do original: a criação de uma peça de trabalho. Pela ordem do original, recusa tudo o que pode recusar antes de escrever o que quer que seja (as flags, o scope, os níveis, o tipo de projeto, a classe de review, a Guard Policy e o seu nome reformado `--change-control`, as cerimónias, o plano composto com `--skip`/`--add`/`--plan-name`, o espaço, a memória `strict` do espaço, o abaixamento de fences que é decisão da pessoa, os repos); depois, sob o lock do workspace, cunha o registo (`<YYMMDD>-<label>`, com contador quando o nome está tomado), escreve na auditoria as linhas das três stages de inicialização e a passagem para a primeira stage do plano, garante os diretórios do registo e do espaço, actualiza a cópia da memória activa (`focus/active-memory/`), faz o scan do workspace (fatia 3a), constrói o ficheiro de estado a partir do plano, escreve `project-description.json`, o marcador `engine-touch`, e por fim lista o registo em `intents.json`, aponta-lhe o cursor `active-intent` e liga-lhe a sessão quando a há (senão deixa o `creation-receipt`).

**O que existe.** `focus-intent` (`names`: slug, data, uuid v7, nomes reservados; `plan`: o plano com alterações, as fases que corre, a primeira stage, a seguinte; `template`: o ficheiro de estado, a descrição numa linha, o documento colado; `record`: o registo, os cursores, a ligação da sessão; `create`: a orquestração), `focus-workspace::active_memory` (a cópia da memória), `focus-workspace::terminal` (quem está ao terminal: a marca do host no ambiente, o terminal próprio da pessoa), e em `focus-guards` a leitura dos marcadores de turno: a vez da pessoa depois do toque do engine é um grant (`turn-marker`), o contrário não cobre nada; o `active-directive.json` continua a recusar-se (M5). Um kill switch lido do ambiente pelo CLI (`FOCUS_DISABLE_*`) estava a ser procurado com o prefixo em dobro; corrigido.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `utility/create/*` | 85 verificações com nome: 77 iguais, 8 desvios, 0 diferentes. Comparam o que o comando diz, o ficheiro de estado byte a byte, as linhas da auditoria campo a campo (menos o `Source Baseline`), o registo `intents.json` (com o uuid e o dia como marcadores), os cursores, a ligação da sessão, o recibo da criação, a cópia da memória activa e os diretórios criados. Cobrem cada recusa de flag e de valor, os onze scopes, os níveis, a classe de review, o plano composto, as cerimónias, a Guard Policy em cada posição da escada (default do scope, acima, abaixo, `FOCUS_UNATTENDED`, o kill switch de presença, um terminal de host pela marca `TERM_PROGRAM` ou por uma variável do host, e a configuração de uma ferramenta que não é marca), a memória `strict` do espaço, o tipo de projeto declarado e lido, os submodules, os repos explícitos e descobertos, os espaços, as sessões, o registo malformado e existente, o nome do diretório tomado, a memória por semear, os observadores |
| Diferencial, `state/marker/*` | 7 verificações: 5 iguais (os marcadores de turno, nas duas ordens, com e sem `FOCUS_UNATTENDED`, e um marcador que é diretório), 2 desvios (o `active-directive.json`) |
| Diferencial completo | 1 921 verificações: 1 798 iguais, 123 desvios, 0 diferentes; 56 das iguais com o `Source Baseline` retirado antes de comparar. 1 898 gravadas, sobre 419 árvores. As marcas de um host no ambiente (`TERM_PROGRAM`, `CLAUDECODE`, …) são retiradas do ambiente de cada verificação e postas por uma verificação que as quer, para que o resultado não dependa de onde o diferencial corre. A fixture da instalação passou a levar a semente da memória (`tools/data/memory-seed`), que a criação restaura |
| Mutações | 53 na criação, no template, no registo, na cópia da memória e nos marcadores de turno; 48 apanhadas, 12 delas só depois de o diferencial e os testes ganharem o caso que faltava (uma stage concluída no caminho da seguinte, um plano composto que salta `reverse-engineering` num projeto novo, um grafo com duas stages de inicialização ou sem operation, um grid que salta uma stage de inicialização, `code-generation` saltada com `units-generation` a correr, um cursor de espaço que nomeia um espaço sem diretório, `--guard-policy strict` num scope `strict`, um scope com plan approval `off` por defeito (`express`), os diretórios `knowledge/` e `codekb/`, um marcador de turno que é diretório). Cinco são equivalentes: a primeira ocorrência contra todas ao marcar a primeira stage (nenhum slug do grafo é prefixo de outro); um recibo que nunca se substitui (o diretório acabou de ser cunhado); a releitura da memória sob o lock (só uma corrida a distingue); o observador em `mark_engine_touch` (o append da auditoria pára-o antes); e o filtro de UTF-8 no que a cópia da memória quer (um ficheiro que não é texto não se copia de qualquer forma) |
| `cargo test` | 325 testes |

**Pormenores do original que ficam iguais.**

- O slug é cortado a 24 caracteres e o nome do diretório leva a data UTC à frente; o mesmo label no mesmo dia leva `-2`, `-3`, …; `help` e os verbos de intent e space são nomes reservados.
- `--guard-policy` com o próprio default do scope não é um abaixamento e fica registado como vindo do scope; um valor abaixo do default é recusado com a frase que diz ao agente para criar o trabalho e correr o setter quando a pessoa pedir, salvo com `FOCUS_SKIP_HUMAN_PRESENCE_GUARD=1` e nenhum chat registado; num terminal de um host (a marca no ambiente) a frase manda pedir no chat desse host.
- A memória `strict` do espaço é lida duas vezes: antes de qualquer mutação e outra vez sob o lock.
- Um projeto novo salta `reverse-engineering` e diz porquê na lista das stages saltadas; um scope de código existente que faz scan Greenfield deixa uma nota no stderr.
- O documento colado (`<document>…</document>`) sai da linha `Project`; um caracter que uma linha não pode ter vira espaço.
- `package.json` e companhia são lidos pelo nome, como no original: são ficheiros da pessoa, não do workspace.

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| `Source Baseline` na linha `WORKFLOW_STARTED`, e o ficheiro `baseline-<hash>.tsv` ao lado do registo | O passeio da atestação por todos os ficheiros fonte do projeto | Não escreve nenhum dos dois: é M5. O diferencial retira o campo e o ficheiro antes de comparar (`SHAPES`), e compara todo o resto da criação. Levantado em M5 (fatia 1d) |
| Um observador (`FOCUS_STOP_HOOK_PROBE`, `FOCUS_ROUTE_CHECK`) que chega à criação | Pára no `writeFileAtomic` da baseline | Pára no append da primeira linha da auditoria; os dois deixam o diretório cunhado, o clone id e o título do shard. Levantado em M5 (fatia 1d): Focus escreve a baseline e pára no mesmo sítio |
| `--request <id>` | Vai buscar a pergunta do engine | Recusa: a loja de perguntas é M5 |
| O uso de `intent-create` | Oferece `--request` | Lista as flags que tem |
| Um projeto com o layout plano `focus-docs/` | A primeira criação migra-o para um registo | Recusa |
| As concessões que o hook de turno humano guarda para um chat (`*-at-creation-<sessão>.json`), e um bypass de presença gravado | Gasta-as | Recusa quando há uma para a sessão (M4) |
| As linhas que um chat ainda deve à pessoa (`<sessão>.person-lines`) | Passam para o trabalho criado | Ficam onde estão (M4) |
| Os blocos de `.gitignore` e `AGENTS.md` numa cópia em que o config nunca correu | Acrescenta-os na primeira criação | Não lhes toca: é do instalador (M6). O diferencial não compara os ficheiros da raiz; fica dito aqui |
| `--project-dir` sem instalação, sessões por processo, worktree delegada | Resolve | Recusa, como nos outros comandos (as sessões por processo fecharam em M4, fatia 3c-i) |

**O que a fatia 3b não tem.** `reclassify`, `status`, os verbos `intent` (list, switch, archive, unarchive) e `space`: entram com as transições e o estado visível (fatias 4 e 5).

### Fatia 4a: as transições

`focus engine state advance`, `finalize`, `complete-workflow`, `skip` e `resume` são os `handleAdvance`, `handleFinalize`, `handleCompleteWorkflow`, `handleSkip` e `handleResume` do original: o que o orquestrador faz ao estado depois de uma decisão. Cada transição corre inteira sob o lock do workspace, lê o estado, decide, escreve primeiro as linhas da auditoria e depois o ficheiro de estado (uma linha que não se consegue escrever pára a transição antes de o estado mudar), e responde com um objeto JSON. Uma transição repetida depois de um crash reconhece o que já fez e escreve-o uma vez: `advance` de uma stage já marcada e já auditada com a seguinte já aberta é um replay; `skip --route` de uma stage já saltada lê a cauda da auditoria desde o seu `STAGE_SKIPPED` e só escreve as linhas que faltam.

Antes de mexer, a transição passa pela cadeia de admissão (`admitStageAction`), listada uma vez em `focus-lifecycle::admission`: o workflow não está arquivado; os artefactos declarados da stage existem (um basta; todos, em todos os stores, para a base de conhecimento do código); uma stage que produz código (`workspace_requires`) deixou trabalho fora da árvore do workspace, que num repositório git se distingue de uma baseline brownfield (uma alteração por confirmar, ou código no último commit) e fora dele é um ficheiro que exista; a confirmação do sumário, o recibo do reviewer e os handoffs de um pipeline, quando devidos. A conclusão de uma stage deixa na linha `STAGE_COMPLETED` o recibo de validade (`Validation Basis`, `aidlc-validity.ts`): o contrato da stage no grafo, o tipo de projeto, e por cada artefacto de entrada e de saída um hash dos ficheiros a que resolveu e um hash do seu conteúdo (CRLF lido como LF); o que não se consegue capturar fica como `Validation Warning` e não pára a conclusão. `STAGE_STARTED` leva o modo de resposta que a pessoa escolheu antes (`Answer Mode`), lido do par pergunta/resposta da auditoria.

**O que existe.** `focus-lifecycle` (`workflow`: o registo, o lock, a auditoria; `stage`: o que uma transição lê de uma stage compilada além dos campos tipados; `events`: o que se lê de volta da auditoria, o emparelhamento decisão/resposta e o modo de resposta; `artifacts`: onde cada stage escreve, o DAG de units, o nome do store da base de conhecimento, o trabalho de código; `validity`: o recibo; `admission`: a cadeia; `transitions`: as cinco; `registry`: a linha do registo ao concluir), `Install` público em `focus-intent`, e em `focus-js` um NUL ignorável na colação (`localeCompare` de uma chave `artefacto\0produtor`).

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `state/{advance,finalize,complete,skip,resume}/*` | 104 verificações com nome: 95 iguais, 9 desvios, 0 diferentes. Comparam o que o comando diz, o ficheiro de estado byte a byte, as linhas da auditoria campo a campo e o registo. Cobrem cada recusa (uso, stage desconhecida, scope em falta, vazio ou inválido, a stage que não é a atual, a seguinte explícita saltada pelo plano, sem seguinte, arquivado, sem ficheiro de estado), a transição inteira, a fronteira de fase, o replay, a stage já marcada com e sem a sua linha, o guard dos artefactos (um diretório no lugar do ficheiro, CRLF, o kill switch), as entradas do recibo (opcionais presentes, só de brownfield, tipo de projeto desconhecido, dois produtores, as de uma stage por unit lidas do DAG, dos diretórios, ao nível da stage, com o DAG em cache e com o DAG ilegível), a base de conhecimento com o repo registado, por registar e incompleta, a porta das transições, os observadores; para `skip`, o salto simples em cada estado, `--route` e cada replay parcial, a aprovação pendente com e sem SKIP no plano, a última stage com o registo a fechar; para `resume`, a compactação pendente e tratada, os campos de unit, um estado esparso |
| Diferencial, fuzz de sequências (`fuzz/transitions`) | 650 sequências de 3 a 8 transições a partir de uma stage ao acaso com artefactos ao acaso (`--fuzz 200` e `--fuzz 2400 --seed 7`): todas iguais |
| Diferencial completo | 2 025 verificações: 1 893 iguais, 132 desvios, 0 diferentes; 5 das iguais com a pergunta de recuperação retirada e 35 com os hashes do recibo verificados antes de comparar. 2 002 gravadas, sobre 482 árvores. O replay ganhou `FOCUS_RECORDED_ONLY=<texto>` para repetir um grupo |
| Mutações | 33 em `admission`, `transitions`, `events`, `artifacts` e `validity`; 32 apanhadas, 7 delas só depois de o diferencial e os testes ganharem o caso que faltava (uma stage marcada com a seguinte aberta e sem linha de conclusão, uma linha de conclusão no instante do `WORKFLOW_STARTED`, um workflow começado de novo depois de um salto, dois stores por reclamar, e três testes unitários sobre a fixture: o trabalho de código, as fases que devem sumário, o modo de pipeline). Uma é equivalente: `required` ausente num `consumes` (o compile escreve-o sempre) |
| `cargo test` | 347 testes |

**Pormenores do original que ficam iguais.**

- Os argumentos posicionais são os que não começam por `--`; o valor de uma flag que o verbo não lê conta como posicional (`advance x --foo y` abre `y`). `complete-workflow` exclui o valor de `--reason`, que pode ser outra flag; `skip` lê `--reason` com `getFlagValue` (um valor em falta ou outra flag recusa-se).
- `advance` com a seguinte explícita só recusa a que o plano salta; a seguinte derivada passa as stages concluídas e saltadas e lê o sufixo do estado antes do grid.
- `finalize` move o cursor sem abrir a seguinte nem escrever linha nenhuma; a seguinte `advance` da mesma stage escreve então o `STAGE_COMPLETED`.
- O contador `Completed` conta as stages de inicialização.
- `resume` não passa pela porta das transições e lê os últimos 400 linhas da auditoria para a compactação pendente.

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| A pergunta de recuperação depois da frase de um guard (`{"kind":"ask",…}`) e o registo da série em `.focus-engine/guard-refusals/` | Escreve os dois: os remédios que o condutor pode oferecer, lidos da tentativa de review (M5) | Imprime a frase. O diferencial retira a linha e o ficheiro antes de comparar (`SHAPES`), e compara a frase, a linha `ERROR_LOGGED` e tudo o resto |
| Os hashes do recibo de validade | Sobre caminhos que levam o nome do diretório do workspace (`aidlc/`) | Sobre `focus/`: diferem por construção. O diferencial recalcula os hashes de cada engine a partir da sua própria árvore e dos seus nomes e compara os que batem como `<verified>` (`SHAPES`); um que não bata compara-se tal como está. A gravação guarda `<verified>` e o replay normaliza os hashes de Focus da mesma forma |
| A confirmação do sumário, o recibo do reviewer e os handoffs de um pipeline, quando devidos | Pesa os registos de `aidlc-log` e recusa com o que falta | Recusa pelo nome (M5). Com a cerimónia `summary_confirmation` desligada (`--summary-confirmation off`, `FOCUS_DISABLE_SUMMARY_CONFIRMATION`, `FOCUS_SKIP_SUMMARY_CONFIRMATION_GUARD`), a classe de review `none` (`--review none`, `review_cap` do scope), uma stage sem reviewer, ou `FOCUS_DISABLE_ENSEMBLE_EVIDENCE` para um pipeline, nenhum dos dois deve nada e a transição corre inteira. Uma stage já marcada que tem linhas `REVIEW_*` na auditoria recusa-se também: a recuperação do original pesa-as |
| Uma stage de Construction | Admite-a sem DAG de units como stage ao nível da stage | Recusa a fase pelo nome: checkpoints, unit gates e swarms são M4 |
| O rollup de uso (tokens, custo) na linha de conclusão | Lê o ledger de uso que os hooks do harness dobram das transcrições | Recusa a conclusão enquanto `focus/.focus-sessions/usage-ledger.json` tiver uso para ela (`FOCUS_DISABLE_USAGE_TRACKING=1` para concluir sem) |
| `skip --unit` | Verifica a política de Construction e recusa por ela | Recusa a flag pelo nome (M4) |
| O uso de `state` | Lista os 33 verbos | Lista os que tem (nove nesta fatia, treze com a 4b) |

**O que a fatia 4a não tem.** Os verbos de gate (`gate-start`, `approve`, `reject`, `revise`), que leem a resposta da pessoa e os sensores: fatia 4b. `park`, `unpark`, `jump`, `validity` e `--status`: fatia 5.

### Fatia 4b: os verbos de gate

`focus engine state gate-start`, `approve`, `reject` e `revise` são os `handleGateStart`, `handleApprove`, `handleReject` e `handleRevise` do original: o gate de aprovação de uma stage, que o orquestrador abre quando os artefactos estão prontos e fecha com a decisão da pessoa. `gate-start` passa a stage a `[?]` e escreve `STAGE_AWAITING_APPROVAL` com o que o gate pergunta: a stage seguinte, o digest do brief de review (`.focus-engine/reviews/<slug>/stage/briefs/latest.json`), o estado dos sensores por artefacto (`SENSOR_PASSED`/`SENSOR_FAILED` desde a abertura, `not-run` para os que não correram), as decisões em aberto lidas dos blocos ```` ```focus-decisions ```` dos artefactos (`unreadable (<ficheiro>)` quando um bloco não se lê). Um gate já aberto responde `revalidated`. `reject` guarda o feedback, sobe `Revision Count` e passa a `[R]`; `revise` volta a abrir; `approve` escreve `GATE_APPROVED` (a resposta, o digest, as decisões aceites em aberto, a stage com que aprova em conjunto) e `STAGE_COMPLETED`, e segue como `advance` ou, na última stage, como `complete-workflow`.

A decisão é da pessoa, e é isso que a cadeia verifica antes de a gravar. Com o guard de presença ligado (`FOCUS_SKIP_HUMAN_PRESENCE_GUARD` ausente), a decisão precisa de um turno humano (`HUMAN_TURN` do hook de turno) desde a última resolução de um gate ou pergunta, e, uma vez mostrado o gate neste clone, desde que foi mostrado; um turno que foi só um comando (`Reply: command`) não conta e é nomeado na recusa. A resposta lê-se então como sintaxe: vazia, é "não foi passada nenhuma escolha" (a pessoa decidiu algo: `person-decided`); boilerplate de cancelamento (`Cancelled`, `timed out`, `no answer`) não é decisão; uma frase que atribui a escolha ao assistente (`conductor's default`, `decision was made by the assistant`, `I, as the agent, am approving`, `approved by the stop hook`, por categoria, com o que está entre aspas, em código ou citado mascarado antes) recusa-se com a categoria e a frase. Essas recusas são "para o agente": a linha JSON leva `agent_guidance` (`question-open` ou `person-decided`), como no original. Com o guard desligado, a resposta é a palavra da pessoa e não se lê. A recusa por falta de turno diz o que fazer: com `FOCUS_UNATTENDED=1`, que as submissões automáticas não contam; num terminal próprio, como correr os comandos; sem nenhum heartbeat dos hooks (`.focus-engine/hooks-health/*.last`) e com um evento de stage ou de gate, que os hooks nunca correram aqui e o passo do próprio harness (`hookActivation.agentStep` do manifesto, com `<entry>`, `<next>` e `<folder>` preenchidos); com o heartbeat do hook de prompt recente, que a pessoa ainda não respondeu; de outro modo, que a resposta pode não ter chegado, e nunca a oferta de desligar uma verificação.

O backstop de revisão (`approve` de um gate cujo artefacto mudou depois de aberto, com um turno humano antes e depois) grava `GATE_REJECTED` com `Recovered` e `STAGE_REVISING` e volta a abrir antes de aprovar; `FOCUS_SKIP_REVISION_BACKSTOP=1` ou uma política que aceita alterações (`relaxed`, `off`) não o fazem. `--approves-together` recusa-se com as stages que o gate cobre (nenhuma: o plano de aprovação conjunta é do orquestrador). As perguntas protegidas (`focus/.focus-sessions/plan-approval/protected-question-*.json`) são retiradas quando um gate abre.

**O que existe.** Em `focus-lifecycle`: `gates` (os quatro verbos, a recusa para o agente), `gate` (os campos do pedido, os sensores que disparariam, o digest, as decisões, a aprovação conjunta, os findings de review, as perguntas protegidas), `presence` (`humanTurnState` e família: os turnos contra as resoluções, shard a shard, com um empate no mesmo segundo entre shards a falhar fechado; a apresentação neste clone; os heartbeats dos hooks e a ativação do harness), `reply` (a boilerplate, a escolha exata, a aprovação de um gate, o marcador de auto-atribuição com as categorias do original). Em `focus-js`, `glob`: o `Bun.Glob` dos padrões dos sensores (`**`, `*`, `?`, `{a,b}`, `[...]`) como regex ECMAScript. Em `focus-cli`, a recusa com `agent_guidance`, `getTextFlagValue` (um valor que pode começar por `--`, uma flag dada duas vezes) e `getFlagValues`.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `state/gates/*` | 144 verificações com nome: 131 iguais, 13 desvios, 0 diferentes. Cobrem o uso, as stages e os estados que cada verbo admite, arquivado, sem scope, o gate inteiro (aberto, mostrado outra vez, aprovado; rejeitado, revisto, rejeitado de novo, aprovado), a fronteira de fase e a última stage, os artefactos (nenhum, o guard desligado, `--artifacts` com e sem valor e vazio, `--recovered`), a aprovação conjunta (a flag, a linha `Approves Together`, a cobertura por outro gate), os sensores (a cerimónia ligada, desligada por switch, sem artefactos, `--override-blocking-sensors`, as linhas `SENSOR_*` com e sem fim), o digest (válido, ilegível, não texto), as decisões (válidas, ilegíveis, sem cabeçalho, vazias, duplicadas, dois blocos um ilegível), a evidência devida, a secção `## Review` (com findings, dentro de um fence, vazia), `--reject-finding` sem reviewer, as formas do feedback, a contagem de revisões NaN e em falta, a presença (sem turno, turno depois e antes do gate, consumido, só comando, só pergunta, noutro shard, empate entre shards, usado por uma resposta ou por um sumário, dois no mesmo segundo, sem sessão, shard ilegível, unattended, terminal próprio, heartbeat recente e antigo, o passo dos hooks desligados, `--recovered --person-approves`), 30 respostas e 12 feedbacks que percorrem cada categoria de auto-atribuição e de boilerplate, o backstop (ligado, desligado, política relaxada), `--unit` sem e com `team`, o ledger de uso, o marcador de diretiva, a porta das transições, sem ficheiro de estado, os observadores |
| Diferencial, fuzz de sequências (`fuzz/transitions`) | Agora com os quatro verbos de gate entre as transições, sobre um estado com os sensores desligados e o guard de presença desligado: 400 sequências (`--fuzz 400` e `--fuzz 1200 --seed 7`), todas iguais. Apanhou `--artifacts ""`, que o original lê como lista nenhuma |
| Diferencial completo | 2 171 verificações: 2 028 iguais, 143 desvios, 0 diferentes; 7 das iguais com a pergunta de recuperação retirada e 79 com os hashes do recibo verificados. 2 147 gravadas, sobre 545 árvores (`presence_hooks_heartbeat` fica fora da gravação: o heartbeat é recente contra o relógio, e o replay corre mais tarde) |
| Mutações | 34 em `gates`, `gate`, `presence` e `reply`; 33 apanhadas, 8 delas só depois de o diferencial e os testes ganharem o caso que faltava (um empate no mesmo segundo entre um turno e uma decisão noutro shard, um diretório de shards e o dos heartbeats atrás de um link, os exemplos citados mascarados antes de ler a auto-atribuição, a aprovação sem resposta com o guard desligado, o gate mostrado noutro clone ou recuperado, um `Revision Count` que não cabe num número, linhas `REVIEW_*` numa stage sem reviewer). A restante era um ramo sem chamador portado (`missedReply: false`, do log tool, M5) e foi retirado |
| `cargo test` | 361 testes |

**Pormenores do original que ficam iguais.**

- As recusas de leitura da resposta só correm com o guard de presença ligado: com `FOCUS_SKIP_HUMAN_PRESENCE_GUARD=1`, `approve --user-input "Cancelled"` aprova.
- `reject` lê `--feedback` (ou `--reason`) com `getTextFlagValue`: o valor pode começar por `--` (`--feedback=--x`), uma flag dada duas vezes recusa-se, e `--feedback` e `--reason` juntos são duas flags de uma só.
- `Revision Count` que não é número conta de zero; em falta, a linha não é acrescentada e `get` não a encontra.
- A presença lê este clone para a apresentação do gate: um turno humano só noutro shard não responde ao gate mostrado aqui.
- A linha `STAGE_AWAITING_APPROVAL` leva `Artifacts` só quando a lista não está vazia.

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| Os sensores de gate com a cerimónia ligada e os artefactos declarados em disco | Dispara-os pela ferramenta de sensores, grava os findings e abre o gate | Recusa a abertura pelo nome (M5) e diz como abrir sem eles: `--sensors off` na criação ou `FOCUS_DISABLE_SENSORS=1`. Sem artefactos, ou com a cerimónia desligada, abre como o original |
| A confirmação do sumário e o recibo do reviewer, quando devidos | Pesa-os e recusa com o que falta; com `SKIP_REVIEWER_GATE_GUARD` abre o gate e pesa o recibo outra vez na aprovação | Recusa pelo nome (M5); o kill switch abre o gate nos dois |
| Os findings de uma review (linhas `REVIEW_*` na auditoria, a secção `## Review` do artefacto de review) | Lê-os do brief de review e grava cada um em aberto na decisão como risco aceite | Recusa a decisão pelo nome enquanto lá estiverem (M5); uma secção vazia ou dentro de um fence não é secção, como lá |
| `--reject-finding`, `--reopen-finding` | Procura o finding no brief e recusa um que não seja atual | Recusa as flags pelo nome; sem reviewer, a recusa do original |
| As palavras do gate (`.focus-engine/gate-words/<sessão>.json`) | Lê-as para a linha da decisão | Recusava a decisão enquanto o ficheiro existisse; lidas como no original desde M4 (fatia 4d-ii) |
| O gate de uma unit (`--unit` com `Unit Ownership: team` numa stage de Construction por unit) | Procura a unit no DAG | Recusa pelo nome (M4); as recusas de `--unit` sem `team` e numa stage que não é por unit são as do original |
| O rollup de uso e o marcador de diretiva ativa | Como na fatia 4a | Recusa pelo nome |

Fora de M3 e recusados pelo nome, como `fragment-fork` ficou em M2: os subcomandos de units, bolts e swarm (`unit`, `fork`, `merge`, `fold-unit-merge`, `refresh-unit-progress`, `sync-unit-scope-stage`, `set-unit-ownership`, `set-unit-gate-rhythm`), as políticas de Construction (`set-construction-*`, `set-skeleton-stance`), as práticas (`practices-event`, `practices-promote`), `reuse-artifact`, `lookup` e `acknowledge-compaction`.

### Fatia 5a: parar e saltar

`focus engine state park` e `unpark` são `handlePark`, `parkWorkflow` e `handleUnpark`: `park` escreve `WORKFLOW_PARKED` e as linhas `Parked` (o instante), `Parked At Stage` e, quando a paragem é da pessoa, `Parked By: person`, debaixo de `## Runtime State`; `unpark` apaga-as e escreve `WORKFLOW_UNPARKED` uma vez (repetido, não escreve nada). De quem é a paragem decide-se como no original (`personSpokeSinceGate` com `outlivesApproval`): um turno humano na auditoria desde a última resolução, ou, se já foi gasto, um turno que ainda está de pé porque o que veio depois foi a aprovação dada nessa mensagem e as do próprio run (`User Input` em falta, `Autonomous`, `Approved Together With`, a aprovação do plano); uma resposta a uma pergunta, uma segunda aprovação, a autonomia concedida ou um empate entre shards gastam-no. Com `FOCUS_UNATTENDED=1` não há pessoa. Com `Construction Autonomy Mode: autonomous`, só a paragem da pessoa pára o run, e um run sem ninguém nunca se pára a si próprio.

`focus engine jump resolve` e `execute` são `handleResolve` e `handleExecute` de `aidlc-jump.ts`. Cada leitura do plano passa pela ação efetiva (`effectiveAction`): o sufixo `EXECUTE`/`SKIP` da própria linha do estado ganha à grelha do scope, quando o scope perguntado é o do estado; `--scope <outro>` lê só a grelha. `resolve` aceita a stage por slug ou número, ou a fase por nome ou número (a primeira stage com `EXECUTE` no plano efetivo, senão a primeira na coluna do scope), recusa uma stage que o plano salta salvo com `--allow-skipped` (`target_skipped`), e diz a direção e as stages afetadas. `execute` escreve o estado como `movedState`: para a frente, as stages em curso entre a atual e o alvo passam a `[S]` (e a atual, se não estava por começar); para trás, o alvo e tudo o que o segue no plano efetivo volta a `[ ]`; `redo` repõe só o alvo; o alvo fica `[-]` e os campos do cursor seguem-no (`Lifecycle Phase`, `Current Stage`, `Next Stage` pelo plano efetivo, `Active Agent`, `Next Action`, `Completed`, `Last Completed Stage`, os campos da unit ativa apagados); ao cruzar uma fase, as linhas de progresso de fase e o trio `PHASE_COMPLETED`/`PHASE_VERIFIED`/`PHASE_STARTED`, como em `advance`. O `STAGE_JUMPED` leva a direção, a origem, o alvo e, para trás, os artefactos declarados do alvo (`Changed Upstream Artifacts`, existam ou não: `reviewArtifactEntries`, com os caminhos lógicos `codekb/*/…` e `construction/*/…` quando não há repositório nem unit), os que existem nas stages repostas e as secções `## Review` que lá estão (`<artefacto>#Review`); segue-o um `STAGE_STARTED` com o modo de resposta. A resposta diz o aviso que o agente repete (`Moved to X; skipped A, B and C. You can go back to Y any time.` / `Moved back to X. You can return to Y any time.`), com o nome da stage ou, para uma stage de plugin, o slug.

**O que existe.** Em `focus-lifecycle`: `park`, `jump`, `presence::person_spoke_since_gate` e `request_outlives_its_approval`, `artifacts::review_artifact_entries`; `Workflow::emit_plain` para os verbos que o original não embrulha em "Audit emission failed". Em `focus-cli`, `focus engine jump` (ferramenta `focus-jump` nas linhas `ERROR_LOGGED`) e os verbos `park`/`unpark` do comando `state` (`park` passa pela fence das transições, como lá). O replay gravado aceita `FOCUS_RECORDED_ONLY=a,b`.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `state/{park,unpark}/*` | 34 verificações: 33 iguais, 1 desvio, 0 diferentes. A paragem simples e repetida, concluído, arquivado, sem `Current Stage`, sem `## Runtime State`, a fence; de quem é a paragem (um turno e nada depois, só um comando, só uma pergunta, uma aprovação que o turno sobrevive, duas que o gastam, aprovações autónomas e em conjunto, a aprovação do plano, uma resposta, a autonomia concedida, o plano e uma aprovação, um empate entre shards, um turno noutro shard, nenhum turno, unattended); a autonomia (sem resposta, com resposta, unattended); `unpark` com e sem campos, com o campo vazio, com argumentos a mais; os observadores |
| Diferencial, `jump/*` | 67 verificações: 59 iguais, 8 desvios, 0 diferentes. `resolve`: o uso, slugs, fases e scopes desconhecidos, para a frente por slug e por número, por fase (nome, número, maiúsculas, inicialização, a atual, noutro scope), para trás, `redo`, o alvo saltado com e sem `--allow-skipped`, outro scope, os sufixos do estado (a stage promovida e a despromovida, com `--scope` igual e diferente), sem `Current Stage`, com uma atual desconhecida, sem scope, as formas das flags, sem ficheiro de estado. `execute`: o uso (sem alvo, direção inválida, stage desconhecida, saltada, scope desconhecido), para a frente de cada estado da atual, por cima de stages feitas, uma e duas fronteiras de fase, os sufixos, outro scope, para a própria stage, a direção mal dita nos dois sentidos; para trás com e sem artefactos, com uma secção `## Review` (e uma dentro de um fence), por cima de uma fronteira, por cima de Construction (diretórios de units, o DAG, nada, ao nível da stage), por cima da base de conhecimento (sem e com repositórios registados), por cima de uma stage com reviewer, com `Unit Stage` conhecido e desconhecido; `redo` na atual e noutra; o modo de resposta; arquivado; sem ficheiro de estado; `reopen`; os observadores |
| Diferencial, fuzz de saltos (`fuzz/jump`) | 500 sequências de 1 a 3 saltos sobre uma stage, sufixos, caixas e artefactos ao acaso (`--fuzz 600` e `--fuzz 2400 --seed 11`): todas iguais |
| Diferencial completo | 2 272 verificações: 2 121 iguais, 151 desvios, 0 diferentes. 2 248 gravadas, sobre 601 árvores |
| Mutações | 33 em `park`, `jump`, `presence` e `artifacts`; 31 apanhadas, 11 delas só depois de o diferencial ganhar o caso que faltava (uma segunda paragem sem pessoa por cima de `Parked By`, um turno que foi só uma pergunta ou só um comando gasto por uma aprovação, uma fase com todas as stages despromovidas no estado, uma stage saltada pelo plano atrás de um salto, uma fase sem stages `EXECUTE` atrás de um salto, repositórios repetidos no registo, e as stages de Construction feitas atrás de um salto: diretórios de units sem Units Generation no plano, um ficheiro ao nível da stage ao lado dos diretórios, o kind de cada unit a escolher os seus artefactos, um artefacto opcional). Duas são equivalentes: a fase do alvo marcada `Skipped` antes de `Active`, e `Last Completed Stage` a olhar para o alvo que acabou de ficar em curso |
| `cargo test` | 367 testes |

**Pormenores do original que ficam iguais.**

- `jump` não passa pela fence das transições nem verifica um workflow arquivado; `park` passa pela fence e `unpark` não; os dois recusam um workflow arquivado antes de começar (`archivedProtectedCommands`).
- As flags de `jump` leem-se aos pares (`--x valor`): uma flag no fim não é nada, e o valor é o que se segue, outra flag incluída; `--allow-skipped` é retirado antes.
- Sem `Current Stage` o salto parte de `state-init`; `Scope` em falta ou vazio lê `feature`.
- Uma paragem sem `## Runtime State` no estado falha como lá (`appendUnderHeading: heading not found`).
- `execute --target <atual> --direction forward` e um `backward` para a frente fazem o que `movedState` faz com a direção dita, não com a real.

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| `park`/`unpark` com `Unit Ownership: team` | Lê o carimbo de scope de unit do checkout e verifica-o contra o registo de claims e o fan-out | Recusa pelo nome (M4) |
| O baseline de fonte nas linhas `STAGE_JUMPED` e `STAGE_STARTED` de um salto | Escreve o snapshot da atestação e o campo `Source Baseline` | Não escreve nenhum (M5; `SHAPES`, como na criação); um observador pára na primeira linha da auditoria, não no snapshot. Levantado em M5 (fatia 1d): o campo e o snapshot são escritos e comparados |
| `execute --units`, `reopen` | Movem units uma a uma | Recusam-se pelo nome (M4), `reopen` depois de satisfeito o uso |
| Um salto para trás com uma sessão ligada | Guarda o aviso do caminho de volta nas linhas pendentes do chat (hook de turno) e omite `notice` | Recusa pelo nome (M4) |
| O marcador de diretiva ativa | Lê dele o passo em que a pessoa estava | Recusa o salto pelo nome (M5) |
| As reviews que um salto para trás invalida | Lê os registos de review do brief, mais as secções `## Review` embutidas | Só as secções; com linhas `REVIEW_*` na auditoria e uma stage com reviewer atrás do salto, recusa pelo nome (M5) |
| `--project-dir` sem valor | Fica como argumento do verbo | Recusa a flag |

### Fatia 5b: validar e mostrar

`focus engine utility status` é `handleStatus` de `aidlc-utility.ts`: o workflow como a pessoa o lê. Sem ficheiro de estado, o texto "No active Focus workflow found" com os três passos. Com ele: o projeto; o scope, ou o plano composto (`Plan: x (this piece of work only)`, e uma definição que veio do scope diz-se "the approved plan"); o tipo de projeto (`existing code`/`new project`, "(you said so)", e quando o código foi lido pela última vez, da conclusão de Reverse Engineering); a profundidade e a sua fonte (o scope, o plano, ou "set for this piece of work") e a estratégia de testes quando difere; a fase e a stage (nome e número); o passo da unit ativa; a linha de estado (`Awaiting your approval on X (waiting since <data> <hora> UTC, about N minutes/hours/days)` lida de `pendingOrganicGate` — a abertura do gate contra o que a fechou, shard a shard, com um empate no mesmo segundo a falhar fechado —, `Revising X (revision N of 3)`, `X approved - ready to advance`); o agente ativo pelo nome da persona, ou "a custom agent"; a Guard Policy resolvida e os checks desligados por quem (`Checks off: review-freeze (set by you); human-presence (env FOCUS_…)`, os que a política baixa ficam com a sua linha); as cinco cerimónias com a fonte (`resolveCeremony`: o kill switch, a linha do estado, o scope, o default) e a aprovação do plano com as suas regras (`resolvePlanApprovalSetting`: o switch da máquina só conta quando o harness arrancou com ele — gravado para a sessão, ou sem sessão alguma gravada —, uma memória `strict` mantém-na ligada, uma fonte que não é das conhecidas lê-se "this piece of work's settings"); a contagem contra o plano efetivo e as barras de fase (`█▒S░`, e numa Construction unit a unit, `Unit N of M` lido do DAG e dos gates `construction-unit`); a stage atrasada e o que a acompanha; a última concluída e a próxima; o outro trabalho aberto no space.

A validade é a segunda metade de `aidlc-validity.ts`: ler os recibos de volta (`parseStageValidationBasis`: só o schema 3, bem formado; `completionReceiptsFromAudit`: a conclusão atual de cada stage, e a última conhecida, um `STAGE_STARTED` a retirar a atual), compará-los com o que a árvore dá agora (`diffStageValidationBasis`: contrato do grafo, tipo de projeto, cada entrada por chave; `contentOnlyChanges`: a edição de um documento que continua lá, que uma política relaxada aceita; um recibo tirado sobre bytes CRLF de uma versão anterior lê-se na forma atual), propagar (`propagateStageInvalidation`: as raízes diretas, e as stages concluídas que dependem delas pelas arestas que os recibos observaram, com as razões `depends on stale stage "x" via artifact:y`), e dizer (`staleStageNote`: "X ran before the code was here…" quando o tipo de projeto mudou para brownfield, "a.md and b.md changed after X finished…" quando só os documentos da stage foram editados, "Something X used changed…" nos outros casos; "Also affected: …").

**O que existe.** Em `focus-lifecycle`: `validity` (a leitura, a comparação, a propagação, a inspeção, as frases; a memória dos hashes tirados sobre bytes crus), `ceremonies` (as cinco cerimónias com fonte, a aprovação do plano), `status` (o texto, o gate pendente, os nomes das personas, a caminhada por units, o trabalho em curso no space), `stage::stage_label`. Em `focus-cli`, `focus engine utility status [--intent] [--space]`. O diferencial e o replay aceitam um passo que nomeia a sua própria ferramenta (`@state advance …`, depois `status`). Dois diretórios estavam mal nomeados desde a fatia 3b/4b e foram corrigidos: os registos da aprovação do plano (as perguntas protegidas, o bypass, a concessão na criação) vivem em `focus/.focus-sessions/plan-approval/`, não em `.focus-plan-approval/` (o nome do caso de um worktree delegado); o desvio da criação com uma concessão em disco passou a ser real (o original gasta-a, Focus recusa).

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `utility/status/*` | 84 verificações: 81 iguais, 3 desvios, 0 diferentes. Sem workflow, sem registo, fresco, por flags, depois de um `advance`, um estado esparso, tudo desconhecido; o gate aberto (sem linha, fechado, empate entre shards, recuperado, reaberto, depois de um salto, linhas de outra stage e de uma stage isolada, instante ilegível), a revisão com e sem contagem, aprovada e pronta, concluída, parqueada; o plano, o tipo de projeto (dito pela pessoa, lido do código com três conclusões, desconhecido), a profundidade (própria, do plano, ausente); a política strict, em memória, memória ilegível, linha inválida, do plano; os checks desligados por você, pelo ambiente, mistos, pela política (não listados); as cerimónias (linhas, kill switches, linhas malformadas, sem linhas, scope desconhecido); a aprovação do plano (desligada por você, presa pela memória, memória ilegível, fonte estranha, o switch sem confiança, com sessão gravada, com o bypass gravado e malformado); as barras (cada estado, sufixos, scope desconhecido, scope sem stages), a caminhada por units (uma feita, nenhuma, todas, rejeitada, do runtime graph, stage-major, sem DAG), o passo atual (e na mesma stage, unit inválida, stage que não é por unit); o outro trabalho aberto (vários, um, registo malformado); a validade (atrasada, propagada, só dependente, schema antigo, malformada, tentativa recomeçada, tipo de projeto com e sem efeito, recibo acabado de escrever, indisponível); os observadores |
| Unidades (`validity`) | A leitura de um recibo (schema, tipo, contagens, ordem), as mudanças e a edição isolada, a propagação por uma cadeia com uma stage por concluir no meio, um documento editado depois da stage (a frase, a política relaxada, o documento apagado), um recibo tirado sobre CRLF, as frases |
| Diferencial completo | 2 357 verificações: 2 204 iguais, 153 desvios, 0 diferentes. 2 326 gravadas, sobre 662 árvores. Os sete `status/awaiting*` ficam fora da gravação: o tempo de espera conta-se contra o relógio |
| Mutações | 27 em `validity`, `status` e `ceremonies`; 27 apanhadas, 7 delas só depois de ganhar o caso que faltava: quatro por testes unitários (uma tentativa recomeçada a retirar o recibo atual, a edição e a mudança de forma com o mesmo nome, uma stage atrasada por si que uma raiz também alcança) e um caso no diferencial (uma conclusão de schema antigo a suceder a um recibo), e três do gate pendente (o fecho, `Recovered`, o salto) só pelo diferencial ao vivo contra o original, porque os `status/awaiting*` não se gravam |
| `cargo test` | 377 testes |

**Pormenores do original que ficam iguais.**

- Um campo vazio lê-se como `Unknown` (`getField(...) || "Unknown"`); um scope que a instalação não tem conta todas as caixas.
- O tempo de espera diz-se em minutos até 59, horas até 47, dias depois; um instante que não se lê não se diz.
- `status` nunca recusa um workflow arquivado nem passa por fence alguma; o seu `ERROR_LOGGED` é o de `focus-utility`.

**Desvios deliberados.**

| | Original | Focus |
| --- | --- | --- |
| `Unit Ownership: team` | Acaba com o board de equipa, gerado pelo orquestrador a partir das claims das units (e falha no DAG que não lê) | Recusa pelo nome (M4) |
| O marcador de diretiva ativa | Lê dele o passo que o engine entregou por último (só sem `--intent`/`--space`); os guards leem dele a autoridade | Recusa o status pelo nome, com ou sem flags (M5) |

### Limites de M3

O que M3 não faz, e o que acontece quando lá se chega. Tudo recusa pelo nome: nenhum comando ignora o que não porta.

1. **Units e bolts (M5, fatia 5; o que é de equipa está adiado).** Dezoito verbos de `state` (`set-skeleton-stance`, `set-construction-*`, `set-unit-ownership`, `set-unit-gate-rhythm`, `refresh-unit-progress`, `sync-unit-scope-stage`, `fold-unit-merge`, `acknowledge-compaction`, `reuse-artifact`, `lookup`, `practices-event`, `practices-promote`, `fork`, `merge`, `unit`), `jump reopen` e `jump execute --units`, os gates de unit, `park` e `unpark` com `Unit Ownership: team`, o board de equipa do `status`, e uma stage de Construction em `advance`.
2. **Evidências e marcadores (M5).** Os recibos que `aidlc-log` escreve (a confirmação do sumário, os recibos do reviewer, os handoffs de um pipeline) e que uma transição deve; os findings de uma review no gate; o marcador de diretiva ativa, que `jump`, `status` e os guards leem.
3. **Sensores (M7).** Os sensores de gate: `gate-start` com a cerimónia `sensors` ligada recusa; o diferencial e o fuzz correm com ela desligada.
4. **O resto de `utility` (M5 e M8).** Vinte e seis verbos: intents e spaces (`intent`, `space`, `space-create`, `set-status`), o code knowledge base (`codekb-*`, `document-input`, `reclassify`), scopes e settings (`recompose`, `scope-change`, `scope-save`, `config-*`, `detect-scope`, `resolve-env-scope`, `scope-table`, `stage-table`), plugins (`select-plugins`, `plugin-*`) e `help`, `version`, `doctor`, `init`, `project-description`.
5. **Sessões por processo e worktrees delegadas.** Como em M2: um comando que as devia resolver recusa (o limite 3 de M1 passa para M4, com os bolts, cujas worktrees são as delegadas). As sessões por processo fecharam em M4 (fatia 3c-i).
6. **O relógio.** Os sete `status/awaiting*` e o heartbeat da presença contam-se contra o relógio real e não se gravam: verificam-se só pelo diferencial ao vivo.
7. **Só em Unix.** O replay gravado continua a não correr em Windows (R8).

## 15. Estado de M4

M4 está feito. Guards e hooks observadores são, no original, o que corre em cada evento do harness: antes de uma ferramenta (`PreToolUse`: a integridade do runtime, e os quatro guards que decidem por uma fence), depois dela (`PostToolUse`: os que só registam e recompilam), e no arranque e no fim de uma sessão. Todos leem o pedido do harness em JSON pelo stdin e respondem pelo código de saída, pelo stdout e por linhas de auditoria; é esse o critério do marco (secção 6). Entra de baixo para cima: primeiro o que os guards leem (o lexer de shell), depois a integridade do runtime, depois o dispatcher e os observadores, e por fim os guards, que são os que mais dependem do resto.

| Fatia | O que traz | Como se verifica | Estado |
| --- | --- | --- | --- |
| 1. O lexer de shell | `focus-guards::shell`: o que um comando `Bash` pode escrever (POSIX e PowerShell), os programas que corre através dos wrappers, onde a shell pode estar quando escreve; `focus-js::path`: `resolve`, `normalize`, `isAbsolute`, `join` e `basename` do `node:path`, nos dois sabores | Corpus | Feita |
| 2. Integridade do runtime | `focus-guards::integrity`: o que nenhuma ferramenta pode tocar (os registos do runtime, o audit trail, os ficheiros instalados dos hooks), antes de qualquer fence; `focus-js::path` ganha `relative` e `dirname` | Corpus de pedidos de hook | Feita |
| 3a. O dispatcher e os observadores | `focus-hooks`: o comando `focus engine hook <nome>`, o pedido lido do stdin, o traço de cada hook, o contexto de um hook (em que workflow dispara, se esta conversa o integrou), a pulsação e as quedas de cada um; os hooks que só registam: `validate-state`, `session-end`, `log-subagent`, `sync-workflow-state`, `write-audit-log` | Diferencial | Feita |
| 3b. A recompilação e a ligação observada | `rebuild-stage-graph` (o runtime graph recompilado depois de cada ferramenta de transição; uma criação vista na saída da chamada liga a sessão que a fez); `focus-guards::engine_command` (as invocações do engine lidas de um comando de shell: a grafia canónica, o portão da recompilação, a invocação literal e o verbo do orquestrador) | Corpus (`engine_command`), diferencial | Feita |
| 3c-i. Sessões por processo | `focus-workspace::ancestry`: o mapa de processos de uma conversa (`.focus-sessions/pids/<pid>`), escrito pelo arranque de sessão e lido por cada comando a que ninguém nomeou a sessão; a recusa de um override exportado que discorda da conversa dona | Diferencial (o processo do próprio diferencial no mapa) | Feita |
| 3c-ii. O arranque de sessão | `session-start`: a ligação da conversa ao registo e o seu carimbo, o mapa de processos, a oferta de religar, o contexto entregue ao harness; com o que lê e escreve (a cópia da memória ativa e a sua entrega ao chat; `rules-held`; os switches gravados; o ponteiro do transcript; os bypasses com que o harness arrancou) | Diferencial | Feita |
| 4a. `review-freeze` | O congelamento de um documento revisto: quais escritas o alcançam (os alvos de uma ferramenta ou de uma shell, os documentos declarados de um stage com revisor ainda aberto, o ficheiro de perguntas de um summary posto de lado), o interruptor, a pulsação; `reviewed_artifact_unit`; a autoridade de um agente despachado nas fences (`Guards::decide_for`) | Diferencial | Feita |
| 4b. `reviewer-scope` | O matcher puro do âmbito de um revisor despachado (`focus-guards::reviewer_scope`) e o hook à sua volta: o registo de despacho, a identidade, a fence, a linha `REVIEWER_SCOPE_BLOCKED`; a decisão de recusa entregue ao host (`permissionDecision: deny`) | Corpus (`reviewer_scope`, `reviewer_dispatch`), diferencial | Feita |
| 4c. `state-transition-guard` | A fence das transições de estado: os três leitores puros de um comando de shell (`focus-guards::state_transition`: a transição direta, a fronteira de ciclo de vida, o comando que um agente delegado não pode correr) e o hook à sua volta (a fronteira de confiança do harness primeiro, a fence de lado com a sua linha e linha de ledger, a sessão principal fora do workflow, a frase do switch); `fence_switch_sentence` partilhada com `state`; `re::find_all` | Corpus (`state_transition`), diferencial | Feita |
| 4d-i. O turno humano | `record-human-turn`: a linha `HUMAN_TURN` e o que a acompanha (o registo da mensagem, as palavras guardadas para um gate, o marcador, as linhas de uma caixa de perguntas vazia ou com as suas escolhas, o turno do próprio host); os leitores do pedido (`focus-hooks::turn`), o parser do switch escrito (`focus-guards::typed_switch`, com o splitter de Kiro) | Corpus (`typed_switch`, `kiro_args`, `host_envelope`, `host_template`, `supplied_origin`, `picker_empty`, `transcript_origin`), diferencial | Feita |
| 4d-ii. As palavras do gate lidas | O que consome o que 4d-i grava: a decisão de um gate lê as palavras da pessoa (`gateWordsSincePresentation` e os três leitores sobre ele: a escolha exata, as palavras para o recibo, o feedback), as recusas por nome de M3 levantadas; a entrada nua `focus …` decidida | Diferencial, testes unitários | Feita |
| 4e-i. Os leitores do Plan Approval | `focus-guards::guard_operation` (as operações de recuperação de um guard: o comando de cada uma, a sua correspondência exata, a continuação de um restart, a palavra para a pessoa), `focus-guards::plan_approval` (a decisão pura sobre um handoff ao developer e as recusas que o guard imprime) e `focus-guards::posture` (o que o guard lê do contrato de testes: as linhas de contrato de um handoff, a resposta do ficheiro de perguntas, o apêndice `## Review` terminal de um plano) | Corpus (`guard_operation`, `plan_dispatch`, `plan_reason`, `contract_markers`, `questions_file`, `review_appendix`) | Feita |
| 4e-ii-a. O que corre enquanto um plano espera | `focus-guards::dispatcher` (o namespace `engine` do dispatcher do original: a ferramenta e os argumentos a que um comando é delegado) e `focus-guards::plan_wait` (o que qualquer helper pode correr, os pré-requisitos de um Plan Approval, o que o engine nomeia enquanto o plano espera, os diagnósticos só de leitura, a leitura de uma linha PowerShell simples e a avaliação dinâmica de uma linha de shell) | Corpus (`engine_route`, `every_helper`, `plan_prerequisite`, `engine_directed`, `read_only_diagnostic`, `plain_powershell`, `dynamic_evaluation`), os privados do hook perguntados por um shim de exportação | Feita |
| 4e-ii-b. A intenção de uma chamada | `focus-guards::mutation_intent`: o `mutationIntent` do hook (os alvos de escrita de uma chamada, a shell opaca, o que corre o engine, o que o lexer não lê), o que a espera de um plano admite com o que depende do projeto (o switch gravado, o switch de chat, a pessoa ter falado, um script instalado corrido por Bun), e os predicados de alvo (fora do projeto, o output de uma stage, o texto de uma resposta, o diário da stage, a proposta do composer); em `focus-workspace`, a cadeia de links lida como o original a lê, a identidade de uma pasta e o ficheiro simples | Corpus (`mutation_intent`, `mutation_target`) sobre árvores de scratch, pelo mesmo shim; a pessoa no registo lida pelo original e dita pelo caso | Feita |
| 4e-ii-c. `plan-approval-guard`, o hook | `focus-hooks::hooks::plan_approval_guard`: a fronteira de confiança do harness, a conversa que não entrou no trabalho, o heartbeat, o interruptor com `GUARD_DISABLED` uma vez, o que o plano nunca governa (leituras, comandos lidos por inteiro, escritas fora do projeto, o diário, o texto de uma resposta e a proposta do composer), e a recusa sem o passo do engine, com a linha `PLAN_APPROVAL_BLOCKED` ou, de fence baixada, nas outras palavras. As três leituras da pessoa calculadas do ledger. Um marcador de diretiva em disco recusa pelo nome: o marcador, a avaliação da aprovação e `plan-approval-ask` são M5 | Diferencial (`hook/plan-approval-guard/*`), testes da linha de comandos | Feita |
| 4f. `guard-tool-call` | `focus-hooks::group`: o registo único de `PreToolUse` que corre os quatro guards num processo, cada um para as ferramentas que a sua própria linha escolhia; a recusa de qualquer um recusa a chamada, cada recusa dita uma vez, e um membro que falha sozinho não recusa nada. A decisão `deny` passa a ser dada só antes de uma ferramenta, em Claude Code | Diferencial (`hook/guard-tool-call/*`), testes unitários e da linha de comandos | Feita |

### Fatia 1: o lexer de shell

`review-freeze-command.ts` é uma folha: 2 083 linhas que só importam built-ins do Node, e que dois guards, a integridade do runtime e o guard de aprovação do plano chamam para saber o que um comando `Bash` pode escrever. Lê, não corre: não modela a ordem dos segmentos nem avalia o que a shell calcula. Uma palavra que não consegue colocar fica como candidata; um alvo relativo resolve-se de cada diretório em que um `cd` em qualquer ponto do comando pode deixar a shell (`cd .kiro && echo x > hooks/y` nomeia `hooks/y` e `.kiro/hooks/y`); um wrapper que não lê até ao fim (`sudo --weird`) nomeia o próprio diretório de trabalho. Os guards tratam uma candidata como escrita.

**O que existe.** Em `focus-guards::shell`: `words` (as palavras de um segmento, com as aspas e os escapes do POSIX, e os descritores `2>&1` como sintaxe; os segmentos em `;`, `|`, `&`, `&&`, `||` e nova linha; um segmento sem as suas redireções), `invocation` (o programa que um segmento corre através de `sudo`, `env -S`, `xargs`, `timeout`, `command`, `builtin`, `busybox` e das palavras de controlo da shell; `PATH=` e `env -i` como mudança da resolução do executável; `xargs` como comando guiado por dados; a leitura de argumentos que os mutators partilham), `cmdlet` (como `Set-Content`, `Add-Content`, `Clear-Content`, `Out-File`, `Tee-Object`, `New-Item` e `Set-Item` ligam os seus parâmetros, por nome, por prefixo inequívoco, por alias ou por posição, com `--` e `-Switch:valor`; a leitura exata, com as aspas, e a leitura "desaspada" de um comando POSIX, em que todos os valores contam), `powershell` (as palavras em modo de argumento, com as aspas tipográficas, os backticks, as arrays `a, b`, os grupos `(...)`, `$(...)`, `@(...)` e `{...}` entregues como comandos próprios, `$x = comando` em cadeia, `& 'comando'`, `--%`, os comentários; os aliases; os comandos que tomam o caminho do pipeline), e `mod` (os alvos de cada mutator — `cp`, `mv`, `install` com destino diretório ou `-t`, `rm`, `tee`, `touch`, `truncate`, `sed -i`, `perl -i`, `find -delete`/`-fprint`, `dd of=`, `rsync`, os cmdlets —, as redireções do POSIX fora de aspas, os diretórios possíveis com o limite de 64 e os homes fixos, `~`, `$HOME` e `$PWD`, `/dev/null` e `nul`, e `writeTargets` para as ferramentas `Write`, `Edit`, `MultiEdit` e `NotebookEdit`). O host entra por um valor (`Host`: o diretório do processo, se é Windows, `HOME`, o perfil do utilizador), para que o corpus pergunte por qualquer host e os hooks passem o real.

Em `focus-js::path`: `resolve`, `normalize`, `isAbsolute`, `join` e `basename` do `node:path`, nos sabores POSIX e Windows, como o Bun os implementa: o `lib/path.js` do Node de antes do tratamento dos nomes reservados do Windows (CVE-2024-36139; `CON:` e `./C:x` passam como estão) e com uma raiz de dispositivo (`\\.\X`, `\\?\X`) lida como raiz UNC, servidor e partilha. É o que o original corre, por isso é o que se porta. O diretório do processo entra como argumento. Um diretório de trabalho próprio de uma drive (`process.env["=C:"]`) não se modela: o Node cai no do processo sem ele.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `shell_invocations` (613) | Comandos gerados por uma gramática (mutators com as suas opções, wrappers encadeados, palavras de controlo, `cd` nas suas formas, redireções, as aspas dos três tipos, caminhos com `~`, `$HOME`, `$PWD`, variáveis, globs, Windows, Unicode) e 8 % deles com caracteres ao acaso: os detalhes de cada invocação, a lista sem detalhes e a mudança da resolução, iguais em todos |
| Corpus, `shell_targets` (819) | 707 em POSIX e 323 em PowerShell, 39 deles fixos, para as linhas que a mistura ao acaso raramente alcança: os alvos, as palavras em bruto, os diretórios possíveis e as mudanças de diretório, iguais em todos. O diretório de trabalho é uma raiz de rascunho com os mesmos dois níveis e o mesmo nome dos dois lados, para que um `..` que lhe saia e um `cp` para o seu pai caiam no mesmo sítio; os diretórios que o caso nomeia existem nos dois |
| Corpus, `write_targets` (90) | As ferramentas de escrita com os seus campos, as de leitura, `Bash` com e sem comando |
| Corpus, `node_path` (442) | 211 em POSIX e 231 em Windows, sobre raízes de drive, UNC, de dispositivo, `..` acima da raiz, separadores dobrados, nomes reservados, e 22 fixos (`join` a começar num separador, `resolve` de uma drive que não é a do diretório). Um desvio do Node foi apanhado aqui: o Bun lê `\\.\PHYSICALDRIVE0` como raiz UNC, e o porte seguiu o Bun |
| Mutações | 45 em `words`, `invocation`, `cmdlet`, `powershell`, `mod` e `path`; 44 apanhadas, 7 delas só depois de o corpus ganhar os casos fixos (um `sed --in-place` guiado por dados, `-Variable:v` numa leitura POSIX, um backtick dentro de um grupo e de aspas dentro de um grupo, um pipeline continuado na linha seguinte, um `join` do Windows a começar num separador). A que fica é equivalente: ler `&&` como dois `&` dá um segmento vazio, que não corre programa nenhum |
| Unidades | Os ramos de Windows que o corpus, gravado em Linux, não alcança: os homes do PowerShell pelo perfil (`USERPROFILE`, `HOMEDRIVE`+`HOMEPATH`), `nul` como dispositivo nulo, os caminhos no sabor Windows; e as formas do `path` que o Bun responde (medidas à mão contra o Bun 1.4.2) |
| `cargo test` | O corpus passou de 9 810 para 11 759 casos. Os casos de `shell_targets` e `write_targets` não correm em Windows: leem a raiz de rascunho em `/tmp` (R8); os de `shell_invocations` e `node_path` correm nas três plataformas |

**Desvios deliberados.** Nenhum.

**Limites da fatia.**

- Nada consome ainda o lexer: a integridade do runtime (fatia 2) e os guards (fatia 4) são os seus clientes.
- O corpus regenerado trouxe outra vez os casos das famílias anteriores, com outro conteúdo: a geração com a mesma semente não é reprodutível de uma máquina para outra (a ordem de um `glob` entra nela). Todas passam; fica como risco pequeno, a corrigir quando se tocar no gerador.

### Fatia 2: integridade do runtime

`runtime-integrity.ts` (1 427 linhas) é a primeira pergunta que os hooks de `PreToolUse` fazem a uma chamada de ferramenta, antes de qualquer fence: toca em algo que só o harness escreve? Os registos do runtime (`.focus-sessions`, `.focus-plan-approval`, as palavras de um gate em `.focus-engine/gate-words`), o audit trail (`focus/spaces/<space>/intents/<registo>/audit/`, ancorado na árvore do workspace, nunca num segmento `audit` solto), e os ficheiros instalados de que a aplicação das regras depende (`hooks/`, `tools/focus*`, `bin/focus`, `hooks.json`, o `settings.json` do Claude, os agentes do Kiro, os pontos de entrada nativos). Lê a chamada por três caminhos: um comando `Bash`, pela shell (as substituições que corre, os heredocs que entrega a um interpretador ou escreve num script, as variáveis que define e expande, os scripts em disco que corre, os `-c`/`-e` dos interpretadores, os wrappers); uma ferramenta de escrita, pelos alvos; e o conteúdo escrito ou editado, por um lexer de JavaScript e Python que só resolve referências literais e estaticamente ligadas (`require`, `import`, `eval`, `Function`, `vm`, `child_process`, `Bun.spawn`, `subprocess`, `os.system`), com os parâmetros, as declarações locais e os sombreamentos que as desligam. Não é uma sandbox: o próprio original o diz; é defesa em profundidade.

**O que existe.** Em `focus-guards::integrity`: `paths` (os caminhos protegidos, o caminho canónico do prefixo que existe, as raízes de instalação de cada projeto — a do harness resolvido, salvo com um link no caminho, e as convencionais —, os scripts de confiança), `shell` (a caminhada pelo comando, os heredocs e o pipeline em que correm, as atribuições literais e a expansão de cada palavra escrita contra todas elas, a leitura do audit trail que o parser partilhado não decide), `source` (o lexer e a resolução de APIs), e `mod` (a classificação `runtime`/`content`/`audit` e os dois textos de recusa). O host entra por `Integrity` (o diretório do projeto do hook, `FOCUS_HARNESS_DIR`, o binário do engine). Em `focus-js::path`, `relative` e `dirname` nos dois sabores; `relative` dobra o caso só em ASCII, como o Bun (o Node dobra tudo e compara por segmentos quando isso move um carácter).

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `runtime_integrity` (1 794) | Uma árvore de projeto por caso (ficheiros, diretórios, links, bits de execução, um `harness.json`), o pedido do hook lido dela, e a classe e o texto da recusa: 869 permitidas, 604 `runtime`, 181 `audit`, 140 `content`. 1 131 comandos `Bash` (540 ao acaso de uma gramática própria e do lexer da fatia 1, o resto fixos: um por ramo), 392 `Write` (cada excerto de JavaScript, Python e shell escrito num script, num `.sh` e num `.py`), e `Edit`, `MultiEdit`, `NotebookEdit`, ferramentas de leitura e pedidos malformados. Iguais em todos. Um ficheiro grande de mais para ler é gerado com a mesma forma dos dois lados (um `require` de um hook e um comentário até ao tamanho) |
| Corpus, `node_path` (+42) | `relative` e `dirname`, com os casos fixos das drives, das raízes UNC e do `İ` que o Bun não dobra |
| Mutações | 50 em `source`, `shell`, `paths` e `mod`; 50 apanhadas, 31 delas só depois de o corpus ganhar os casos fixos. Os casos fixos apanharam também um erro do porte que a mistura ao acaso não via: o fecho de uma substituição (`)` ou o segundo backtick) era lido outra vez como texto |
| `cargo test` | 392 testes; o corpus tem 13 571 casos. Os casos de `runtime_integrity` leem a árvore em `/tmp` e não correm em Windows, como os de `shell_targets` (R8) |

**Pormenores do original que ficam iguais.**

- O lexer de fonte trata os comentários, os corpos de expressões regulares e as strings como opacos, e abre um literal só quando um `import`/`require` ou um sink o consome; uma interpolação de template é código.
- Uma palavra que a shell calcularia (`$X`, um glob, uma substituição que não é um `echo`/`printf %s` literal) é recusada quando o comando pode estar a apontar ao audit trail: nomeia um segmento `audit`, ou entra em `focus/spaces`, ou já corre lá dentro.
- O repositório autorado do original (`scripts/package.ts` ao lado de `core/`, `harness/`, `tests/`, `docs/`) pode escrever os seus próprios hooks: a exceção fica, embora Focus não a precise.

**Desvios deliberados.** Nenhum. Os nomes dos ficheiros instalados (`hooks/focus-*.ts`, `tools/focus*`) seguem o mapa de nomes; a árvore que Focus instala (M8) decide quais existem, e proteger um nome que não existe não custa nada.

**Limites da fatia.** Ninguém chama ainda `integrity::violation`: os hooks de `PreToolUse` (fatias 3 e 4) são os seus clientes. O binário do engine entra como ponto de entrada protegido só quando o processo o conhece (`current_exe`); o corpus não o passa.

### Fatia 3a: o dispatcher e os observadores

Os hooks são o que o harness corre em cada evento de uma conversa. No original cada um é um módulo em `core/hooks/aidlc-<nome>.ts` que exporta `run(input)`, e `aidlc.ts engine hook <nome>` lê o stdin, chama-o e sai com o seu código. Em Focus é `focus engine hook <nome>`, com o mesmo contrato: o pedido do host em JSON pelo stdin, a resposta pelo código de saída, pelo stdout e pelo stderr, e o que o hook deixa no registo. Esta fatia traz o dispatcher e os cinco hooks que só observam: registam e mantêm o registo em passo, e não decidem nada.

Antes de escrever, cada hook pergunta em que workflow dispara e se esta conversa o integrou (`enterHookWorkflow`, `workflowParticipation`): a sessão do pedido, se tem uma ligação (`.focus-sessions/<id>.binding.json`), fica fixada como a sessão com que todos os helpers de caminhos resolvem; é participante quem tem o cursor a nomear o registo, ou uma ligação de uma fonte de confiança (`create`, `migration`, `switch`, `space-switch-cursor`, `cursor`, `stamp`); um registo solitário sem cursor nem ligação resolve-se (um worktree não herda cursor) mas é de um colega para o que esta conversa mostra, e o hook fica de fora. Cada hook deixa a sua pulsação em `<registo>/.focus-engine/hooks-health/<hook>.last` (ou, sem registo, no do space), e o que teve de engolir em `<hook>.drops`, nunca através de um link.

**O que existe.** Em `focus-hooks`: `context` (o workflow de um hook, a participação, a sessão fixada), `health` (o diretório de saúde, a pulsação, as quedas, o log de depuração ligado por `FOCUS_HOOK_DEBUG` ou por `focus/.focus-hook-debug`), `engine` (escrever um ficheiro do engine sem seguir links; as recusas pelo nome dos registos de marcos posteriores), `payload` (o pedido como o original o lê: só um objeto; `session_id ?? sessionId`; um campo de texto, e o que o original faz a um valor de outro tipo), `registry` (os spaces, um intent pelo uuid, o uuid do intent ativo, o carimbo de uma sessão), `status` (`set-status`, partilhado com `sync-workflow-state`), `trace` (o traço por fase em `FOCUS_HOOK_TRACE_DIR`), e os cinco hooks: `validate-state` (`PreCompact`: o breadcrumb de recuperação com a validade do ficheiro de estado, e `SESSION_COMPACTED`), `session-end` (a quem pertence a sessão — a ligação, o carimbo, o cursor só num workspace sem uuid — e `SESSION_ENDED` com a razão), `log-subagent` (`SUBAGENT_COMPLETED` enquanto o workflow corre, com a mensagem cortada a 200 unidades UTF-16), `sync-workflow-state` (um `TaskUpdate` para `in_progress` cujo `activeForm` acaba em `[slug]` move a stage; `ide-audit-sync` espelha a última `STAGE_STARTED` do audit trail, só para a frente, e deixa `sync-workflow-state.noop` quando nada há a fazer), `write-audit-log` (uma escrita dentro do registo ou do codekb é `ARTIFACT_CREATED` ou `ARTIFACT_UPDATED`, com o contexto `a > b > c`; o audit trail e o que está fora não são). Em `focus-cli`, `focus engine hook <nome> [--project-dir] [--harness]`: o projeto vem de `FOCUS_PROJECT_DIR`, depois de `CLAUDE_PROJECT_DIR` (o que o host passa), depois da flag ou da descoberta. O diferencial e o replay gravado passam o pedido de um passo `hook` pelo stdin.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `hook/*` | 169 verificações: 139 iguais, 30 desvios, 0 diferentes. Por hook: o pedido vazio, malformado, que não é um objeto, sem sessão, com a sessão sob o outro nome (e `null` sob o primeiro), com uma sessão que não é um nome; participante pelo cursor e por cada fonte de ligação de confiança; de fora (ligado a outro registo, `unjoined`, fonte sem confiança, fonte sem confiança com o cursor noutro registo); sessões por processo sem sessão, com uma sessão sem ligação e com uma ligada; worktree delegado; sem ficheiro de estado, sem shard, duas vezes, com os observadores de rota; `validate-state` com secções em falta e com o diretório do engine como link; `session-end` com cada tipo de razão, ligado a nenhum registo, carimbado, carimbo desconhecido (a queda vai para o registo do cursor), carimbo em branco, sem ligação num workspace com uuid e num plano, noutro space; `log-subagent` com os campos de cada tipo, a mensagem longa e multilinha, o workflow parado; `sync` com cada forma do `activeForm`, a stage já concluída, cada guarda do espelho do IDE (parado, sem stage, sem linha, slug inválido, stage desconhecida, outro shard); `write-audit-log` por `Write`, `Edit` e outra ferramenta, ficheiro em falta, a raiz do registo, fora dele, noutro registo, no codekb, no audit trail e ao lado dele, caminho relativo, com barras invertidas, com `..`, um diretório |
| Testes do CLI (`crates/focus-cli/tests/hook.rs`) | 15, nas três plataformas: a superfície do comando, o que cada hook deixa, quando fica de fora, o projeto vindo do host, o traço, e cada desvio marcado |
| Unidades | 19 em `payload`, `health`, `engine`, `registry` e `context`. Apanharam um erro do porte que o diferencial ainda não tinha: `session_id: null` não passava ao `sessionId` (`??` passa por cima de `null`); o diferencial ganhou os dois casos |
| Mutações | 38 em `context`, `payload`, `health`, `engine`, `registry`, `status` e nos cinco hooks; 36 apanhadas, 13 delas só depois de os testes ganharem o caso que faltava (o cursor a nomear outro registo com uma ligação sem confiança a este, uma fonte de ligação de cada vez, o claim de unit com o cursor noutro lado, a sessão fixada só com ligação, `unjoined` sem pulsação, o fim de sessão com sessões por processo, um workflow concluído e não parqueado, uma stage saltada e um workflow parado no espelho do IDE, um estado `pending` na tarefa, um registo cujo nome começa como o deste, um ficheiro mudado depois de nascer). As duas que ficam são equivalentes: `stands_outside` só vê uma participação indeterminada sem seleção, e um fim de sessão ligado a `unjoined` já resolve para nenhum registo pela sessão fixada |
| `cargo test` | 426 testes. Diferencial completo: 2 526 verificações, 2 343 iguais, 183 desvios, 0 diferentes; 2 493 gravadas, sobre 712 árvores. `write-audit-log/record_root_itself` fica fora da gravação: criado ou atualizado decide-se pelo tempo entre o nascimento do diretório e a última escrita, que a árvore do replay decide de novo |

**Pormenores do original que ficam iguais.**

- `validate-state` e `session-end` sem sessão no pedido registam pelo cursor; `log-subagent` e `write-audit-log` precisam dos campos do pedido; `sync-workflow-state` não lê o nome da ferramenta, só os campos da tarefa.
- Uma ligação a um registo sem ficheiro de estado não é uma ligação; sem ela nem cursor, a pulsação vai para o space.
- `agent_id` e `last_assistant_message` escrevem-se quando o valor é verdadeiro em JavaScript: um array vazio escreve `**Message**: ` vazio; `0` e `false` não se escrevem.
- Uma escrita de um ficheiro que já não está lá, ou por uma ferramenta que não é `Edit`, é uma criação quando a hora de modificação ainda é a de criação (a 10 ms).

**Desvios deliberados.** Os cinco últimos têm um teste marcado, uma entrada em `DEVIATIONS` e uma linha aqui; o primeiro é o lock da secção 10, que o diferencial trata como trata o `.focus-lock`.

| | Original | Focus |
| --- | --- | --- |
| O lock do original à volta do marcador de diretiva | Deixa um ficheiro vazio `active-directive.lock.gate-mutex` no diretório do engine (o mutex nativo da recuperação do lock) | O lock é o do workspace (secção 10); o ficheiro não se compara, como o `.focus-lock` |
| Um nome que não é nome de hook (`../x`, vazio) | O dispatcher procura o ficheiro e recusa à sua maneira (`unknown verb`, saída 2) | Recusa o nome antes de procurar (saída 1) |
| Um hook que esta fatia não traz | Corre | Recusa pelo nome, dizendo o que o hook faz (saída 1) |
| Um registo de um marco posterior ao lado do hook: o marcador de diretiva, um claim de unit, o livro de subagentes em curso, as autorizações de resumo | Lê-o ou limpa-o e continua | Recusa o hook pelo nome enquanto lá estiver (M4/M5), e não regista nada. (As regras entregues à conversa, `<id>.rules-held.json` e `<id>.rules-delivered.json`, que esta fatia também recusava, fecharam na 3c-ii: a compactação esquece o texto integral entregue, como no original) |
| Um worktree delegado | Resolve o worktree para o intent do checkout principal | Recusa pelo nome. Qualquer outra falha da resolução deixa o hook de fora, como no original. (As sessões por processo, que esta fatia também recusava, fecharam na 3c-i) |
| Um campo do pedido de outro tipo (`activeForm: 5`, `file_path: true`, `last_assistant_message: {}`) | Cai com o `TypeError` do runtime, que o dispatcher imprime | Recusa o campo pelo nome. Saída 1 nos dois, nada registado por nenhum |

**Limites da fatia.**

- `rebuild-stage-graph` é a fatia 3b; `session-start` a 3c-ii; `record-human-turn` entra com os guards (fatia 4), porque escreve nas lojas que eles leem.
- O log de depuração lê a flag do ambiente e o marcador do workspace; o original lê também o `settings.json` do projeto (`resolveProjectFlag`). Entra quando os guards (fatia 4) a precisarem.
- `set-status` deixa a preservação do cursor de uma Construction unit-major por fazer, por ler o marcador de diretiva (M5).


### Fatia 3b: a recompilação e a ligação observada

`rebuild-stage-graph` dispara depois de cada chamada `Bash` do agente e recompila o runtime graph quando a chamada foi uma ferramenta de transição e o audit trail acabou numa linha de transição. Filtra barato pelo comando (`classifyRuntimeCompileCommand`: as ferramentas `state`, `jump`, `bolt`, `unit`, `utility` pelo script ou pela gramática nativa, `report` pelo orquestrador, e nunca a própria `runtime`, que o faria recompilar em ciclo); lê os três últimos blocos de todos os shards do registo que esta conversa integrou (uma aprovação escreve até três linhas numa chamada; um fim de workflow cinco, e `WORKFLOW_COMPLETED` está no conjunto por isso); e corre `focus engine runtime compile` como processo próprio, com a sessão do pedido passada por `FOCUS_SESSION_OVERRIDE`, registando uma saída não-zero como queda. No modo do IDE (`source: ide-audit-sync`) não há comando: o portão é só a cauda, com a marca `.noop` para o front gate e o guarda de idempotência pela data do grafo contra a do shard mais novo. Antes de tudo isso, duas coisas que não precisam de workflow: uma criação vista na saída da chamada (`Intent created: <registo> (space: <space>)`) liga a sessão que a fez — provada pelo recibo de criação que o `intent-create` desta máquina deixou e gasta uma vez (`create`), ou só observada (`observed-create`, que não move uma sessão participante noutro registo nem carimba); uma segunda criação deixa o recibo de passagem (`<id>.handoff.json`) para o Stop hook — e a diretiva de erro do engine na saída de uma chamada literal ao orquestrador seria entregue à pessoa pelo canal do harness.

**O que existe.** Em `focus-guards::engine_command`: `canonical_engine_command`, `classify_runtime_compile_command`, `parse_literal_shell_invocation` (a gramática literal de um comando: um só, com um `cd <absoluto> &&` opcional antes e um `2>&1` opcional depois, sem expansões nem operadores), `engine_invocation_from_words` (a ferramenta do engine atrás dos wrappers transparentes `env`, `command`, `exec` e das atribuições, com os flags globais retirados e o `--project-dir` lido), `literal_orchestrate_verb`, `shell_tool_response_text`. Em `focus-hooks`: `hooks::rebuild_stage_graph`, `sessions` (o carimbo de uma sessão e o recibo de passagem), `registry::intent_uuid_of`, `health::cursor_selection` (o registo que os helpers do original nomeiam quando o hook não passa nenhum); em `focus-intent::record`, `consume_creation_receipt`. O diferencial normaliza o nome da ferramenta `runtime` dentro de uma queda e o instante de um recibo de passagem.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `engine_command` (453) | 93 fixos (cada grafia do dispatcher e das ferramentas, cada wrapper, cada prelúdio `cd`, cada operador que recusa a leitura literal, os flags globais com e sem valor) e 360 gerados: a classe da recompilação, a invocação literal com as suas palavras e o verbo do orquestrador, iguais em todos |
| Diferencial, `hook/rebuild/*` | 65 verificações: 58 iguais, 7 desvios, 0 diferentes. Cada classe de comando nas duas grafias, uma menção entre aspas, duas vezes; a transição no terceiro bloco a contar do fim e no quarto, o fim do workflow, o gate aberto, o campo com espaços, o campo dentro de uma linha, o trail em CRLF, outro shard, sem shard, sem estado; participante pelo cursor, de fora, `unjoined` (os helpers seguem o cursor, como no original), sem sessão, o pedido vazio, sem comando, comando de outro tipo; o modo do IDE: dispara, idempotente, sem transição, grafo mais antigo que o shard; a ligação observada: provada, só observada, nativa, migrada, resposta em texto e em objeto, sem resposta, outro comando, registo desconhecido, outro space, recibo de outro uuid, recibo gasto à segunda, já ligada (com e sem fonte), carimbo noutro registo (o recibo de passagem) e no mesmo, participante noutro registo que a observação não move e a prova move, de fora que a observação move, pelo IDE, registo plano; a diretiva de erro, pelo dispatcher e em texto, e as chamadas que não são literais ou não são erro |
| Testes do CLI | +4 (19): a recompilação por classe de comando e pela cauda, o espelho do IDE uma vez por mudança, a ligação observada e provada com a passagem, o desvio da diretiva |
| Unidades | +5 em `engine_command`, +2 em `sessions`, +1 em `record` (o recibo) |
| Mutações | 44 em `engine_command`, no hook, em `sessions` e no recibo; 44 apanhadas (a leitura literal e a classificação pelo corpus; o hook pelos testes do CLI) |
| `cargo test` | 438 testes; o corpus tem 14 024 casos. Diferencial completo: 2 590 verificações, 2 401 iguais, 189 desvios, 0 diferentes; 2 556 gravadas sobre 730 árvores (`rebuild/ide_stale_graph` fica ao vivo: a idade do grafo é posta à mão, o que a gravação não leva) |

**Pormenores do original que ficam iguais.**

- O separador final do shard fecha um bloco vazio, que conta entre os três últimos: uma transição seguida de duas linhas quaisquer já não dispara.
- Um `tool_response` que não é texto nem traz `stdout` não é lido para a diretiva, mas é lido, como JSON, para a criação.
- A queda da recompilação vai para o registo do cursor, não para o da sessão: é onde o `--doctor` a lê.

**Desvios deliberados.** Cada um tem um teste marcado, uma entrada em `DEVIATIONS` e uma linha aqui; os dos registos de marcos posteriores e das resoluções não portadas são os da fatia 3a, que este hook partilha.

| | Original | Focus |
| --- | --- | --- |
| Uma chamada literal ao orquestrador cuja saída nomeia uma diretiva de erro | Valida a diretiva e entrega a mensagem à pessoa pelo canal do harness (`systemMessage`), com uma nota para o modelo | Recusa o hook pelo nome antes de validar: a diretiva é do orquestrador (M5) |
| `command` de outro tipo no pedido | Cai no `TypeError` da linha de depuração | Recusa o campo pelo nome |

**Limites da fatia.** O processo filho da recompilação é o próprio binário (`current_exe`), sem o `timeout` de 15 minutos do original. `record-human-turn` passou para a fatia 4: escreve nas lojas que os guards leem (as palavras de um gate, a resposta a uma pergunta protegida, o switch tipado de uma fence) e não faz sentido sem elas.


### Fatia 3c-i: sessões por processo

Um harness corre os comandos do engine a partir de uma shell que ele próprio lança, e não lhe dá o id da sessão. O arranque de sessão regista por isso o processo do harness e cada um dos seus antepassados como da conversa (`focus/.focus-sessions/pids/<pid>`, com a hora de arranque do processo, para que um pid reutilizado não passe por dono), e um comando a que ninguém nomeou a sessão sobe pelos seus antepassados até ao primeiro registado (`resolveSessionIdFromAncestry`). Um override exportado para o ambiente (`FOCUS_SESSION_OVERRIDE` sem a marca `payload`) que discorde da conversa dona é recusado (`SessionResolutionConflictError`): é um guarda contra um export velho, não uma fronteira de segurança. Até aqui Focus recusava pelo nome sempre que o mapa existia; agora percorre-o, e todos os comandos e hooks herdam isso pela resolução.

**O que existe.** Em `focus-workspace::ancestry`: a identidade de um processo (pai e hora de arranque: `/proc/<pid>/stat` em Linux e `ps -o ppid=,lstart=` em macOS, como o original os lê; em Windows a tabela de processos por `sysinfo`, ao segundo), os registos do mapa e a sua leitura, `write_ancestry` (o slot do pai reformado antes de ser lido, para que uma sessão velha não volte quando a leitura recupera; o GC dos registos de processos que já não existem ou cujo pid mudou de dono), `resolve` (a caminhada com o orçamento de 50 ms e a memória de uma falha por um segundo), e `write_atomic_unsynced` para estes registos, que o original também não leva ao disco. Em `records::resolve_selection`, `invoking_session_id`: o override entregue pelo hook, depois a árvore. `not_ported` fica só com o worktree delegado.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `*/sessions_by_process*` | 40 verificações, 40 iguais, 0 desvios: um mapa que não nomeia nenhum antepassado (o cursor decide), e o processo do próprio diferencial registado como da conversa `s1` — o pai dos dois engines, com a hora de arranque como os dois a leem em Linux —, com e sem ligação, em `graph resolve` (a escrita do plano vai para o registo da ligação, não para o do cursor), `audit append`, `runtime compile` e `summary`, `state get`, e em cada um dos seis hooks sem sessão no pedido; o override exportado que discorda (a mesma frase de recusa), o entregue pelo payload e o igual. Ficam ao vivo: o pid é o desta corrida |
| Unidades | `ancestry`: os registos lidos como escritos ou nada, a ordem das horas em Windows, e a caminhada no próprio processo de teste (o pai registado, uma hora de outro processo, um registo reformado, o GC); `records` e `context`: a conversa encontrada pela árvore, o override exportado recusado, o do payload e a sessão nomeada a ganhar |
| Testes do CLI | `graph resolve` pelo pai registado; o hook de compactação pelo pai registado com o cursor noutro lado |
| `cargo test` | 443 testes. Diferencial completo: 2 605 verificações, 2 431 iguais, 174 desvios, 0 diferentes; 2 555 gravadas sobre 730 árvores |

**Desvios que fecham.** Os de `sessions_by_process` em `verbs/resolve_writes`, `audit/append`, `state/record`, `runtime/*` e `hook/*`, e os testes marcados que os acompanhavam (secções 13 e 14, e as fatias 3a e 3b desta).

**Limites da fatia.** Em Windows a hora de arranque lê-se ao segundo: um pai nascido no mesmo segundo que o filho não se verifica (o original lê o relógio de 100 ns do kernel e nunca encontra esse caso), e a caminhada fica por aí; o comando cai então no cursor, como com o mapa vazio. A fronteira de segurança não muda: o mapa é estado por utilizador. O orçamento de 50 ms é o do original; num host carregado a caminhada falha fechada, como lá. Ninguém escreve ainda o mapa num projeto real: é o arranque de sessão (3c-ii).

### Fatia 3c-ii: o arranque de sessão

O arranque de sessão (`SessionStart` em Claude Code; o prompt hook de Cursor e Copilot chama-o com `rebind_check`) é o hook que mais escreve: é ele que liga uma conversa nova ao registo que o cursor partilhado nomeia e a carimba com o uuid desse registo, que regista o processo do harness no mapa (3c-i), que mantém a cópia da memória ativa que os includes do harness importam (`focus/active-memory/`), que anota que regras o chat carrega (`<id>.rules-held.json`, o que `next` lê para saber se tem de reenviar o texto de um stage), e que entrega ao modelo o contexto do trabalho em curso. Como corre depois de o harness ter lido os seus includes, quando acaba de escrever a cópia diz ao chat o que mudou: o texto dos ficheiros, se couber em 8 KiB, senão os seus nomes.

**O que existe.** `focus-hooks::hooks::session_start`, com o fluxo do original na mesma ordem: o pedido (fonte, sessão, transcript, `rebind_check`; um pedido que não é JSON é um arranque sem sessão), o ponteiro do transcript, a sessão viva (`.current-session`), os bypasses com que o harness arrancou (`presence-bypass-<id>`, `plan-approval-bypass-<id>`, este limpo quando a variável não está), o mapa de processos, a oferta de religar limpa num arranque, a resolução (um carimbo de `resume` sem ligação aponta o registo; senão a seleção, e a ligação escrita com a sua origem: `create`, `cursor`, `stamp`, `unjoined`, `none`), o cursor do espaço, a cópia da memória (`active_memory::drift`/`refresh`/`inline_memory_files`), `rules_held::record_rules_load` (Claude: os hashes dos ficheiros que o stub `rules/focus.md` importa, e `stale` quando a cópia acabou de ser escrita, num `fork`, num `resume` que mudou, ou numa fonte desconhecida; Cursor e Copilot não têm registo), e então os dois ramos: sem ficheiro de estado, a linha `FOCUS Runtime Session`, com a oferta de voltar ao registo que a conversa trabalhava, os switches desligados e a entrega da memória; com estado, a pulsação, a linha `SESSION_STARTED`/`SESSION_RESUMED` no shard (nenhuma em `compact` e nas fontes desconhecidas), o carimbo e a oferta de religar (uma conversa trabalha um registo; o cursor é partilhado; a oferta faz-se uma vez por assinatura, e a sonda de Cursor escreve-a como aviso de seleção), e o contexto `FOCUS WORKFLOW ACTIVE` lido do estado, com a linha da unit, a nota de recuperação, a deriva do grafo (stages em disco que o runtime graph não tem) e os switches. `focus-hooks::switches` lê o que `config flags` gravou (`recorded-switches.json`) e diz cada check desligado numa frase, com a data em hora local e as palavras da pessoa. Em `sessions`: a ligação, a sessão viva, a oferta e o aviso de seleção, o transcript (com o kill switch do usage), os bypasses. O barreira do observador entra onde o original a tem (`writeFileAtomic`): a cópia da memória, o registo de regras e os bypasses; um observador (`STOP_HOOK_PROBE`, `ROUTE_CHECK`) arranca a sessão sem escrever nada.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `hook/session-start/*` | 75 verificações, 72 iguais, 3 desvios, 0 diferentes: cada fonte (`startup`, `resume`, `clear`, `compact`, desconhecida, número, sem fonte), o pedido vazio, não-JSON e não-objeto, sem sessão e com um id inválido, o transcript com e sem o kill switch, um ficheiro de settings recusado, dois arranques seguidos, só cursor, frio, sem ficheiro de estado, um registo não integrado (oferta de voltar, no mesmo espaço e noutro; a sonda uma vez), ligada a outro registo, `unjoined`, `none`, carimbada (`resume`, `startup`, uuid desconhecido), a oferta de religar (uma vez; a sonda; só por carimbo; limpa num arranque), registo plano, ligação sem origem, as origens `migration`/`switch`/`stamp`, os bypasses por ambiente e por settings, a linha da unit (pausada, em stage de unit, de equipa, nome inválido), estado esparso, a nota de recuperação, a deriva do grafo, a cópia atual, desatualizada, a memória inline (a frio, um ficheiro atrasado), por nome e não-UTF-8, os switches gravados (com palavras, sem data, por ambiente, registo de outra versão, numa sonda, a frio), um `rules-held` anterior, sem shard, os dois observadores com um bypass por registar; e os casos que a mutação pediu: um `rebind_check` que não é `true`, uma sonda que já fez a oferta, um carimbo noutro registo ao lado de uma ligação sem uuid |
| Diferencial, `hook/*` | 319 verificações, 291 iguais, 28 desvios, 0 diferentes (os de `validate-state` com `rules-delivered.json` passam a comparar) |
| Unidades | `switches`: as frases, a citação capada, o relógio local, um registo de outra versão, um ficheiro de settings recusado; `sessions`: os nomes que uma ligação e um comando aceitam, a ligação com e sem origem, a sessão viva, a oferta, o aviso, o transcript com o kill switch, os bypasses; `active_memory`: os blocos inline e o teto |
| Testes do CLI | O arranque que liga e entrega (o contexto, os registos da sessão, o shard, `stale` até um arranque que leia a cópia atual); o arranque frio (a memória inline; sem sessão nada é escrito nem dito); a oferta de religar uma vez e a sonda; a oferta de voltar; a unit, a nota, a deriva e os switches; os bypasses e um ficheiro de settings recusado; os observadores; os três desvios |
| Mutação | 46 mutantes em `session_start`, `sessions`, `switches`, `rules_held` e `active_memory`: 46 apanhados, depois de a primeira corrida deixar escapar 6 (o `rebind_check` que não é `true`, a oferta de voltar feita uma vez numa sonda, os switches numa sonda sem estado, o uuid dono com uma ligação sem uuid, a assinatura vazia da oferta, a versão do registo de switches), cada um fechado por uma verificação ou um teste |
| `cargo test` | 466 testes. Diferencial completo: 2 680 verificações, 2 505 iguais, 175 desvios, 0 diferentes; 2 627 gravadas sobre 765 árvores |

**Desvios deliberados.** Com teste marcado, `DEVIATIONS` e esta linha.

| | Original | Focus |
| --- | --- | --- |
| Uma árvore da distribuição em que `config` nunca correu (`focus-projection.json` com `rootIntegrations`, sem `focus-manifest.json`) | `addRootBlocks` acrescenta a parte do engine aos ficheiros de raiz da equipa (`.gitignore`, `AGENTS.md`) e diz ao chat que os includes foram reapontados | Recusa pelo nome até à fusão da distribuição (M8); o manifesto de instalação, que `config` escreve, fecha a recusa |
| `FOCUS_SESSION_ANCHOR=1` | Depois da linha da sessão, percorre os commits recentes e escreve `SOURCE_COMMITTED` (`attest`, adiado) | Recusa pelo nome depois da mesma linha |
| Um claim de unit (`.focus-unit-scope.json`) como origem da ligação | Lê-o | Recusa pelo nome (M4, units) |

**O que o diferencial deixa ao vivo.** O mapa de processos (`.focus-sessions/pids/<pid>`) nomeia os processos deste host: compara-se ao vivo e não se grava nem se relê (`workspace_files` do replay salta-o); um switch que ninguém gravou leva a hora de modificação do ficheiro de settings, que é a da corrida; um ficheiro de memória que não é UTF-8 não cabe numa árvore gravada, que é texto. O registo de regras guarda o hash de cada ficheiro de memória, e esses ficheiros diferem entre os engines pelo nome no texto: um hash que é o do ficheiro da própria árvore mostra-se `<sha256>`, no diferencial e no replay; a árvore base das fixtures passa a ter o stub `rules/focus.md` que os nomeia.

**Limites da fatia.** Kiro, OpenCode e Codex não são harnesses de Focus: o registo de regras só tem os ramos de Claude (hashes) e de Cursor/Copilot (sem registo). A sonda de Cursor escreve o aviso de seleção que o directive de `next` mostra (M5). A anotação de `stale` segue o original: um `resume` herda o estado do registo anterior até um arranque que leia a cópia atual.

### Fatia 4: o que os guards leem, e o que fica para M5

Os quatro guards de `PreToolUse` decidem sobre três coisas: a fence (a Guard Policy e o switch da pessoa: secção 14, fatia 1), a autoridade por trás da ação (os marcadores de turno e o de diretiva; a estampa de um despacho) e a evidência do trabalho (os recibos de revisão, a confirmação do resumo, o plano aprovado). A evidência é o que mais pesa, e é dela que a fatia 4 se afasta: `freshReviewReceipts` (1 066 linhas) pesa as linhas de revisão do ledger contra a janela da tentativa, as ligações à fonte (`Source Fingerprint`, os claims de source) e as units do bolt DAG — o território da atestação e das units, M5 —, e `checkSummaryConfirmationEvidence` (647) o mesmo para a confirmação do resumo. M3 já recusava pelo nome um gate cujo ledger tem linhas de revisão ("Focus does not weigh review receipts yet"); os guards fazem o mesmo. Fica assim decidido: **os recibos de revisão são uma fatia própria, a primeira de M5**, que fecha de uma vez as recusas de `state` (fatia 2 de M3), de `review-freeze` e do que mais as herdar. O recovery ask (`evaluateGuardRefusal`, `guardAttemptState`, `guardRefusalHookNote`) vai com eles: só se escreve depois de uma recusa, e as recusas que o levam são as que pesam evidência.

### Fatia 4a: `review-freeze`

O hook recusa a escrita que anularia um recibo terminal de revisão fresco: o documento declarado de um stage com revisor que ainda não terminou. Tudo falha aberto, como no original: um pedido malformado, um ledger sem shards, um estado ou grafo ilegível, uma conversa que não integrou o workflow, deixam a chamada passar; `FOCUS_DISABLE_REVIEW_FREEZE_HOOK=1` desliga a verificação, depois da pulsação (a verificação desligada nunca parece um host que não corre hooks).

**O que existe.** `focus-hooks::hooks::review_freeze`: o pedido (a sessão, a ferramenta, o `cwd`, a shell que o adaptador anotou), os alvos de escrita (`focus-guards::shell::write_targets`, da fatia 1: os das ferramentas de escrita e os de uma shell), a saída antes de qualquer leitura quando não há alvos nem shards, os stages com revisor ainda abertos (`parse_checkboxes`), os repositórios do intent, e por documento o matcher `reviewed_artifact_unit` (`focus-lifecycle::gate`: os documentos declarados e opcionais do stage menos o ficheiro de perguntas de um summary, e a unit que um caminho `construction/<unit>/` nomeia). Um documento alcançado cujo ledger tem uma linha de revisão (`REVIEW_REQUESTED`, `REVIEW_COMPLETED`) para esse stage é recusado pelo nome (ver acima); sem linha não há recibo, e a escrita passa como no original. Um ficheiro de settings recusado é a leitura que no original rebenta o hook (`focus: <frase>`, saída 1). Em `focus-guards::workflow`: `Guards::actor_from` (quem o pedido diz que age: um agente despachado por `agent_type` ou `subagent_type`, o driver sem pessoa, a conversa) e `Guards::decide_for` (a fence decidida com essa autoridade: um despacho sem estampa não herda o turno da pessoa), para os guards que se seguem.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `hook/review-freeze/*` | 31 verificações, 25 iguais, 6 desvios, 0 diferentes: uma leitura (com e sem ficheiro de estado), uma escrita fora do registo, o documento de outro stage, os documentos do stage sem linhas de revisão (`Write`, `Edit`, `MultiEdit`, `NotebookEdit`), o ficheiro de perguntas, um stage concluído e um saltado, linhas de revisão (completa, só pedido, de outro stage, de um stage sem revisor), a shell (uma redireção para o documento, uma leitura, outro alvo), um agente despachado, o interruptor por ambiente e por settings, um ficheiro de settings recusado, uma conversa de fora e uma não integrada, sem ledger, sem estado, a frio, pedidos malformados, sem `tool_input`, o marcador de diretiva, os dois observadores |
| Diferencial, `hook/*` | 346 verificações, 0 diferentes |
| Unidades | `gate`: o matcher dos documentos revistos (declarado, opcional, o ficheiro de perguntas, a unit de um caminho e a ambiguidade, um stage que não corre por unit); `workflow`: a autoridade de um agente despachado e quem o pedido nomeia |
| Testes do CLI | Cada escrita que não alcança recibo passa; a que alcança é recusada pelo nome (ferramenta e shell), com o interruptor (e um valor que não é o interruptor), o ficheiro de settings recusado e a conversa de fora |
| Mutação | 13 mutantes em `review_freeze` e `gate`: 12 apanhados (5 depois de verificações que a primeira corrida pediu) e 1 equivalente (a saída antecipada sem alvos: sem alvos nada mais se lê que deixe rasto, porque uma seleção sem ficheiro de estado não tem shards) |
| `cargo test` | 469 testes. Diferencial completo: 2 712 verificações, 2 529 iguais, 183 desvios, 0 diferentes; 2 659 gravadas sobre 776 árvores |

**Desvios deliberados.** Com teste marcado, `DEVIATIONS` e esta linha.

| | Original | Focus |
| --- | --- | --- |
| Uma escrita no documento de um stage com revisor aberto cujo ledger tem uma linha de revisão | Pesa o scan de recibos e congela o documento quando um recibo terminal fresco o cobre (saída 2, uma linha `REVIEW_FREEZE_BLOCKED`, o recovery ask no stderr), ou deixa passar quando nenhum o cobre | Recusa a escrita pelo nome (saída 1) enquanto não pesa recibos (M5) |

**O que o diferencial deixa ao vivo.** O mapa de processos, que o arranque de sessão escreve com um orçamento de 50 ms: sob carga um dos engines não chega a um slot e deixa-o a null; o diferencial deixa de fora um slot que um dos lados não alcançou (a forma `process-map-budget`) e compara o resto.

### Fatia 4b: `reviewer-scope`

Um revisor despachado para uma unit não lê o `construction/<outra>/` das outras, por nenhuma ferramenta: nem abrindo ficheiros, nem por grep, glob ou shell que atravesse as units vizinhas. O condutor escreve o registo de despacho (`<registo>/.focus-engine/reviewer-dispatch.json`: revisor, stage, unit, ficheiros isentos) antes de invocar o revisor, e `log review --verdict` apaga-o (M5). O hook falha aberto sem essa evidência: sem registo, com um registo velho (mais de seis horas: um órfão, apagado), malformado, com um pedido malformado, uma ferramenta que não lê, um agente que não é o revisor despachado; `FOCUS_DISABLE_REVIEWER_SCOPE_HOOK=1` desliga-o.

**O que existe.** `focus-guards::reviewer_scope`, o matcher puro: as strings de uma chamada (os campos de caminho, o comando de uma shell, o padrão e a raiz de `Glob` e `Grep`; o `pattern` de `Grep` é o regex do conteúdo e não se lê), os componentes normalizados (`..` colapsado contra o pai), a ocorrência de `construction/` julgada pelo segmento seguinte (a unit despachada passa; um wildcard ou uma raiz nua varrem; uma vizinha concreta só passa com o sufixo isento exato), o julgamento léxico e o resolvido (contra o cwd, a raiz do registo e a raiz da unit; uma raiz de pesquisa acima de `construction/` varre todas), os comandos de shell segmento a segmento (`cd` move as bases; `grep`/`rg` com as suas opções e a raiz `.` implícita quando não há operando e nada vem de um pipe; `find`, `ls`, `cat` e afins; um comando genérico com os operandos que parecem caminhos), `parse_dispatch_record` e `block_reason`. `focus-hooks::hooks::reviewer_scope`: a pulsação, a ferramenta, o claim de unit de um workflow de equipa (recusado pelo nome: units), o interruptor, o registo e a sua idade, a identidade (`agent_type` igual ao revisor do registo, ou `scoped_registration`), o veredicto, a fence (`relaxed` mantém esta de pé; `off` e o switch da pessoa param de lado com a linha `systemMessage` e a linha `GUARD_STOOD_ASIDE`), a recusa (saída 2 com a razão) e a linha `REVIEWER_SCOPE_BLOCKED`. No `focus-cli`, o que `runHookModule` faz a uma recusa: a razão do stderr entregue ao host também como decisão `deny` no stdout, para os guards todos.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `reviewer_scope` | 1 096 casos (720 ao acaso e 376 fixos: os que a mutação pediu), todos iguais ao original: as dez ferramentas e duas que não leem, caminhos com `construction` em várias grafias e globs, a unit despachada, vizinhas, `..`, isentos por sufixo e por caminho, raízes de pesquisa, comandos de shell com `grep`/`rg`/`find`/`ls`/`cat`/`cd` e pipes, aspas e escapes; `reviewer_dispatch`: 17 registos, válidos e cada forma inválida |
| Diferencial, `hook/reviewer-scope/*` | 37 verificações, 33 iguais, 4 desvios, 0 diferentes: a própria unit, uma vizinha (por caminho, relativo, shell, `cd`), um glob sobre units, um `Grep` sem raiz, um `rg` sem operando, um `grep -r .` dentro da unit, um ficheiro isento e o seu diretório, um sufixo isento, fora de `construction/`, outro agente e nenhum, `scoped_registration`, outra ferramenta, sem registo (e a advertência), registo malformado e velho, o interruptor (`1`, `0`, settings, settings recusadas), as políticas `off`, `relaxed`, `strict` e `strict` com o switch, uma conversa de fora, sem shard, a frio, pedidos malformados, os observadores |
| Diferencial, `hook/*` | 382 verificações, 0 diferentes |
| Testes do CLI | O revisor preso à sua unit (cada forma de recusa, a decisão `deny`, a linha no ledger, o isento), o que o deixa passar (interruptor, settings, a fence de lado com a sua linha e linha de ledger, `off` em silêncio, `relaxed` de pé, registo malformado, órfão apagado, sem registo, pedidos malformados), e o desvio do claim |
| Mutação | Matcher: 26 mutantes contra o corpus, 25 apanhados (13 só depois dos casos fixos) e 1 equivalente (o filtro de wildcard em `pattern_limits_to_current_unit`: um segmento com wildcard nunca chega lá, porque o julgamento do caminho o bloqueia antes). Hook: 13 mutantes, 12 apanhados e 1 equivalente (a lista de ferramentas: o matcher não lê nada de uma ferramenta fora dela) |
| `cargo test` | 473 testes. Diferencial completo: 2 748 verificações, 2 562 iguais, 186 desvios, 0 diferentes; 2 694 gravadas sobre 793 árvores (o registo órfão fica ao vivo: a sua idade é a hora de modificação, que uma árvore gravada não leva) |

**Desvios deliberados.** Com teste marcado, `DEVIATIONS` e esta linha.

| | Original | Focus |
| --- | --- | --- |
| Um workflow de equipa com um claim de unit (`.focus-unit-scope.json`) | Segura as escritas deste checkout à unit que reclamou (`REVIEWER_SCOPE_BLOCKED` com o stage `claimed-checkout`) | Recusa pelo nome (units) |
| O marcador de diretiva ao lado do registo (sem despacho, um agente de revisão a ler `construction/`) | Lê-o para saber se uma revisão por unit era devida, e deixa uma advertência | Recusa pelo nome (M5) |

### Fatia 4c: `state-transition-guard`

O condutor não chama as transições da ferramenta de estado diretamente (`set`, `checkbox`, `advance`, `finalize`, `complete-workflow`, `gate-start`, `approve`, `reject`, `revise`, `skip`, `park`, `refresh-unit-progress`, `fold-unit-merge`): o engine é dono do pin da stage, das verificações de evidência e da escolha da transição, e a ferramenta de estado recusa por si (secção 14, fatia 2) com as mesmas palavras. Um agente delegado não muda o estado nem o routing de todo: nem pelas escritas da ferramenta de estado, nem pelos verbos do orquestrador, do jump, das units, dos bolts, do swarm, do log, da auditoria, do worktree, da postura de testes ou da utilidade, nem pelo dispatcher, nem por `machine-config`. O hook falha fechado onde não consegue inspecionar: um executável que é uma variável de duas palavras, um `sh -c` ou `eval` com `$`, um `env -S` com expansão, mais de oito níveis de shells aninhadas.

**O que existe.** `focus-guards::state_transition`, os três leitores puros, port de `aidlc-state-transition-guard.ts` com os nomes de Focus (o mapa de nomes traduz o comando pedido ao original). `direct_state_transition`: o verbo bloqueado numa posição de comando, pelo script, pelo binário nativo ou pelo dispatcher, sobre o texto executável (separadores dentro de aspas, corpos de heredoc e corpos de função mascarados; uma substituição dentro de aspas duplas é executável). `is_lifecycle_boundary_command`: `report` e `park` do orquestrador, `execute` e `reopen` do jump, e as transições. `delegated_lifecycle_command`: segmento a segmento (comentários fora, `${}` opaco), as substituições e os heredocs expandidos primeiro, as atribuições literais que sobrevivem para os segmentos seguintes, os wrappers (`time`, `command`, `exec`, `env` com `-S` relido, `nice`, `nohup`; as opções que fazem o wrapper imprimir em vez de correr passam; as que não se inspecionam nomeiam a recusa), `eval` e `sh -c` recursivos, as opções de `bun` até ao script, `parseArgs` (uma flag toma a palavra seguinte salvo se for flag; `--` termina), o parser dos nouns de workspace (`intent`/`space` switch, create, archive; `space-create`; `--help`, `list`, os reservados e `--json` passam), as flags globais do launcher, o namespace `engine`/`system`. Onde o original indexa code units UTF-16 e mascara code points (um emoji desloca a máscara), os leitores seguem a mesma aritmética. `focus-hooks::hooks::state_transition_guard`: a fronteira de confiança do harness primeiro (`integrity::violation`, para qualquer ferramenta, sem consultar política, memória, presença ou bypass), só `Bash`, a transição direta (a fence de lado com a linha `systemMessage` e a linha `GUARD_STOOD_ASIDE` com `Tool: Bash`; uma sessão principal fora do workflow deixada à verificação da própria ferramenta; a recusa com a frase do switch quando não há agente), e o comando delegado (a recusa nomeia o agente e o comando). `focus_guards::workflow::fence_switch_sentence`, generalizado por fence e partilhado com `focus engine state`. `focus_js::re::find_all` (`matchAll`).

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `state_transition` | 2 886 casos, todos iguais ao original: 900 ao acaso (preludes, wrappers, separadores, heredocs, funções, substituições, `eval`, `sh -c`, atribuições, emojis fora do BMP), 480 dirigidos às transições diretas, 1 197 fixos (os que a mutação pediu: `\`$(…)\``, `"\$(…)"`, `-- --json`, `env -S`, os wrappers opção a opção) e 309 do varrimento das tabelas (cada entrada de cada tabela uma vez, como verbo de script e como rota do dispatcher) |
| Diferencial, `hook/state-transition-guard/*` | 43 verificações, 41 iguais, 2 desvios, 0 diferentes: a transição direta pelo script, pelo binário e pelo dispatcher (strict, memória strict, política ilegível no estado e na memória), as leituras e a prosa, `off`, `relaxed` com o switch, strict com o switch, uma conversa de fora e não ligada, só o cursor, sem estado, a frio, um subagente (por `agent_type` e por `subagent_type`, de fora, em branco), os comandos delegados (scripts, dispatcher, utilidade, aninhados, dinâmicos, permitidos, pela sessão principal, `off`, de lado com a linha, de fora, memória strict, sem shard), outra ferramenta, sem `tool_input`, pedidos malformados, a integridade (escrita num hook, no audit trail, antes da fence), os observadores |
| Diferencial, `hook/*` | 425 verificações, 0 diferentes |
| Testes do CLI | A transição direta recusada na fence (cada forma, a decisão `deny`, a frase do switch e a sua ausência para um subagente, as leituras e a prosa, outras ferramentas, pedidos malformados), a fence de lado (`off` em silêncio com a linha de ledger completa, `relaxed` com o switch a falar, memória strict a nomear o ficheiro, um observador sem linha), a política ilegível e a reparação que nomeia, a sessão principal fora do workflow (e o agente delegado de fora, mesmo com `off`; a frio), o agente delegado (17 comandos recusados com a razão exata, o `subagent_type`, o que pode correr), a fronteira do harness antes da fence (hook, audit trail, outra ferramenta), e o desvio do marcador |
| Mutação | Matcher: 56 mutantes contra o corpus, 51 apanhados (5 só depois dos casos fixos e do varrimento das tabelas) e 5 equivalentes (`command -v`: a opção falha também o teste `-p`; o salto do segundo `\|`/`&`: o segmento vazio é descartado; o separador que precede uma função mascarado ou não; a saída antecipada sem `{`; o `retain` das atribuições: a última ganha na procura). Hook: 15 mutantes contra os testes do CLI, 15 apanhados (4 só depois de afinar os testes: outra ferramenta com um campo `command`, a fence de fora com `off`, a linha de ledger completa, a recusa de integridade exata) |
| `cargo test` | 495 testes. Diferencial completo: 2 791 verificações, 2 603 iguais, 188 desvios, 0 diferentes; 2 737 gravadas sobre 801 árvores |

**Desvios deliberados.** Com teste marcado, `DEVIATIONS` e esta linha.

| | Original | Focus |
| --- | --- | --- |
| O marcador de diretiva ao lado do registo | Lê-o na decisão da fence (a autoridade da ação) | Recusa pelo nome (M5) |

### Fatia 4d-i: `record-human-turn`, o turno em si

O hook de `UserPromptSubmit` (e do `PostToolUse` de uma caixa de perguntas) grava uma linha `HUMAN_TURN` por cada mensagem real da pessoa: é o sinal do gate de presença humana (secção 14, fatia 4b lê-o). O hook regista presença e ordem, não autentica quem lançou o dispatcher. No mesmo passo guarda o registo da mensagem (`focus/.focus-sessions/messages/<id>.json`: o texto como o host o entregou, as palavras da pessoa, as flags lidas; cortado a 8 000 caracteres e a 2 000 por entrada da caixa; um dia de registos, 200 no máximo), as palavras que a pessoa escreveu nesta conversa para a decisão de um gate (`<registo>/.focus-engine/gate-words/<sessão>.json`, indexadas pelo tamanho do shard depois da linha; oito mensagens de 8 000 caracteres), o marcador `human-turn` que o Stop hook compara com o avanço do engine, a linha `QUESTION_UNANSWERED` com o aviso ao agente quando a caixa volta vazia, as linhas `QUESTION_REPLIED` com o que a caixa trouxe, e a linha `HOST_TURN` para um turno que é do próprio host (a notificação de uma tarefa de fundo do Claude Code, por template ou pela sua transcript; a palavra de um adaptador). `FOCUS_UNATTENDED=1` retém a linha e os registos e deixa o marcador. Uma conversa de fora do workflow não cunha nada. Tudo fail-open: nada do que falha bloqueia o turno da pessoa.

**Decisão de âmbito.** O hook do original toca em sete famílias de M5 e da fatia 4e. Ficam para depois, recusadas pelo nome antes de qualquer escrita quando a sua evidência está lá: o switch escrito a aplicar (`applyTypedGuardSwitchPrompt`, que escreve pelo setter `config-change`, não portado: vai com ele), uma linha `/focus` com palavras depois da entrada (o parser de flags do orquestrador, `parseNextFlags`, M5), a frase de override do plan approval e os registos do plan approval (o challenge desta conversa, uma pergunta protegida de qualquer conversa: 4e), as perguntas do marcador de diretiva (o plan approval ask, o guard-recovery ask: M5), uma review pedida sem veredicto numa stage cuja classe revê (os recibos: M5) e as linhas em fila para a pessoa (`person-lines`, escritas só pelo switch). O que 4d-i grava e ninguém lê ainda, as palavras do gate, é a fatia 4d-ii: a decisão de `state approve/reject/revise` passa a lê-las e as recusas por nome de M3 levantam-se.

**O que existe.** `focus-hooks::turn`: os leitores puros do pedido (`extractResponseText` e o envelope de uma caixa, a pergunta única, as etiquetas oferecidas, as escolhas e a sua multiplicidade, as entradas que a caixa trouxe com os seus cortes, a caixa vazia), o envelope do host (Kiro Crew), as frases fixas do host, a palavra de um adaptador, e a transcript do Claude Code (a última linha `user` com este `prompt_id` e exatamente este texto, lida do fim do ficheiro). `focus-hooks::messages`: o registo da mensagem com as chaves na ordem do original, o id de oito hexadecimais, a poda. `focus-guards::typed_switch`: `parse_typed_guard_switch_request` por inteiro (as palavras simples do plan approval e da policy, uma pergunta sobre o switch, a linha `/focus` com as suas flags, o `config set`, os comandos de workspace que são donos da linha, o token ilegível nomeado de volta, as flags do trabalho novo) e `split_kiro_command_args`, os dois sobre code units UTF-16 como o original. `focus-hooks::hooks::record_human_turn`: a ordem do original (o handoff gasto, fora do workflow, a migração do campo `Change Control`, a pulsação antes do workflow, o turno do host, o registo da mensagem antes do estado, a pulsação no registo, a caixa vazia, a linha `HUMAN_TURN` com `Origin`, `Source`, `Reply`, `Picked` e `Message Id`, as palavras do gate, as linhas `QUESTION_REPLIED`, o marcador), com as recusas por nome entre o turno do host e a primeira escrita. `focus_guards::policy::set_guard_policy_line` reutilizado na migração; `focus_js::text::{len_utf16, slice_utf16}`.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `typed_switch` e `kiro_args` | 1 610 casos, todos iguais ao original: as palavras simples e as suas variantes, pontuação final e perguntas, as entradas `/focus`, `$focus` e a palavra nua, cada flag com e sem valor, contradições, `--`, o token ilegível antes e depois das palavras, `config set`, os comandos de workspace, aspas e escapes; o splitter com apóstrofos, caminhos Windows, `--output`, separadores e emojis |
| Corpus, os leitores do turno | 196 casos, todos iguais: o envelope (cabeçalhos com `--` e travessão, `\r\n`, repetidos, parciais), a frase fixa do host (inteira, com espaço à volta, cortada, repetida, outros hosts), a palavra do adaptador (cada forma válida e inválida, a razão cortada a 200), a caixa vazia (cada forma), e 50 transcripts (a linha certa e as erradas, ids, `content` como texto e como lista, ficheiros de 2 MB lidos pelo fim, 3 000 linhas, caminhos) |
| Diferencial, `hook/record-human-turn/*` | 81 verificações, 73 iguais, 8 desvios, 0 diferentes: a mensagem escrita (uma, duas, de fora, não ligada, só o cursor, a frio, sem estado, sem sessão, sessão inválida, por `user_prompt` e `message`, vazia, escalares JSON, envelope), comandos e perguntas sobre o switch, `/api/…` e `aidlc on…` como palavras, unattended (com e sem switch, com caixa), as caixas (um gate, `(Recommended)`, texto livre, uma etiqueta, `Accept as-is`, multi-select, várias perguntas, uma em branco, Codex, resposta em string, vazia de três formas, cancelada, sem `tool_input`), outro evento, pedidos malformados (com e sem registo), o turno do host (template inteiro e não, a palavra do adaptador de cada forma, sem estado, a transcript em oito formas), o corte a 8 000, o tecto das palavras do gate (e um registo anterior do mesmo e de outro shard), a poda dos registos, o campo `Change Control` (relaxed, strict, por caixa), o handoff, sem shard, os observadores |
| Diferencial, `hook/*` | 506 verificações, 0 diferentes |
| Testes do CLI | O turno escrito e as suas palavras (a linha, o registo, as palavras do gate com o tecto, o marcador, a pulsação), comandos e perguntas marcados como não-resposta, a caixa (um gate, uma etiqueta, texto livre, vazia com o aviso), o turno do host (template com transcript, adaptador, transcript, outro `prompt_id`), unattended, de fora e a frio e malformado, o corte e a poda, o campo retirado (relaxed renomeado, strict não), e o desvio do que fica para depois |
| Mutação | Parser do switch: 16 mutantes contra o corpus, 14 apanhados (2 só depois de casos fixos: `--space` duas vezes, `--plan-approval off` depois `on`) e 2 equivalentes (`config set` com menos de quatro tokens, que o laço já devolve vazio; o ramo `guard.plan-approval` das fences do trabalho novo, que `setting_name` torna inalcançável, como no original). Leitores do turno: 15 mutantes, 14 apanhados (2 só depois de um caso: um transcript `.txt` que existe; o último cabeçalho do envelope) e 1 equivalente (o texto vazio antes das templates). Hook: 23 mutantes contra os testes do CLI, 22 apanhados (4 só depois de afinar os testes: `tool_name` num pedido de prompt, a template sem transcript, 8 000 exatos, o handoff) e 1 equivalente (`state_exists` no ramo do host: sem estado a conversa fica de fora antes) |
| `cargo test` | 516 testes. Diferencial completo: 2 871 verificações, 2 675 iguais, 196 desvios, 0 diferentes; 2 817 gravadas sobre 821 árvores |

**Desvios deliberados.** Com teste marcado, `DEVIATIONS` (ou `SHAPES`) e esta linha.

| | Original | Focus |
| --- | --- | --- |
| Um switch escrito (as palavras simples, ou `/focus` com uma flag de setting) | Aplica-o ao trabalho aberto pelo setter, regista a mensagem com as linhas que disse e põe-nas em fila para a pessoa | Recusa o turno pelo nome (vai com `config-change`) |
| Uma linha `/focus` com palavras depois da entrada | Lê-a com o parser de flags do orquestrador (M5) para o registo e a resposta | Recusa o turno pelo nome |
| A frase de override do plan approval | Regista o pedido de override para o `answer` emparelhar (4e) | Recusa o turno pelo nome |
| O challenge desta conversa, uma pergunta protegida de qualquer conversa, o marcador de diretiva, uma review sem veredicto numa stage que revê, as linhas em fila para a pessoa | Responde, nota ou transporta, e segue | Recusa o turno pelo nome |
| Os offsets do registo das palavras do gate (`SHAPES`: `gate-words-offset`) | Indexam o shard cujo cabeçalho é `# AI-DLC Audit Log` | Indexam o shard cujo cabeçalho é `# Focus Audit Log`, um byte mais curto: a única frase do mapa de nomes cujo comprimento muda. Os offsets do original leem-se um byte atrás antes de comparar |

**Um risco do nome.** A entrada nua do comando escrito (`aidlc …` sem barra) passa a `focus …`: uma frase da pessoa que comece pela palavra inglesa "focus" ("focus on the login page") lê-se como uma linha de comando sem flags (palavras, nenhum switch, nenhum efeito hoje). O corpus não a pode comparar (o original só conhece `aidlc`); a fatia 4d-ii decidiu mantê-la (ver lá).

### Fatia 4d-ii: as palavras do gate lidas

A decisão de um gate lê o que o hook de turno gravou para esta conversa (`<registo>/.focus-engine/gate-words/<sessão>.json`). `focus-lifecycle::presence` ganha os leitores do original: o registo validado (versão 1, a sessão que invoca, o shard deste clone pelo caminho relativo, `dropped` e cada `offset` inteiros seguros, o ficheiro até 256 KiB), `gate_words_since_presentation` (as mensagens com offset depois da última apresentação da stage, `gate_presentation_start` sem a primeira apresentação; nenhuma se uma mensagem depois dela não foi guardada), `persons_latest_gate_pick` (a última mensagem que não é cancelamento, quando é exatamente uma opção: `Approve`, `Request Changes`, e `Accept as-is` só a partir da terceira revisão), `persons_gate_words` (todas, para o recibo) e `persons_gate_feedback` (sem a que é só uma escolha). Em `state approve`: uma escolha exata de `Request Changes` recusa a aprovação "para o agente" (`personDecided`), e a escolha nomeia a aprovação (`User Input`) em vez do que o condutor escreveu; `Person Reply` vai na linha `GATE_APPROVED` (as palavras do gate que cobre, numa aprovação coberta). Em `state reject`: a escolha de `Approve` ou `Accept as-is` recusa; as palavras da pessoa são o `Feedback`, o texto do condutor fica em `Conductor Summary` quando difere depois de normalizar espaços, e o JSON leva `feedback` e `feedback_source: person`. As recusas por nome de M3 (fatia 4b) levantam-se: `refuse_gate_words` e o desvio `state/gates/gate_words` saem. `revise` não lê as palavras no original; a sessão lê-se da seleção, que já a resolveu do ambiente (nenhum comando de gate nomeia uma sessão).

**O switch escrito aplicado** (`applyTypedGuardSwitchPrompt`) fica com o setter `config-change` (M5): não é desta fatia.

**A entrada nua, decidida.** Mantém-se, como `aidlc …` no original: uma frase que comece pela palavra inglesa "focus" lê-se no parser como uma linha de comando com palavras e sem flags, o que não é um switch, não é uma pergunta sobre o switch, não é um prompt de comando (`is_command_prompt` exige `/` ou `$`) e não é uma linha `/focus` com palavras (a recusa por nome de 4d-i só vale para a entrada com barra). O hook grava-a como as palavras da pessoa, como qualquer frase. O único efeito que resta é o pretendido: `focus --guard-policy relaxed` sem barra é um switch, como `aidlc --guard-policy relaxed` lá. Teste unitário em `typed_switch`.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `state/gates/gate_words_*` | 32 verificações, todas iguais: registo malformado; aprovar com palavras, com a escolha `1`, com `2` e com `Request Changes (Recommended)` (recusadas), cancelada, aprovada e depois cancelada; `Accept as-is` escolhido com a revisão no tecto e escrito sem ela; rejeitar com as palavras da pessoa, com o mesmo texto noutro espaçamento, só a escolha com e sem `--feedback`, palavras sem `--feedback`, a escolha de `Approve` e de `Accept as-is` (recusadas) e `Accept as-is` antes de ser oferecido; `dropped` depois e antes da apresentação; uma mensagem antes da apresentação; outro shard, outra sessão, sem sessão; depois da decisão; depois de uma pergunta respondida; outra stage; versão errada; offset fracionário; o gate reapresentado (palavras entre as duas apresentações, e depois da segunda); uma stage coberta pela aprovação de outra (a escolha na sua própria apresentação, e as palavras que não vão no recibo) |
| Diferencial, `state/gates/*` | 176 verificações, 164 iguais, 12 desvios, 0 diferentes |
| Testes unitários (`presence`) | Os leitores sobre um registo real: a ordem, a escolha a partir da última mensagem, o cancelamento ignorado, `Accept as-is` só quando oferecido, a mensagem no offset da apresentação, o gate reapresentado; e o que não se lê: `dropped` depois da apresentação, outro shard, outra sessão, outra versão, sem mensagens, cada offset e `dropped` inválido (fração, negativo, texto, acima do inteiro seguro), texto que não é texto, JSON partido, uma lista, o ficheiro acima do tecto, sem sessão |
| Teste do CLI | A escolha de `Request Changes` recusa a aprovação; as palavras são o feedback com o `Conductor Summary` e o JSON; depois da revisão a escolha de `Approve` recusa a rejeição e vai como `Person Reply`; os registos que não se leem; `Accept as-is` escrito sem revisão (palavras, não escolha) e escolhido com três (recusa a rejeição, nomeia a aprovação) |
| Mutação | 34 mutantes nos leitores e na integração do gate contra os testes unitários e do CLI: 33 apanhados (25 pelos testes como estavam; 1 por um teste unitário novo, o gate reapresentado; 7 só pelo diferencial, 3 deles depois de verificações novas: `Accept as-is` escolhido e escrito sem ser oferecido, as palavras de uma stage coberta) e 1 equivalente, retirado do código (a sessão lida do override do invocador depois da seleção, que já a resolveu dele) |
| `cargo test` | 519 testes. Diferencial completo: 2 902 verificações, 2 707 iguais, 195 desvios, 0 diferentes; 2 848 gravadas sobre 849 árvores |

**Desvios deliberados.** Nenhum novo; um levantado (`state/gates/gate_words`). A forma `gate-words-offset` (4d-i) continua a corrigir os offsets do original antes de comparar.

### Fatia 4e-i: os leitores do Plan Approval

**Decisão de âmbito.** O `plan-approval-guard` do original (2 793 linhas) decide quase tudo a partir do marcador de diretiva ativa (`active-directive.json`, o passo que `next` emitiu: `run-stage`, `invoke-swarm`, o `ask` de plan approval ou de recuperação) e da avaliação da aprovação em `testing-posture` (`evaluateCodeGenerationApproval`: o fingerprint do plano e das instruções, o recibo da aprovação, a origem do código, as worktrees de um swarm). Os dois são de M5, como a secção 14 já dizia do marcador. A fatia 4e divide-se por isso em duas: **4e-i**, os leitores puros que o corpus compara função a função (esta), e **4e-ii**, o hook com o que decide sem marcador e os seus leitores privados de um comando de shell, comparado pelo diferencial; um marcador presente recusa pelo nome até M5, que traz o marcador, a avaliação da aprovação e `plan-approval-ask`.

**O que existe.** `focus-guards::guard_operation` (`aidlc-guard-operation.ts` inteiro): as seis operações de recuperação lidas de JSON campo a campo, o comando do engine de cada uma com a sua grafia de source tree quando difere (o switch de fence por `utility config-change`), a renderização em modo nativo ou source para POSIX ou PowerShell com `quote_command_argument` (de `aidlc-runtime-paths.ts`), a correspondência exata de um comando escrito e de um `engine <route> <args>`, a correspondência com o remédio de um guard, `same`, o parser da continuação de um restart (`focus engine jump execute --target … --direction … --scope …`), a admissão do abort e do switch de fence inteiramente especificados, e a palavra de cada remédio para a pessoa (`GUARD_REMEDY_WORDING`). `focus-guards::plan_approval` (a secção "The pure decision" do hook): os marcadores `FOCUS-UNIT` e `FOCUS-STAGE` de um handoff, a decisão sobre um dispatch do developer dada a evidência por alvo (um só alvo, conhecido, aprovado, com a sua linha de contrato, sem o apêndice da review citado), e as recusas `blockReason`, `handoffBlockReason`, `appendixBlockReason`, `mutationBlockReason` e `receiptDetail`, com o `brief` que cada uma nomeia. `focus-guards::posture` (de `aidlc-testing-posture.ts`): as linhas `FOCUS-TESTING-CONTRACT`, o `latestPlanApproval` do ficheiro de perguntas sobre as linhas visíveis de `focus-markdown` (a secção aberta pela última heading com o rótulo, com prefixo de pergunta ou numerada com o rótulo na linha seguinte; fechada por uma heading ao mesmo nível ou por uma pergunta numerada; uma subsecção só preenche o que falta), `questions_file_approved` e `questions_file_has_pending_plan_approval`, e o apêndice `## Review` terminal de um plano (`terminalReviewAppendixBoundary`: a linha exata na coluna 0, heading de nível 2 pela estrutura do documento, sem h1 ou h2 depois dela em Markdown nem num bloco HTML; o corte com o espaço separador reduzido a um fim de linha). O original confirma a heading renderizando o documento; Focus lê a estrutura de `focus-markdown` e o nível de cada heading da própria linha (ATX pelas marcas; uma setext é sempre h1 ou h2).

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `guard_operation` | 2 280 casos, todos iguais: cada operação válida e inválida (campo a mais ou a menos, identificador inválido, fence não switchable, não-objeto), a invocação, a renderização nos dois modos e nas duas shells com diretórios de harness válidos e inválidos, comandos que correspondem e que diferem por um espaço, um wrapper, uma flag, maiúsculas ou aspas, o remédio de cada operação e os outros, `same` com lados ausentes e malformados, a continuação em cada grafia e cada defeito, os argumentos do engine que admitem e os que não, e a palavra de cada remédio com e sem Unit, fence e custo de reset |
| Corpus, `plan_dispatch` e `plan_reason` | 480 + 750 casos, todos iguais: ferramentas e agentes guardados e não, a stage em cada grafia, zero, um e vários marcadores, alvos conhecidos e desconhecidos, evidência completa e com cada campo em falta, a linha de contrato certa, errada, repetida e ausente, o apêndice citado inteiro, dobrado e só pela heading; as cinco recusas com cada forma de `mentioned`, `detail` (vazio, com pontos finais) e `targets` |
| Corpus, `contract_markers`, `questions_file`, `review_appendix` | 180 + 378 + 379 casos, todos iguais: as linhas de contrato com espaços, maiúsculas e hashes curtos; ficheiros de perguntas com cada forma de heading do rótulo e de pergunta numerada, cada resposta aprovada e pendente, as tags nos três formatos, subsecções, fences, comentários e blocos HTML, mais 18 escritos à mão para as arestas da secção; planos com o apêndice terminal e não terminal, headings setext, h1 e h2 em HTML, em comentários e em elementos de texto cru, `## Review` dentro de fence, quote, lista e bloco HTML, separadores `\r\n` e linhas em branco com espaços, mais 19 escritos à mão. Os dois últimos são leitores de Markdown; comparados por igualdade, sem nenhuma diferença a julgar |
| Testes unitários | `guard_operation` (4), `plan_approval` (3), `posture` (3) |
| Mutação | 58 mutantes nos três módulos: 55 apanhados pelo corpus, 2 pelos testes unitários (a plica na citação de PowerShell; as duas shells em `matches_command`, com um teste novo para uma operação construída em código) e 1 equivalente (o comprimento do abort em `is_guard_recovery_engine_invocation`, que só guarda as duas palavras lidas: a comparação exata a seguir é que recusa). Três equivalentes da primeira versão saíram por simplificação do código (o nível exato de uma heading setext, o nível 2 de uma candidata, a bandeira de "viu h1 ou h2") |
| `cargo test` | 529 testes; corpus de 24 275 casos (4 447 novos), todos iguais |

**O Bun do corpus.** O corpus foi regravado com o Bun 1.4.4 (antes 1.4.2). O parser Markdown do Bun mudou entre as duas versões, e o original lê por ele: 8 documentos da família `markdown` têm respostas diferentes das gravadas antes (`<!x y>` passa a ser um bloco HTML, `> [Answer]:` seguido de `> ---` uma heading setext, e semelhantes). Em todos o original aproximou-se do que Focus já lia, e o teste que julga essas linhas passa sem alteração. O resto do corpus (19 820 casos anteriores) é igual byte a byte. As árvores gravadas do diferencial (`fixtures/recorded`) continuam as de 1.4.2: esta fatia não lhes acrescenta nada.

**Desvios deliberados.** Nenhum. A grafia de source tree (`bun .claude/tools/focus-<route>.ts`) continua a ser aceite por `matches_command` e pela continuação, como no original: Focus nunca é uma instalação de source tree, e um comando escrito nessa grafia não corre; aceitá-lo não muda nada.

### Fatia 4e-ii-a: o que corre enquanto um plano espera

**Decisão de âmbito.** O hook `plan-approval-guard` sem marcador de diretiva decide só uma coisa: se a chamada é das que o plano nunca governa (uma leitura, uma escrita fora do projeto, três ficheiros que passam em qualquer estado) ou se espera pela diretiva. Quase todo o seu código é a leitura da chamada para essa pergunta (`mutationIntent` e o que ele chama, cerca de 1 100 linhas privadas do hook). A fatia 4e-ii divide-se em três: **a**, o que é puro (esta); **b**, `mutationIntent` com o que depende do projeto; **c**, o hook e o diferencial.

**Os privados do hook, perguntados ao original.** As funções desta fatia não são exportadas pelo original. O driver do oráculo escreve ao lado do hook uma cópia do seu próprio source com uma lista de exportação acrescentada (`aidlc-plan-approval-guard.focus-oracle.ts`, removida quando o driver termina) e importa-a: as funções são as do original, sem alteração, e o corpus compara-as uma a uma. É a regra "o original é perguntado nos seus próprios nomes" levada aos nomes privados, e vale para 4e-ii-b.

**O que existe.** `focus-guards::dispatcher`: `resolve_engine_action`, o `resolveAction` do dispatcher (`aidlc.ts`) para uma linha que começa por `engine`: as rotas do namespace como dados (29 nomes que delegam), os aliases (`--status`, `--resume`, `__sensor-script`), os nomes com parser próprio (`intent` e `space` pelo parser de workspace, `config`, `plugin`, `gen`), as flags globais retiradas antes de `--` e entregues ao delegado, `--project-dir` na forma de dois tokens (uma só vez, com valor), e o projeto do dispatcher fixado antes de uma cauda literal. Só a delegação se resolve: o que o dispatcher responde por si (ajuda, um hook, a status line, um adapter, um erro) não resolve para nada. A linha de comandos de Focus não passa por esta tabela. `focus-guards::plan_wait`: `every_helper_may_run` (a rota julgada pelo script e verbo que o dispatcher corre, contra `DELEGATE_ADMITTED_VERBS` e numa grafia que o guard de transição não recusa a um helper), `is_plan_approval_prerequisite` (o transporte de `next` e `continue`, parar e voltar, a resposta ao gate aberto, as leituras, o checkpoint de um bolt sem verificação, os recibos de Unit, as escolhas de entrada em Construction, plan approval ligado ou desligado quando a pessoa falou, a stance do esqueleto, as respostas do plan approval e da recuperação), `engine_directed_while_plan_waits` (as rotas que o engine nomeia, as que exigem que a pessoa tenha pedido), `intent_repo_change_admitted`, `is_read_only_diagnostic` e `is_read_only_tool_diagnostic`, `plain_power_shell` e `power_shell_reading` (uma linha PowerShell que se lê tal como está escrita e a sua leitura POSIX fiel), `shell_uses_dynamic_evaluation`.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `engine_route` e `every_helper` | 1 201 + 1 228 casos, todos iguais: cada rota do namespace com cada verbo, só e com uma cauda (flags globais, `--project-dir` em cada forma válida e inválida, uma cauda literal depois de `--`, um argumento vazio); cada script de ferramenta pelo seu stem com cada verbo admitido e alguns que não; as primeiras palavras que resolvem sós, os aliases e os scripts de sensor; o parser de workspace com cada verbo, nome e flag; cada chave de `config set`; `plugin` e `gen`; e linhas ao acaso sobre os mesmos nomes |
| Corpus, `plan_prerequisite`, `engine_directed`, `read_only_diagnostic` | 617 + 498 + 208 casos, todos iguais: cada pré-requisito com os seus flags em cada valor, repetidos, sem valor e com um flag por valor; o gate aberto e fechado; a pessoa ter falado e não; as invocações de recuperação; cada rota do namespace como controlo; as rotas dirigidas pelo engine com os flags permitidos, um a mais, na forma `--flag=valor` e sem valor; a mudança de repo nos dois prefixos, com e sem a pessoa; os diagnósticos por cada cabeça e stem |
| Corpus, `plain_powershell` e `dynamic_evaluation` | 759 + 240 casos, todos iguais: linhas PowerShell simples e não (variáveis, subexpressões, splatting, comentários, `-x.y`, aspas tipográficas, tabs, palavras vazias, barras finais, o operador `&`, redirects que não escrevem e que escrevem, junções `;`, pipe, `||` e `&&`), com a leitura POSIX fiel e a que um wrapper ou uma atribuição desfaz; linhas de shell com `$`, backticks e substituições dentro e fora de cada tipo de aspas |
| Testes unitários | `dispatcher` (3), `plan_wait` (4) |
| Mutação | 79 mutantes nos dois módulos: 67 apanhados pelo corpus (59 à primeira; 8 depois de casos escritos à mão para as arestas: `space list --all`, `--help` como nome de um registo, `--project-dir` vazio e depois de `--`, o último valor de um flag repetido, um resultado `rejected` no gate aberto, o verbo de um script depois de `--project-dir`), 2 pelos testes unitários (o nome normalizado de um comando e a rota de `park`, que só a fatia seguinte usa), 6 equivalentes retirados do código (o erro de um verbo de workspace que nenhum verbo alcança, `__sensor-script-file`, `--version`, a ajuda e os nomes `hook` e `top`, que já não resolviam para nada, e `\|\|` numa linha PowerShell, que a segunda barra já recusa), 1 equivalente anotado (saltar o valor de um flag, que nunca é o próprio flag) e 3 redundantes entre si (os launchers, o executável e os argumentos na leitura fiel: cada um denuncia sozinho um wrapper em todas as linhas sondadas no original, e o original pergunta os três) |
| `cargo test` | 536 testes; corpus de 29 026 casos (4 751 novos), todos iguais; os 24 275 anteriores iguais byte a byte |

**Desvios deliberados.** Nenhum. Um `--project-dir` relativo resolve-se contra o diretório de trabalho do dispatcher, que é o do host: o corpus não o pergunta, e os testes unitários cobrem-no sobre `focus_js::path::resolve`, que o corpus já segura.

### Fatia 4e-ii-b: a intenção de uma chamada

**Decisão de âmbito.** `mutationIntent` responde a três perguntas sobre uma chamada de ferramenta: que ficheiros diz que escreve, se algum comando da linha pode mudar o que o lexer não vê (a shell *opaca*), e se o lexer leu a linha inteira (*legível*). O hook pergunta depois, de cada alvo, onde fica: fora do projeto, a proposta do composer, o texto de uma resposta, o diário da stage. Tudo isto é desta fatia; o hook que decide com as respostas é a seguinte.

O que depende do projeto entra por dois valores. `Project` leva as pastas, resolvidas por quem selecionou o trabalho (a pasta do projeto e a do harness, a raiz dos registos, o registo selecionado, a proposta do composer) e o host em que os caminhos se leem. `PlanWait` junta-lhe o gate aberto e três leituras do ledger sobre a pessoa (falou desde a última decisão, pediu o que o agente corre agora, o último passo admite o que ela pede), passadas como funções: `focus-guards` não lê o ledger, que é de `focus-lifecycle`, e este depende daquele. As admissões que o original lê do marcador de diretiva ativa (as respostas da pergunta de recuperação, a continuação de um restart escolhido, as units de um swarm) ficam ausentes, como no original enquanto não há marcador em disco: são de M5.

**A pessoa, no corpus.** Uma árvore de scratch pode levar um registo com um turno humano no ledger. O que o original lê da pessoa nessa árvore faz parte da sua resposta (`person`), o caso diz o mesmo, e a réplica de Focus recebe o que o caso diz: uma árvore mal escrita aparece como diferença, e os ramos que só abrem quando a pessoa falou (desligar um switch gravado, desligar um check pelo chat, os movimentos que o engine nomeia) são comparados com o original, com e sem ela, com um driver sem ninguém atrás (`UNATTENDED`) e com o trabalho parado. Focus calcular estas três leituras a partir do ledger é do hook, e fica com o diferencial de 4e-ii-c.

**O que existe.** `focus-guards::mutation_intent`: `PlanWait::mutation_intent` (os alvos de uma linha de shell lidos pelo lexer de 4b, com a leitura PowerShell quando o adapter a anuncia e a linha é simples; os caminhos de uma ferramenta de ficheiros; `opaque_shell`, `runs_engine`, `readable`), `PlanWait::admits` (o `planWaitAdmits` do original: os pré-requisitos, os diagnósticos, o switch gravado de `config flags`, o switch de chat de `engine config set`, a mudança de repo, o que qualquer helper pode correr e o que o engine nomeia), a necessidade de aprovação de cada comando (`cd` de volta ao diretório atual, os cmdlets só de leitura, `sort` e `uniq`, os comandos só de leitura, os subcomandos de `git`, o engine pelo nome ou pelo launcher `.cmd`, um script instalado do harness corrido por Bun e julgado pela rota que implementa, os mutadores com alvos concretos), `code_generation_gate_held`, e em `Project` os predicados de alvo: `is_outside`, `is_stage_record_output`, `is_answer_text_target`, `is_stage_diary_target`, `is_composer_proposal_target`. Em `focus-workspace`: `links::chain_is_link_free` (o `assertNoSymlinkInChainOrThrow` do original feito pergunta, com o caminho lido como ele o lê: as duas famílias de barras em qualquer host, uma raiz ou uma drive recusadas, `..` recusado), `files::same_directory` (a identidade de uma pasta, por inode e device ou pelo que o Windows guarda), `files::absent_or_plain_file` e `files::hard_links`, e `project::normalize_drive_letter`, que sai de `focus-hooks` para onde os dois crates o alcançam.

**O que o corpus encontrou.**

| Achado | Consequência |
| --- | --- |
| `lstat` com `throwIfNoEntry: false` em Bun só devolve nada para um nome que não existe: um caminho que atravessa um ficheiro (`a.txt/x`) lança, onde em Node devolveria nada | O original corre em Bun: o alvo não está "fora do projeto" nem é "ausente", e Focus lê o erro da mesma maneira |
| Uma linha dinâmica "pode correr o engine" quando o nome do engine aparece nela em qualquer sítio, incluindo no caminho da raiz de scratch | A raiz das árvores destes casos não leva o nome de nenhum dos dois engines |
| `\` seguido de fim de linha é, para o lexer, um fim de linha escapado dentro da palavra, não uma continuação | A recusa do original para `cd` com continuação só se distingue numa pasta cujo nome acaba em fim de linha: o corpus tem uma |
| `bun .. topo`, corrido de dentro de um caminho por baixo de um script instalado, resolve para o script e é admitido | Focus responde o mesmo: é o script admitido, por outro caminho |
| Em macOS `/tmp` é um link e o sistema de ficheiros não distingue maiúsculas: um projeto por baixo de `/tmp` tem um caminho real diferente do escrito desde a raiz, e um alvo que difere de um ficheiro só nas maiúsculas é esse ficheiro (a primeira corrida de CI em macOS, 8 árvores) | A réplica monta a árvore por baixo do `/tmp` real do host e diz os caminhos como escritos; nenhum alvo do corpus difere de um ficheiro da árvore só nas maiúsculas |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `mutation_intent` | 2 542 casos, todos iguais: cada comando de leitura, de `git`, de `cd`, do engine pelo nome (cada pré-requisito, cada switch de `engine config set` em cada valor, `config flags` com cada par, camada e `--project-dir` em cada grafia), de um script instalado corrido por Bun (direto, por `run`, com `-r` e `--preload` antes e depois, por um link, em falta, numa subpasta, com outro Bun, por um wrapper), de escrita e de avaliação dinâmica, sozinhos e em linhas de dois e três comandos, de cada diretório de trabalho; as mesmas linhas com a pessoa no registo, com um driver sem ninguém atrás e com o trabalho parado; a leitura PowerShell com e sem o anúncio do adapter; cada ferramenta de ficheiros com cada campo; e árvores com a pasta do harness atrás de um link, sem ferramentas, com pastas no lugar das ferramentas e com o projeto nomeado por um link. 1 550 linhas opacas, 914 lidas por inteiro e livres, 1 485 que correm o engine, 479 que o lexer não lê, 981 com a pessoa no registo |
| Corpus, `mutation_target` | 20 árvores, 1 316 alvos julgados pelos cinco predicados, todos iguais: com e sem registo selecionado, com dois registos e sem cursor, com links de fora para dentro e de dentro para fora, um link pendente, um segundo nome para o ficheiro, um link no lugar do ficheiro e a caminho dele, um ficheiro onde se espera uma pasta, o projeto nomeado por um link, e sem pasta de registos |
| Testes unitários | `mutation_intent` (4: o launcher `.cmd` num host Windows, outra drive não está dentro de uma pasta, e os dois desvios), `links` (a cadeia lida como está escrita), `files` (a identidade de uma pasta e um ficheiro com um só nome), `project` (a letra da drive) |
| Mutação | 221 mutantes no código final (`mutation_intent`, e em `focus-workspace` a cadeia de links, a identidade e o ficheiro simples): 195 apanhados pelo corpus (na primeira passagem, 162 de 233; os outros depois de casos escritos à mão para as arestas: outra stage à espera de aprovação, `git -o`, `config flags` e `config set` atrás de outra primeira palavra, `-r` e `--preload=` depois do script, um script que começa por `-`, um script relativo a outro diretório de trabalho, nomes ao lado do de uma ferramenta, um cmdlet com caminho, `cd.exe`, `cd` relativo que resolve para o diretório atual, pastas com `[`, `{`, `*` e `?` no nome, um diretório de trabalho que não existe e um que é um ficheiro, a mesma pasta por um link, um alvo concreto ao lado de um glob e de um `$` entre plicas, a leitura PowerShell de um caminho com `\`), 11 pelos testes unitários (o que só um host Windows alcança, e a cadeia de links e o ficheiro simples lidos diretamente), e 15 equivalentes anotados: a raiz do sistema de ficheiros existe sempre; o sufixo que não existe não muda de que lado o alvo fica; o caminho real de um alvo nunca fica dentro do caminho escrito de um projeto que é um link; `relative` só devolve um caminho absoluto entre drives; um link a caminho do registo está a caminho do alvo que está dentro dele; a pasta do script é a das ferramentas sempre que a cadeia está limpa; um link no lugar do script já foi recusado pela cadeia; a resolução do executável alterada já torna a linha dinâmica, e um argumento vindo de dados vem com um launcher; um stem vazio não é admitido por nenhuma rota; numa leitura PowerShell fiel não há wrappers nem espaço em branco que o lexer não descodifique (dois); o lexer já tira `.cmd` do nome; e duas pastas com a mesma identidade são as duas pastas (dois, que em Windows não são equivalentes). 11 equivalentes saíram do código antes (um deles voltou em 4e-ii-c, ver lá): `rel === ""` em `isWithinDir`, as duas exclusões de plan approval no switch de chat (já admitido nos pré-requisitos com os mesmos valores), o nome normalizado de Bun antes do nome exato, o script vazio e a pasta do script perguntados duas vezes, os diagnósticos só de leitura depois do que a espera admite, o engine atrás de um caminho (que em Focus nenhum caminho nomeia), o dialeto antes dos nomes PowerShell de `cd`, o alvo vazio antes do alvo não absoluto, e `.` como componente de uma cadeia |
| `cargo test` | 543 testes; corpus de 31 588 casos (2 562 novos), todos iguais; os 29 026 anteriores iguais byte a byte |

**Desvios deliberados.** Dois, cada um com um teste marcado em `mutation_intent`; a entrada em `DEVIATIONS` vem com as verificações do hook em 4e-ii-c, porque antes dele não há comando que o diferencial compare.

| Desvio | Original | Focus |
| --- | --- | --- |
| O Bun que corre o hook | Um script instalado é admitido quando corrido pelo Bun que corre o hook, nomeado pelo seu caminho absoluto (`isThisBun`, por identidade de ficheiro) | O hook de Focus não é corrido por um Bun: Bun é admitido só pelo nome (`bun`, `bun.exe`) |
| O engine instalado, por caminho | Onde um caminho nomeia o engine (uma linha PowerShell, um host Windows), é admitido o caminho absoluto do launcher instalado ou do executável ativo (`installedEngine`) | O layout da instalação é da distribuição (M8) e não está portado: um caminho é recusado, e o engine é admitido pelo nome (`focus`, `focus.exe`, e `focus.cmd` nesses mesmos sítios) |

Os dois recusam o que o original admitiria: a chamada espera pela diretiva em vez de passar. Nenhum admite o que o original recusa.

### Fatia 4e-ii-c: `plan-approval-guard`, o hook

**Decisão de âmbito.** Sem marcador de diretiva, o hook do original decide em três passos: a chamada é das que o plano nunca governa e passa; ou chega à decisão que se lê do último passo do engine, e não havendo passo é recusada com o `next` que o emite; ou a avaliação falha e é recusada fechada. É isto que Focus faz. Um marcador em disco (`active-directive.json`) recusa pelo nome, só quando a chamada chegaria a lê-lo: uma leitura, ou o despacho de outro agente, passam antes, como no original. Uma fence baixada (a Guard Policy, ou o switch `guard.plan-approval off`) não fornece o passo que falta: muda as palavras da recusa e não deixa linha no ledger.

**O que existe.** `focus-hooks::hooks::plan_approval_guard`, pela ordem do original: o payload; a entrada no workflow; a integridade do runtime, que não é fence e nenhum interruptor desliga; a conversa que não entrou no trabalho selecionado (as suas escritas passam, o despacho do developer desse trabalho não); o heartbeat `plan-approval-guard.last`, antes do interruptor; `FOCUS_DISABLE_PLAN_APPROVAL_GUARD=1`, do ambiente ou das settings, com uma linha `GUARD_DISABLED` na primeira chamada que passa por baixo dele e nenhuma nas seguintes até outra linha chegar ao shard; as ferramentas só de leitura; a stage (a corrente é Code Generation, ou o despacho do developer leva um marcador de unit, de stage ou de contrato de testes); a intenção da chamada (4e-ii-b), com as três leituras da pessoa calculadas do ledger (`person_spoke_since_gate` com `requests`; a mesma com `outlives_approval` e sem `FOCUS_UNATTENDED=1`; o trabalho parado); o que passa em qualquer estado; e a recusa. `focus-guards::plan_approval` ganha `authority_remedy` e `authority_block_reason`, as palavras da recusa sem aprovação de que falar.

**O que o diferencial encontrou.**

| Achado | Consequência |
| --- | --- |
| Uma pessoa pode ter pedido sem ter falado desde a última decisão: a mensagem que pediu também aprovou um gate. `personAskedSinceGate` conta-a, `personSpokeSinceGate` não. A exclusão de `plan-approval` no switch de chat, que a fatia 4e-ii-b tinha retirado como equivalente, não o é: sem ela, `engine config set plan-approval off` passava nesse estado | A exclusão voltou. O corpus não o tinha visto porque nenhuma das suas árvores punha a pessoa nesse estado: tem agora 102 casos com um turno gasto pela sua própria aprovação, e apanha a mutação. A exclusão de `guard.plan-approval` é que é equivalente (esse comando é uma invocação de recuperação, admitida com ou sem a pessoa) e fica anotada |
| Uma linha que não se consegue escrever (modo observador) não é uma linha largada: o original só regista a queda quando não consegue o lock | Focus distingue o lock da escrita |
| O hook `review-freeze` lia a chave `aidlc_shell` do payload, o nome do original | Lê `focus_shell`, como a integridade do runtime já lia; uma verificação do diferencial e um teste passam por lá |
| Com o Bun 1.4.4, o renderer de Markdown por onde o original lê deixou de divergir em quatro dos desvios `markdown/commonmark` de M2 (o título setext que parece um rótulo, a declaração em minúsculas, o destino com `<`, a etiqueta indentada sob um parágrafo), e o leitor de shards deixou de tomar por separador uma barra invertida no caminho do projeto | Os cinco testes deixaram de estar marcados como desvio, as tabelas de M2 dizem desde quando, e `verbs/memory_lowercase_declaration` e `runtime/compile/project_path_with_a_backslash` saíram de `DEVIATIONS`: as suas verificações comparam igual. Cada entrada de `DEVIATIONS` foi confrontada com a corrida completa: todas as outras ainda têm uma verificação que desvia (as duas de fuzz só correm com `--markdown`) |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `hook/plan-approval-guard/*` | 77 verificações, 74 iguais, 3 desvios, 0 diferentes: a escrita de source por cada ferramenta de ficheiros, sob cada Guard Policy, com o switch da fence, com plan approval desligado, com a policy fixada pela memória da equipa e com uma policy ilegível; escritas fora do projeto, dentro e fora ao mesmo tempo, sem alvo; o diário, o texto de uma resposta e a proposta do composer, e os ficheiros do registo ao lado deles; o diário atrás de um link; as ferramentas de leitura, uma ferramenta desconhecida, nenhuma ferramenta; o despacho do developer e o de outro agente, noutra stage com e sem marcadores; a stage escrita de outra maneira; leituras, escritas e linhas ilegíveis de shell; o engine pelo nome, admitido e recusado; a pessoa calada, a falar, com o trabalho parado, a fazer uma pergunta, com o turno gasto e com um driver sem ninguém atrás; o gate aberto e fechado; a shell do adapter; a integridade; o interruptor (ligado, a zero, pelas settings, já dito, dito por outro guard, dito antes de outra linha, noutra stage, sem shard, sem estado, de fora); settings recusadas; de fora e sem ter entrado; só com o cursor; sem sessão; sem estado; a frio; sem shard; payload malformado; registo malformado; o estado que é uma pasta (a avaliação falha fechada, com a fence levantada e baixada); a memória ilegível; os marcadores de observador; e as arestas da mutação: uma escrita que nomeia o developer, marcadores numa chamada que não é despacho, o marcador de contrato, o diário escrito pela shell, o interruptor sem ferramenta, com o guard nomeado por outra linha e com fins de linha CRLF |
| Diferencial completo | 2 981 verificações, 2 786 iguais, 195 desvios, 0 diferentes; 2 924 gravadas sobre 880 árvores, regravadas com o Bun 1.4.4 (as duas do estado que é uma pasta só correm ao vivo: uma árvore gravada guarda ficheiros). Das 2 848 gravadas antes, mudaram dez: três deixaram de ser desvio (a declaração em minúsculas), seis levam na resposta gravada do original uma hora ou um identificador de execução, e uma a versão do Bun num stack trace. Os desvios são os mesmos 195 por coincidência: quatro novos (os três do hook e a escrita PowerShell do `review-freeze`, da classe que já existia) e quatro levantados |
| Corpus | 31 690 casos (102 novos em `mutation_intent`), todos iguais |
| Testes | 550: seis novos na linha de comandos (o código espera pelo passo do engine; a pessoa que falou; o interruptor diz uma vez; um script por Bun; o marcador recusa pelo nome; a shell do adapter pelo nome de Focus) e um unitário (a recusa numa só frase) |
| Mutação | 78 mutantes no hook: 35 apanhados pelos testes da linha de comandos, 23 pelo diferencial, 13 depois de nove verificações e um teste escritos para as arestas, 3 equivalentes retirados do código (o shell e as ferramentas de ficheiros nomeados antes de "qualquer ferramenta com nome", e os alvos vazios perguntados duas vezes) e 4 anotados: a seleção do cursor quando a do hook falha (igual à dos outros hooks; nenhuma entrada conhecida as separa), a queda registada quando a leitura da Guard Policy falha sem ser por nome (nenhuma entrada conhecida lá chega: uma policy ilegível lê-se como a mais estrita), a recusa passada a uma só frase (a de uma diretiva em falta já o é; o que a função faz está no teste unitário) e a saída antecipada do despacho do developer (sem ela, o despacho chega à mesma recusa como ferramenta desconhecida). Em `mutation_intent`, a exclusão de `plan-approval` que voltou é apanhada pelo corpus novo, e a de `guard.plan-approval` fica anotada como equivalente |

**Desvios deliberados.** Um novo, que é a terceira declaração dos dois de 4e-ii-b e junta-lhes o que os torna visíveis; os outros dois são da classe de M4 que já existia (um registo de um marco posterior ao lado do hook).

| Desvio | Original | Focus |
| --- | --- | --- |
| `hook/plan-approval-guard/tool_script` | Admite um script instalado do engine corrido por Bun (pelo nome, ou pelo caminho absoluto do Bun que corre o hook) a fazer o que a espera admite, e o engine instalado pelo caminho absoluto do launcher ou do executável ativo | Uma instalação de Focus não tem scripts de ferramenta, o seu hook não é corrido por um Bun, e o layout da instalação é da distribuição (M8): o engine é admitido pelo nome, e o comando espera pelo passo do engine como qualquer comando para dentro do qual não se vê |
| `hook/plan-approval-guard/directive_marker`, `unit_scope_stamp` | Lê o marcador e decide por ele; lê o carimbo de unit | Recusa o hook pelo nome enquanto lá estiverem (M5, units) |

**Levantados.** `verbs/memory_lowercase_declaration` e `runtime/compile/project_path_with_a_backslash` (M2), pelo Bun 1.4.4.

### Fatia 4f: `guard-tool-call`

**Decisão de âmbito.** Um host que regista vários hooks para um evento arranca um processo por hook. No original, as quatro linhas de `PreToolUse` do `settings.json` são uma só, `engine hook guard-tool-call`, e o dispatcher corre os membros no seu processo (`HOOK_GROUPS` em `aidlc-command.ts`, `runGroupMembers` em `aidlc.ts`). O grupo é um registo, não um check: não decide nada. É isto que esta fatia porta, e com ela `guard-tool-call` sai da lista dos hooks recusados pelo nome. Um ficheiro de hook em falta ou que não exporta `run`, que o original responde como o processo desse hook responderia, não tem equivalente: os membros de Focus estão no binário.

**O que existe.** `focus-hooks::group`: os membros por ordem (`state-transition-guard`, `reviewer-scope`, `review-freeze`, `plan-approval-guard`) com o matcher que a linha de cada um tinha, uma expressão regular sobre o nome da ferramenta inteiro: leituras e pesquisas chegam aos três que leem; só uma escrita, um comando de shell ou um despacho chega ao guard do plano; uma ferramenta que nenhuma linha escolhia não chega a nenhum. Um pedido que não se consegue ler, ou que não nomeia ferramenta, vai a todos os membros, que o julgam. Todos os membros correm, mesmo depois de um recusar. A chamada é recusada (saída 2) quando algum recusa, com cada recusa dita uma vez pela ordem em que os membros correm; sem recusa, vale o código da primeira falha, e cada falha é dita uma vez. O que um membro diz no stderr enquanto deixa passar a chamada não é levado; o que responde no stdout é, tal como o escreveu.

**O que o diferencial encontrou.**

| Achado | Consequência |
| --- | --- |
| A decisão `deny` no stdout é dada pelo dispatcher só quando o pedido é de `PreToolUse` e o harness é Claude Code (`runHookModule`). Focus dava-a a qualquer recusa: um pedido de outro evento, ou sem evento, recusado com saída 2 levava uma decisão `PreToolUse` que o original não dá | A condição está portada, para o grupo e para cada hook sozinho; duas verificações (`no_event`, `other_event`) correm os dois |
| Um membro que lança (settings recusadas) não recusa nada, e o grupo leva o que ele disse. No original é o stack trace do runtime, com frames diferentes de membro para membro, e a mesma frase sai uma vez por membro que falha; o membro de Focus diz a frase como qualquer comando (`focus: frase`), igual em todos, e o grupo leva-a uma vez | Diferença de forma declarada em `SHAPES` (`group-thrown`): cada trace do original é lido como essa linha e as repetições saem antes de comparar; a saída, a frase e os ficheiros são comparados |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `hook/guard-tool-call/*` | 33 verificações, 31 iguais, 2 desvios, 0 diferentes: leituras e listagens que passam; o guard do plano a recusar uma escrita, uma shell e um despacho (por `Task` e por `Agent`), e a deixar passar o que nunca governa; duas recusas na mesma chamada, pela ordem dos membros; a mesma recusa de todos, dita uma vez; a transição e o revisor a recusar através do grupo (com `LS`, `Glob`, `NotebookRead` e `Grep`); ferramentas que nenhuma linha escolhia, com prefixo, com sufixo e noutra caixa; o despacho que só chega ao plano e a leitura que não lhe chega; o pedido sem ferramenta, com o nome que não é texto, e malformado de cinco maneiras; um membro posto de lado, calado e a falar, sozinho e ao lado de uma recusa; settings recusadas, sozinhas e depois de uma recusa; o interruptor de um e de todos; de fora; a frio; sem evento e de outro evento; os marcadores de observador |
| Diferencial completo | 3 013 verificações, 2 817 iguais, 196 desvios, 0 diferentes; 2 956 gravadas sobre 881 árvores. Os desvios passam de 195 a 196: entram os dois dos membros vistos pelo grupo e sai `dispatch/guard_tool_call_not_ported`. Todas as verificações de hook (`hook/*`) correm de novo com a condição da decisão `deny` |
| Testes | 555: dois unitários em `group` (a que membros chega cada ferramenta; o que o grupo responde ao que os membros respondem, com um membro injetado) e três da linha de comandos (um registo corre os checks; um membro que falha não recusa nada; a decisão `deny` só antes de uma ferramenta, e só em Claude Code) |
| Mutação | 33 mutantes no grupo, no dispatcher e na condição da decisão: 32 apanhados pelos testes, 1 equivalente retirado do código (uma razão vazia já não acrescenta nada ao que se junta). O diferencial sozinho apanha 29 dos 32, depois de sete verificações escritas para as arestas; os três que não vê ficam só nos testes: a ordem dos dois últimos membros (a recusa do `review-freeze` é de M5, e Focus recusa-o pelo nome), o que um membro diga no stderr deixando passar a chamada (nenhum membro o faz), e o nome do harness (o diferencial corre Focus como Claude Code; os outros harnesses são de M6) |

**Desvios deliberados.** Nenhum novo. `guard-tool-call/directive_marker` e `guard-tool-call/review_rows` são os desvios dos membros (o marcador de diretiva, as linhas de revisão: M5), vistos através do grupo; `dispatch/guard_tool_call_not_ported` deixou de existir.

## 16. Estado de M5

M5 está partido em fatias; nenhuma está feita. O orchestrator é o que responde a "o que se faz a seguir?": `next` lê o estado e o grafo e emite uma diretiva em JSON, o condutor (o agente) faz o que ela diz, e `report` grava a transição. À volta dele está o que as diretivas pesam e que os marcos anteriores recusam pelo nome: os recibos de revisão e de sumário, o marcador da diretiva ativa, as units, o Plan Approval. O critério do marco (secção 6) é uma jornada completa dar a mesma sequência de diretivas nos dois motores.

O marco entra de baixo para cima, como M2 a M4: primeiro a evidência que as transições e os guards já esperam (fatias 1 a 4), depois as units (5), o marcador (6), o orchestrator em si (7 a 10), o Plan Approval (11) e a jornada inteira (12). Cada fatia é um ou mais pull requests, verificada sozinha, e pode partir-se ao chegar lá, como a 4e de M4 se partiu em quatro.

### Decisões de âmbito

| | Decisão | Porquê |
| --- | --- | --- |
| 1 | **Solo e em série.** `invoke-swarm` nunca é emitida; um estado que peça Construction em paralelo recusa pelo nome | `swarm`, `swarm-checkpoints` e `worktree` estão adiados (secção 5). Um workflow novo nasce com `Construction Execution: serial` e `Construction Iteration: unit-major` |
| 2 | **O marcador é publicado como em Claude Code** (`generic-committed`). As tentativas seguidas de Copilot (`--aidlc-attempt-id`, `copilot-committed`, a pergunta de resume preservada) entram com o adapter, em M6; a aprovação de plano de Kiro legado recusa pelo nome | Os adapters são M6; Kiro está adiado |
| 3 | **`Unit Ownership: team` está adiado** (decisão de 2026-11-12, secção 11): as claims de `unit`, o board de equipa e o progresso derivado das units recusam pelo nome | É trabalho de várias equipas em vários checkouts, da mesma família que a Construction em paralelo; a jornada sem modelo do próprio original (`tests/harness/scope-run.ts`) declara os gates de units de equipa e os checkpoints de swarm fora do que conduz |
| 4 | **Os sensores de gate continuam em M7.** `report` abre um gate com a cerimónia `sensors` desligada, como `gate-start` desde M3 | Sem alteração |
| 5 | **O que `next` encaminha para módulos adiados recusa pelo nome**: os comandos de DocumentKB, de plugins (M8), de custos | "Input que não está portado é recusado, nunca ignorado" |
| 6 | **O code knowledge base entra** (`codekb-path`, `codekb-snapshot`, `codekb-publish`, `codekb-scope-diff`, `document-input`, `reclassify`), com a fatia que primeiro o precisa | Uma jornada brownfield passa por reverse engineering, que os usa |

`SOURCE_COMMITTED` (`FOCUS_SESSION_ANCHOR=1` no arranque de sessão) estava anotado como M5 e não é: quem o escreve é `attest`, adiado. Continua recusado pelo nome.

### Crates

Três crates novos. `focus-evidence` existe desde a fatia 1a e `focus-log` desde a 2a; `focus-orchestrate` confirma-se na fatia que o criar:

- **`focus-evidence`**: a identidade da fonte, os recibos pesados e o brief de revisão. Fica abaixo de `focus-lifecycle`, porque as transições, os hooks e o orchestrator leem todos a mesma evidência. Desde a fatia 2c-i lê Markdown (`focus-markdown`) e os nomes do grafo (`focus-graph`): o sumário que uma pessoa confirma está num ficheiro de perguntas.
- **`focus-log`**: o comando `log`, que escreve as linhas que essa evidência lê. Fica acima de `focus-lifecycle`. O plano punha `log` em `focus-evidence`; lido o código, não cabe lá: quem escreve uma pergunta precisa do que é do lifecycle (o marcador de uma confirmação que a pessoa não deu, as perguntas protegidas que uma pergunta nova retira, a confirmação de sumário devida) e, com `--message`, do armazém de mensagens que o turno humano escreve. Quem lê as linhas para as pesar (fatia 3) não precisa de nada disso e fica em baixo.
- **`focus-orchestrate`**: as diretivas, `next`, `report`, `continue`, `park`, `wait`, a Construction em série.

O leitor do marcador tem de ficar onde `focus-guards` o alcança (os guards leem dele a autoridade de uma ação, e ficam abaixo de `focus-lifecycle`): em `focus-state`, que já tem a recusa de um registo com marcador. Quem o escreve é `focus-orchestrate`.

### Fatias

Os intervalos de linhas são de `aidlc-orchestrate.ts` no pin, salvo indicação; servem para dar a ordem de grandeza, não são um mapa função a função.

| Fatia | O que traz | Levanta | Como se verifica | Estado |
| --- | --- | --- | --- | --- |
| 1. A identidade da fonte | O passeio pelos ficheiros fonte do projeto, a sua impressão digital, o snapshot `baseline-<hash>.tsv` e o campo `Source Baseline`; o snapshot de fonte de uma unit (`workspaceSourceState`, `readUnitSourceSnapshot` e o que está entre eles em `aidlc-lib.ts`) | `SHAPES["source-baseline"]` (criação e saltos passam a comparar o campo e a listagem); o baseline de uma stage que produz código nas transições; o desvio dos observadores na criação | Corpus sobre árvores de scratch (o que conta como fonte, o que se ignora, a ordem, o hash); diferencial | Feito, partida em quatro (abaixo). O snapshot do workspace e os leitores vêm com quem os usa: fatias 2, 3 e 11 |
| 2. `log` | `focus engine log decision`, `answer`, `answers`, `link` e `review`: as linhas `DECISION_RECORDED`, `QUESTION_ANSWERED`, `SUMMARY_CONFIRMATION_RECORDED`, `REVIEW_REQUESTED` e `REVIEW_COMPLETED`, os handoffs de um pipeline; o registo de mensagens lido (`aidlc-log.ts`, 4 486; `message-store`, 246); o `Source Fingerprint` de um pedido de revisão e o snapshot do workspace atrás dele (`workspaceSourceState`, `writeWorkspaceSourceSnapshot`) | Nada sozinho: é quem escreve o que a fatia 3 pesa. A escolha de uma pergunta de recuperação gravada por `log` fica para a fatia 4 | Diferencial (`log/*`) | Por fazer: partida em cinco (abaixo) |
| 3. Os recibos pesados | `freshReviewReceipts` (1 066) e `checkSummaryConfirmationEvidence` (647), os handoffs devidos por um pipeline, os pares pedido e conclusão de uma revisão; `review-brief` (2 365: `review`, `summary`, `context`) e os findings que um gate grava na decisão; a baseline lida de volta (`readBaselineSourceSnapshot`, `currentStageSourceBaseline`) para dizer que caminhos mudaram | As recusas pelo nome de `state` (`advance` e os gates com sumário, revisão ou pipeline devidos), de `review-freeze` e do grupo (`review_rows`), do salto para trás com linhas de revisão, e do turno humano com um pedido de revisão sem veredicto. Sem units: a dimensão por unit chega com a fatia 5 | Diferencial; corpus para os leitores puros (a tabela de findings, a secção `## Review`) | Por fazer |
| 4. A pergunta de recuperação | `evaluateGuardRefusal` e `guardAttemptState`: depois da frase de um guard, a pergunta com os remédios que o condutor pode oferecer, e a série da recusa em `.focus-engine/guard-refusals/`; a escolha gravada | `SHAPES["guard-ask"]` | Diferencial | Por fazer |
| 5. Units em série | As units como dados (o bloco de units, o DAG, os lotes de um bolt) e como transições: os verbos de `state` que M3 recusa (`set-skeleton-stance`, `set-construction-*`, `set-unit-gate-rhythm`, `sync-unit-scope-stage`, `acknowledge-compaction`, `reuse-artifact`, `lookup`, `practices-event`, `practices-promote`, `unit`), `jump reopen` e `execute --units`, `skip --unit`, uma stage de Construction em `advance`; `bolt` sem worktree (`start`, `complete`, `fail`, `abort`, `set-autonomy`, `checkpoint`; 1 511; `swarm-checkpoint`, `dispatch-event`, `hold-merge` e `release-merge` recusam pelo nome) e os checkpoints de Construction (`construction-checkpoints`, 1 618: a evidência, as decisões, o comando de verificação) | O limite 1 de M3, menos o que é de equipa ou de worktree (`set-unit-ownership`, `refresh-unit-progress`, `fold-unit-merge`, `fork`, `merge`); os recibos por unit da fatia 3 | Diferencial; o fuzz de transições ganha units. Prende os três chamadores que leem só o marcador de uma stage por unit e que a correção de M5 deixou por prender (os outputs de uma stage de Construction e a unit de um artefacto revisto) | Por fazer |
| 6. As diretivas e o marcador | `focus-orchestrate::directive` (os onze tipos de diretiva, `validateDirective`, as notas para o agente; `aidlc-directive.ts`, 2 976) e o marcador `active-directive.json`: escrever, ler, limpar, refrescar, pôr fora de data, o lock, e `state-writes.json` ao lado | As recusas pelo nome de um registo com marcador: `state`, `status`, `jump`, os hooks (`directive_marker`), e as admissões que os guards leem dele (as respostas da pergunta de recuperação, a continuação de um restart, o último passo do engine); o cursor que `set-status` preserva | Corpus (`validateDirective` sobre diretivas geradas, boas e estragadas; o parser do marcador); diferencial sobre árvores cujo marcador é o que o original escreveu | Por fazer |
| 7. `next` num trabalho que existe | O parser de flags (`parseNextFlags`, L3 867 a 4 281), a rota principal para um workflow em curso (a parte de `routeNext`, L7 138 a 9 014, que não cria nem pergunta), a diretiva de uma stage (`buildRunStageDirective` e os resolvedores de caminhos, L5 297 a 6 333; a narração; `inline-context`, `includes`, `rules-held`, `steering`) e a emissão (L981 a 1 900 e 6 333 a 7 073: o limite de tamanho, a entrega em partes por `load-steering` e `continue`, as linhas devidas à pessoa, o aviso de saúde dos hooks); `parked`, `done`, `print`, `error` | As linhas da pessoa (`person-lines`) no turno humano e no salto para trás | Corpus (o parser de flags; a diretiva de uma stage como função do estado e do grafo); diferencial (`orchestrate/next/*`); a jornada começa aqui e compara até onde Focus ainda recusa | Por fazer |
| 8. `report`, `park` e `wait` | O despacho de `report` por stage (L12 868 a 15 358): a evidência de conclusão, `--stage`, o report de um resume, a stance do skeleton, `--single`; `park`; `wait` (L15 774 a 15 972) | A frente da jornada passa a avançar stages | Diferencial (`orchestrate/report/*`); a jornada | Por fazer |
| 9. A porta da frente | `next` num workspace sem trabalho ou com um pedido novo (L2 184 a 3 160 e 4 304 a 5 007): o print de criação, a confirmação do scope, o tipo de projeto, a escolha de um intent, o encaminhamento de trabalho novo, o pedido guardado; a loja de perguntas (`question-store`, 412); o composer (a oferta, o despacho, `saveComposedScope`, `recompose`, `scope-change`, `scope-save`); os settings pelo `next` (`guard-switch`, 1 464: `config set`, `get`, `list`; `recorded-switches`, 403) e o code knowledge base | `--request <id>` na criação; o switch escrito aplicado e as concessões `*-at-creation-*` do turno humano; `saveComposedScope` (limite 5 de M1); os verbos de `utility` que a porta corre | Diferencial; a jornada parte do primeiro pedido | Por fazer |
| 10. Construction em série | O passeio unit a unit (`unit-major`, L11 312 a 12 153: o passo, o redo, o reopen, o salto em frente), as waves por stage (`stage-major`, L9 604 a 10 546), o gate e a stance do skeleton, a autonomia, a forma que um checkpoint dá à diretiva, os gates de bookkeeping. O swarm (L9 014 a 9 548) responde sempre que não | A jornada atravessa Construction com o Plan Approval desligado | Diferencial; a jornada | Por fazer |
| 11. Testing posture e Plan Approval | O contrato de testes (`testing-posture`, 5 453: `resolve`, `render`, `fingerprint`, `verify`, `begin`, `brief`, `reply`, `restore`), a pergunta que o engine guarda (`plan-approval-ask`, 2 196: o ask, o recibo, `PLAN_APPROVAL_RECORDED`), a resposta lida pelo turno humano, e a avaliação da aprovação no guard (`evaluateCodeGenerationApproval`); o snapshot do workspace lido de volta e comparado (`readWorkspaceSourceSnapshot`, `sameWorkspaceSource`, a cache por processo) | A recusa pelo nome de `plan-approval-guard` com marcador, e as do turno humano (`plan_challenge`, `protected_question`) | Corpus (o contrato e a impressão digital); diferencial; a jornada com o Plan Approval ligado | Por fazer |
| 12. Learnings e a jornada completa | `learnings surface` e `persist` (1 252); os onze scopes do original, do primeiro pedido a `done`, em greenfield e em brownfield, gravados | Fecha o marco | A jornada: a mesma sequência de diretivas, o mesmo estado e as mesmas linhas de auditoria | Por fazer |

### Fatia 1: a identidade da fonte, partida em quatro

A tabela acima dava esta fatia como uma região contígua e pequena. Lida, não é. O passeio é `filesystemSourceIdentity` (`aidlc-lib.ts`, L23 035 a 23 998): perto de mil linhas numa só função, com o registo dos caminhos de fonte, as pastas do harness, os modos de seguir um link, os outputs de .NET, a impressão digital como era antes de cada mudança de fronteira, e os fins de linha crus. Por baixo dele está a leitura de um repositório git sem correr git (o `HEAD`, os gitlinks, os repositórios embebidos, os objetos soltos e empacotados: L22 390 a 23 030). Com a listagem e os snapshots são cerca de três mil linhas. A fatia parte-se assim:

| Fatia | O que traz | Como se verifica | Estado |
| --- | --- | --- | --- |
| 1a. A listagem | O registo por caminho de uma árvore de fonte e o texto em que se guarda | Corpus | Feito |
| 1b. O passeio | Ficheiros, pastas, links e ficheiros especiais; o que fica fora pelo nome; o registry do projeto; os limites; os fins de linha; a impressão digital; um projeto que é um teto sobre vários repositórios | Corpus sobre árvores de scratch | Feito |
| 1c. O que o git diz | Um repositório embebido na árvore, listado como gitlink para o commit que o seu `HEAD` nomeia (a leitura de um repositório sem correr git) | Corpus sobre árvores de scratch com um repositório | Feito |
| 1d. O snapshot e o campo | `baseline-<hash>.tsv` escrito ao lado do registo; `Source Baseline` na criação, nos saltos e nas transições | Diferencial: `SHAPES["source-baseline"]` sai | Feito |

A fronteira entre 1b e 1c ficou onde a leitura do resto do passeio a pôs: o registry é um ficheiro do projeto, não do git, e entrou com o passeio; do git o passeio só precisa do `HEAD` de um repositório embebido, que é a 1c.

### Fatia 1a: a listagem de fonte

**O que existe.** O crate `focus-evidence`, com `source::listing`: `Listing` (as entradas pela ordem em que os caminhos foram postos, como o `Map` do original, porque o erro de escrita nomeia a primeira entrada má), `path_key` e `split_path_key` (a chave `<repo>\0<caminho>`), `serialize` e `parse` (o texto de um snapshot: uma linha por caminho, separada por tabs, ordenada pela chave em unidades UTF-16, com `\`, tab e os dois fins de linha escapados), `sha256` (o hash por que um snapshot é endereçado e verificado), `entries_equal` (duas entradas com modo só são iguais se forem idênticas; uma entrada sem modo é igual a outra com o mesmo objeto), `render_path_keys` e `changed_paths`. `is_valid_repo_name` desce de `focus-intent` para `focus-workspace::project`, onde os dois crates o alcançam.

O que fica para o passeio: o original lê uma entrada gravada sobre fins de linha crus como o ficheiro lido com eles assentes, a partir do que o passeio deste processo lembra (`currentFingerprintForm`). Sem passeio não há memória, e a comparação é a mesma.

**O que o corpus encontrou.** Nada que diferisse: os 1 129 casos saíram iguais à primeira. Uma coisa que a leitura sozinha não dava por certa: um caminho com um NUL dentro de um snapshot faz o original lançar ("Source path keys cannot contain NUL bytes"), onde qualquer outro defeito responde "não é uma listagem". `parse` devolve os dois separados.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `source_listing_parse` | 624 textos, todos iguais: 160 listagens (de uma a doze linhas, com e sem modo, em qualquer ordem), 460 que não o são (a última linha sem fim de linha, uma linha vazia, um CR antes de cada LF, o mesmo caminho duas vezes, um repo que não é um nome, um modo ou um objeto com a forma errada, um escape que o escritor não escreve, campos a mais e a menos, e 240 textos estragados carácter a carácter) e 4 em que o original lança |
| Corpus, `source_listing` | 505 pares de listagens dadas como entradas, todos iguais: 273 listagens que o escritor recusa (uma chave sem NUL, com dois, com um repo que não é um nome ou sem caminho; uma entrada com dois espaços, um tab, maiúsculas, um espaço a mais), com o erro a nomear a primeira; 396 pares com caminhos mudados e 109 sem nenhum (um modo ganho ou perdido não é uma mudança, um modo trocado é) |
| Testes unitários | 3: duas entradas ausentes são iguais (nenhuma listagem compara uma, por isso o corpus não o diz), uma chave posta outra vez mantém o lugar, e uma listagem lê-se como foi escrita |
| Mutação | 58 mutantes em `listing`: 54 apanhados pelo corpus, 1 pelo teste unitário, e 3 equivalentes anotados (que espaço separa o modo do objeto, quando nenhum dos dois leva espaços; que NUL separa o repo do caminho, quando um segundo é recusado onde quer que caia; e tomar por objeto uma entrada que não tem forma nenhuma, quando o outro lado só é igual se a tiver) |
| `cargo test` | 559 testes; corpus de 32 819 casos (1 129 novos), todos iguais; os 31 690 anteriores iguais byte a byte |

### Fatia 1b: o passeio

**Decisão de âmbito.** O passeio inteiro, menos duas coisas que recusa pelo nome onde as leria: o contexto de uma worktree delegada (adiada) e um repositório git embebido na árvore, cujo `HEAD` é a 1c (e que com ela deixou de ser recusado). O que o original recolhe para o índice de um snapshot (pathspecs, caminhos de snapshot, links para fora) serve só a Construction em paralelo e não se recolhe.

**O que existe.** Em `focus-evidence`, `source::walk`: `identity` (`filesystemSourceIdentity`: o que os ficheiros de uma raiz são) e `workspace_source` (`walkWorkspaceSource`: o mesmo sobre um projeto que é um repositório, ou um teto sobre vários). O passeio decide pelo nome, nunca perguntando ao git, o que fica fora: as pastas que nunca são fonte, os ficheiros que um sistema operativo, uma corrida de coverage ou um editor deixam, as pastas de output (sempre; e, ao lado de um ficheiro de projeto .NET ou do manifesto de um gestor de pacotes, as que esse preenche), o workspace e as árvores de harness que se anunciam, os settings do engine na raiz, e o diretório do engine de um registo onde quer que haja um workspace. O registry do projeto (`.focus-source-paths.json`) nomeia as exceções: caminhos que são fonte seja qual for o nome ou o conteúdo, e caminhos que não são. Um link é seguido um salto de cada vez; o que está fora do projeto só entra se ler como fonte ou como texto. `source::raw` guarda os hashes que este processo tirou das duas maneiras (sobre os bytes crus e sobre o texto com cada CRLF lido como LF), para um valor gravado antes dessa leitura continuar a bater; `listing::entries_equal` passa a usá-lo.

Uma falha traz o código, a frase e o caminho do original (`Stop::Failed`); o que não está portado é outro valor (`Stop::NotPorted`), para o comando recusar em vez de responder por uma árvore que não leu.

**O nome do engine dentro de um hash.** A impressão digital é tirada sobre linhas, e o nome do engine entra nelas por três sítios: os dois rótulos sob os quais se tira o hash, a linha do próprio ficheiro de registry, e, no valor do passeio anterior, as linhas dos settings do engine. Uma impressão digital de Focus nunca é igual à do original para a mesma árvore. Para a comparação não ficar por fazer, os nomes são uma opção do passeio (`Names`, com os de Focus por omissão), o caso do corpus dá os do original em pedaços, e os dois motores andam sobre a mesma árvore com os mesmos nomes: o que se compara é o hash exato. Onde o nome fica noutro caminho andado (os settings de um membro de um teto), a impressão digital compara-se como `<named>` e as linhas da listagem comparam-se na mesma.

**O que o corpus encontrou.** O passeio saiu igual à primeira em 459 de 468 árvores. Das nove diferenças, sete eram da comparação e não do passeio, e duas eram de um `.git` abaixo da raiz, que é da 1c.

| Achado | Consequência |
| --- | --- |
| Um ficheiro alcançado por um link é listado pelo seu caminho real: um ficheiro que o nome deixa fora (`.coverage`) aparece na listagem se um link, ou o registry, o alcançar | Portado assim. A ordem das linhas da listagem depende então de nomes que ficam entre `.aidlc-…` e `.focus-…`: as linhas comparam-se numa ordem que não depende do nome do engine (a ordem em que um snapshot se escreve é dos casos de 1a) |
| Um LF a seguir a um CR faz parte do fim de linha e não é contado; um LF sozinho também não conta como carácter imprimível | A fronteira entre texto e binário (um carácter de controlo por cada 128 imprimíveis) tem um caso de cada lado, com e sem LF |
| Um link para fora do projeto pode levar a qualquer sítio do host: um caso que apontava para a pasta temporária inteira parava no limite de dez mil ficheiros de fora | O caso saiu: uma árvore de teste não depende do host |
| Um `.git` abaixo da raiz faz da pasta à volta um repositório embebido | Fica para 1c: nesta fatia o corpus só tem `.git` na raiz |
| Em macOS, quatro árvores davam outra resposta: tinham nomes que só diferem em maiúsculas (`Thumbs.db` e `thumbs.db`, um ficheiro `MAKEFILE` e um link `Makefile`, um repositório `Api` onde existe `api`), que um sistema de ficheiros que ignora a caixa guarda como um só | O defeito era das árvores, não do passeio. Esses nomes ficam cada um na sua árvore, e o teste do corpus recusa uma árvore em que dois nomes só difiram na caixa |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `source_walk` | 502 árvores, todas iguais: 411 lidas até ao fim (398 com a impressão digital exata, 13 como `<named>`; 133 com o valor do passeio anterior; 37 com o valor sobre bytes crus) e 91 em que o passeio pára, com o código, a frase e o caminho do original (40 por um registry inválido, 21 por um caminho registado dentro do que fica fora, 15 por um limite, 12 por links que saem e voltam, dão a volta ou não levam a nada, 3 por uma raiz que não se lê). 192 com links, 148 com registry, 26 sobre um teto, 29 como o passeio era antes de os outputs de .NET saírem da fronteira |
| Testes unitários | 3: a impressão digital sob os nomes de Focus, calculada à mão (o desvio abaixo); um limite é um inteiro positivo sem zero à esquerda; o que não está portado recusa pelo nome |
| Mutação | 223 mutantes em `walk`: 195 apanhados pelo corpus (153 nas duas primeiras passagens; os outros depois de 36 árvores escritas para as arestas: um caminho registado que sai e volta visto só num dos sítios, a forma exata do diretório do engine de um registo, o registry no tamanho-limite, um NUL numa linha longa, a amostra de 8 KiB, um ficheiro de fora com nome de fonte e conteúdo binário, a mesma pasta alcançada como dois sítios registados, uma árvore de fora alcançada duas vezes, um membro que é um link partido, os settings na raiz de um teto e na de um membro) e 28 equivalentes, anotados no topo do módulo: perguntas que o original faz duas vezes (um salto para fora visto em três sítios, um registry que é um link e não é um ficheiro regular, um manifest acima do tamanho duas vezes) e quatro que só são equivalentes enquanto não se leem repositórios embebidos (o que uma segunda visita a uma pasta grava), a rever na 1c |
| `cargo test` | 562 testes; corpus de 33 321 casos (502 novos), todos iguais; os 32 819 anteriores iguais byte a byte |

**O que fica por comparar.** Os limites que pedem dez mil ficheiros ou gigabytes (250 000 ficheiros, 4 GiB, 10 000 ficheiros e 64 MiB de fora, 256 MiB do passeio anterior) estão lidos do original e não comparados: só os quatro que uma variável baixa têm casos. Em Windows os casos de árvore não correm (links, modos e named pipes não se montam lá), como os de `mutation_intent`.

**Desvio deliberado.** Um, com o teste marcado em `walk`. Não chegou a haver entrada em `SHAPES`: com 1d um comando mostra o valor, e o diferencial verifica o hash de cada motor contra o snapshot que esse motor escreveu (abaixo).

| Desvio | Original | Focus |
| --- | --- | --- |
| Os nomes dentro da impressão digital | `aidlc-filesystem-source-v2`, `aidlc-workspace-source-v2`, `.aidlc-source-paths.json` | `focus-filesystem-source-v2`, `focus-workspace-source-v2`, `.focus-source-paths.json`: a impressão digital de uma árvore difere por construção, como os hashes do recibo de validade (secção 14) |

### Fatia 1c: o commit de um repositório embebido

**O que existe.** Em `focus-evidence`, `source::git`: `head_oid` (`gitHeadOid`), o commit em que um repositório está, lido dos seus ficheiros sem correr git. Lê o `HEAD`; segue a referência que ele nomeia pelas referências soltas (que podem nomear outra, até oito saltos) e por `packed-refs`; lê do `config` o tamanho de um id (SHA-1 ou SHA-256, e recusa um formato de repositório que esta leitura não cobre); e só aceita o id se o armazém de objetos tiver um commit com ele, solto ou num pack (os índices nas duas versões, os offsets grandes, e uma alteração seguida até ao objeto de que é alteração, por offset ou por id). O repositório pode ser uma pasta `.git` ou um ficheiro que aponta para ela, pode partilhar referências e objetos com outras worktrees (`commondir`) e pode pedir objetos emprestados (`alternates`, até sessenta e quatro armazéns). Toda a leitura tem limite, e toda a dúvida é a resposta "nenhum": o passeio então não consegue ligar a fonte, que é a leitura segura de um repositório que não percebe.

No passeio, uma pasta com `.git` é uma entrada só, um gitlink (`160000`) para esse commit, e os seus ficheiros são andados na mesma. A recusa pelo nome de 1b sai; fica a da worktree delegada.

**Uma dependência nova.** `flate2`, para descomprimir um objeto solto (zlib). A descompressão é feita pela interface de baixo nível, para um stream cortado ser um erro como é no original: a de alto nível devolvia o que tivesse lido até aí.

**O que o corpus encontrou.** Nada que diferisse: as 297 árvores da primeira corrida saíram iguais, e as 71 escritas depois da mutação também.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `source_walk` com um repositório | 368 árvores, todas iguais: 123 com o gitlink na listagem, 9 em que o repositório não é listado (alcançado por um link, debaixo de um nome que fica fora, fora do projeto) e 236 em que o `HEAD` não se resolve num commit. 120 com packs. Cada forma de `HEAD`, cada nome que é e não é o de uma referência, cada defeito de um objeto solto (o tipo, o tamanho declarado, o cabeçalho com 128 e 129 bytes, um stream cortado, lixo depois do stream), cada forma do `config`, o repositório atrás de um ficheiro, de `commondir` e de `alternates`, e nos packs: o objeto abaixo e acima de todos os de um índice, um índice em que não se pode confiar antes de um bom, o offset antes do décimo segundo byte e no fim do pack, uma cadeia de alterações com 64 e 65 de fundo |
| Testes unitários | 1: a descompressão pára no seu limite e no fim do stream (o corpus não leva um objeto de 64 MiB) |
| Mutação | 137 mutantes (128 em `git`, 9 em `walk`): 120 apanhados pelo corpus (81 na primeira passagem; os outros depois das 71 árvores) e 17 equivalentes, anotados no topo do módulo: perguntas que o original faz duas vezes (o tamanho de um ficheiro antes e depois de o abrir, um objeto antes do décimo segundo byte recusado onde se lê) e ciclos que acabam tanto pelo que já foi visto como pela profundidade. Dos quatro mutantes de 1b que só eram equivalentes sem repositórios embebidos, três passaram a ser apanhados (uma pasta de um repositório alcançada outra vez por um link é andada pelo que a primeira visita não pôde listar, e por mais nada) e um é equivalente de vez |
| `cargo test` | 563 testes; corpus de 33 689 casos (368 novos), todos iguais; os 33 321 anteriores iguais byte a byte |

**O que fica por comparar.** Os limites de tamanho (16 MiB de um objeto solto, 64 MiB descomprimidos, 128 MiB de um índice e 256 MiB de todos, 16 MiB de `packed-refs`, 256 índices) estão lidos do original. Um objeto é o que os seus bytes dizem: nem o original nem Focus verificam que o conteúdo bate com o id sob que está guardado, e o corpus usa isso para montar os repositórios à mão.

### Fatia 1d: o snapshot e o campo

**Decisão de âmbito.** O que um comando de hoje escreve: o snapshot com que uma stage começa e o campo que o nomeia. O que só se lê ou escreve a partir de código que ainda não existe vem com esse código, para nada entrar só por leitura: o snapshot do workspace que `log review` guarda atrás de um `Source Fingerprint` (`workspaceSourceState`, `writeWorkspaceSourceSnapshot`) é da fatia 2; `readBaselineSourceSnapshot` e `currentStageSourceBaseline`, que dizem a um recibo que caminhos mudaram desde a baseline, são da fatia 3; `readWorkspaceSourceSnapshot`, `sameWorkspaceSource` e a cache por processo (`withWorkspaceSourceStateCache`) são do Plan Approval e do contrato de testes, fatia 11. As linhas dessas fatias na tabela acima passam a dizê-lo.

**O que existe.** Em `focus-evidence`, `source::snapshot`: `baseline_field` (`sourceBaselineAuditFields`) anda a fonte do trabalho e devolve o valor do campo. Uma fonte lida dá `sha256:<hash>` e a listagem escrita em `<registo>/.focus-engine/source-review/<stage>/baseline-<12 primeiros hex>.tsv`. Uma fonte que não se consegue ler (um registry inválido, um repositório embebido sem commit, um limite) dá `unbindable` se houver um checkout git, e a listagem vazia se não houver: o checkout é o do projeto quando o trabalho não tem repositórios membros, e o de qualquer um deles quando tem, caso em que o do projeto não conta. `write_baseline` (`writeBaselineSourceSnapshot`, `writeSourceSnapshot`) nunca escreve através de um link (o caminho é verificado antes e depois de criar a pasta), deixa estar um ficheiro que já tenha os mesmos bytes, recusa substituir um que tenha outros ("Source snapshot address collision or corruption"), e começa na barreira do observador. Os repositórios membros vêm da linha do próprio trabalho no registry, lidos por `focus-lifecycle::registry::source_repos`, que recusa uma linha cujo `repos` não seja uma lista de nomes (um dos desvios abaixo).

Quem chama: a criação, na linha `WORKFLOW_STARTED`, com os repositórios que a própria criação recebeu (a linha do registry é escrita por último); o salto, uma vez, com o mesmo valor nas linhas `STAGE_JUMPED` e `STAGE_STARTED`; e `advance` e um `skip` com rota, na linha `STAGE_STARTED` de uma stage que produz código (`workspace_requires`), sob o nome dessa stage. A criação e o salto tiram-na sempre sob `code-generation`, seja qual for a stage em que o trabalho fica.

**O que o diferencial encontrou.**

| Achado | Consequência |
| --- | --- |
| O hash da baseline nunca é igual nos dois motores (a 1b explica porquê), e o nome do ficheiro é esse hash | O diferencial não o tira antes de comparar: verifica-o. O snapshot de cada motor é lido, o seu hash recalculado e comparado com o nome do ficheiro e com o campo da linha, e só então os dois passam a `baseline-<verified>.tsv` e `sha256:<verified>`. As linhas da listagem comparam-se uma a uma; o hash de um ficheiro que é o da própria árvore passa a `<own>` (os ficheiros que levam o nome do engine no texto diferem), e a ordem é uma que não depende do nome. `SHAPES["source-baseline"]` saiu sem substituto |
| A criação acrescenta ao `.gitignore` depois de tirar a baseline: a linha da listagem tem o hash que o ficheiro tinha antes | O diferencial guarda os hashes da árvore antes de correr e aceita os dois |
| Com a baseline escrita, um observador que chega à criação ou a um salto pára onde o original pára, no `writeFileAtomic` do snapshot | Os quatro desvios de observador (`utility/create/observer_*`, `jump/observer/*`) deixaram de o ser e saíram de `DEVIATIONS`; as linhas dos marcos em que foram declarados dizem-no |
| Um repositório membro que é uma worktree delegada: o original lê-o pela fronteira dessa worktree (os caminhos de código da sua unit) | Focus não lê essa fronteira e recusa a linha pelo nome. Desvio declarado (abaixo) |
| Um `repos` do registry que não é uma lista de nomes (um número, um `null` ou uma lista lá dentro; um texto no lugar da lista; um nome vazio). A mutação mostrou que Focus deixava de fora o que não fosse texto e seguia com o resto. O original não: cai onde primeiro junta um caminho, com o `TypeError` do runtime (`The "paths[1]" property must be of type string, got number`; `repos.some is not a function`), e um nome vazio lê-o como o projeto inteiro, engine e harness incluídos, como se fosse esse membro | Ignorar é o que a regra proíbe. Focus recusa a linha pelo nome, como nos outros sítios em que o original cai num `TypeError` (os campos de um payload, um ficheiro que não é um grafo). Um `repos` `null` é nenhum nos dois; um nome que não é uma pasta do projeto (`..`, `app/sub`) é recusado pelos dois com a mesma frase, a do escritor da listagem |
| A criação e o salto dentro de uma worktree delegada já eram recusados por Focus, pela mesma função dos outros comandos, mas nenhum dos dois tinha check nem entrada em `DEVIATIONS` | Entraram os dois checks e a entrada |
| A gravação guardava, de cada árvore de um salto ou de uma transição, só os ficheiros do engine; e a base de que a repetição parte não tinha os dois ficheiros que o empacotador põe na raiz (`.gitignore`, `.mcp.json`). Com a baseline, esses comandos leem o projeto inteiro: a repetição falhou em 96 checks, com a listagem de Focus sem os ficheiros que a do original tinha | O defeito era do gravador. As árvores de `jump` e de `state` passam a ser guardadas inteiras, como já eram as de `utility`, e os dois ficheiros entram na base vendorizada (`refresh.sh`). Com eles na base, um teste de `admission` que partia de um projeto "sem trabalho de fonte" deixou de partir: cada um dos dois conta como fonte, como no original, e o teste passa a dizê-lo antes de os tirar. Um teste de CLI do salto afirmava que as linhas acabavam sem o campo, e passa a lê-lo |
| O diferencial montava as duas árvores de cada check antes de correr o primeiro. Com mais checks deixou de caber no disco | Cada par de árvores é montado quando o seu grupo corre e removido a seguir. O fuzz de `runtime` falhava por não nomear a sua ferramenta, também em `main`: corrigido |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, criação | 9 checks novos, todos iguais: os ficheiros fonte de um projeto; o que o registry junta e tira; uma fonte ilegível sem checkout git (a listagem vazia) e com ele (`unbindable`); um repositório embebido sem commit; dois repositórios membros, um só, e um deles ilegível; os outputs de .NET. E todas as criações que já existiam, agora com o campo e o snapshot comparados |
| Diferencial, salto | 19 checks novos, todos iguais: os cinco casos da criação; o lugar do snapshot já ocupado pelos mesmos bytes (fica) e por outros (recusa); os repositórios da linha do próprio trabalho e não os da primeira linha, um que não existe, o checkout do projeto que não conta quando há membros, o de um membro que conta; `repos` a `null`; dois nomes que não são pastas do projeto; o snapshot atrás de um link, de um link para nada, e a pasta da stage atrás de um link; os outputs de .NET. Sete desvios declarados (abaixo) |
| Diferencial, transições | 7 checks, todos iguais: `advance` para uma stage que produz código com a fonte lida, ilegível e `unbindable`, e com o modo de resposta reutilizado (a ordem dos campos da linha); `skip` com rota para a mesma stage, lida e `unbindable`; e um `skip` sem rota, que não começa stage nenhuma e não tira baseline |
| Diferencial, tudo | 4 256 checks com `--fuzz 600`: 4 056 iguais, 200 desvios declarados, nenhum diferente |
| Repetição da gravação | 2 996 checks sobre 921 árvores, todos iguais ao que o original fez |
| Testes unitários | 3: uma baseline pede um registo e um nome de stage (nenhum comando lá chega sem os dois); o desvio da worktree delegada entre os repositórios; e o desvio de um `repos` que não é uma lista de nomes, forma a forma |
| Mutação | 40 mutantes (em `snapshot`, em `registry::source_repos` e nas chamadas da criação, do salto e das transições): 38 apanhados (28 à primeira; 4 depois de checks escritos para eles: os outputs de .NET, um link para nada no lugar do snapshot, uma stage que não produz código e não leva o campo, a ordem dos campos com o modo de resposta; 6 escritos com a recusa de `repos`) e 2 equivalentes, anotados no topo do módulo: a pasta do registo criada antes de tudo, quando nenhuma linha chega ali sem registo, e a segunda verificação de links, depois de criar a pasta. Um mutante que escapou era um defeito: o achado de `repos` acima |
| `cargo test` | 566 testes; o corpus não mudou (33 689 casos) |

**Desvios deliberados.** Dois novos, cada um com o seu teste marcado e a sua entrada em `DEVIATIONS`; e um que já existia e fica agora declarado para os dois comandos que faltavam.

| Desvio | Original | Focus |
| --- | --- | --- |
| Um repositório membro que é uma worktree delegada (`jump/execute/source_repos_delegated_worktree`) | Lê a fonte pela fronteira da worktree e escreve a baseline | Recusa a linha pelo nome: "…Focus does not read that yet: the Source Baseline of this row cannot be taken." As worktrees vêm com Construction em paralelo, adiada |
| Um `repos` do registry que não é uma lista de nomes (`jump/execute/source_repos_refused_*`, cinco formas) | Cai no `TypeError` do runtime onde primeiro junta um caminho; com um nome vazio, lista o projeto inteiro como esse membro | Recusa a linha pelo nome: "…lists the repositories of <registo> as something that is not a list of names. Fix the row's `repos`…" |
| A criação e o salto dentro de uma worktree delegada (`utility/create/delegated_worktree`, `jump/execute/delegated_worktree`) | Resolve a worktree para o trabalho do checkout principal e corre | Recusa o checkout pelo nome, como `audit`, `state`, `runtime` e os hooks |

**O que ficou por fazer, e está feito a seguir.** A recusa de `repos` valia só para quem anda os repositórios como fonte. Os outros leitores da mesma lista, de M3 e M4, continuavam a deixar de fora o que não fosse texto: é a correção abaixo.

### Correção: um `repos` que não é uma lista de nomes, em todos os leitores

A fatia 1d recusou a linha onde os repositórios são andados como fonte e deixou escrito que os outros leitores ficavam por comparar. Comparados:

| Leitor | O que o original faz com `["app", 7]`, `[null]`, `[["app"]]` ou `"app"` | Focus antes | Focus agora |
| --- | --- | --- | --- |
| As pastas do code knowledge base (`advance` em `reverse-engineering`) | Cai onde junta o caminho de cada pasta, com o `TypeError` do runtime (`The "paths[4]" property must be of type string, got number`; `….map is not a function`) | Deixava de fora o que não era texto e seguia: uma resposta que o original não dá | Recusa a linha pelo nome |
| A revisão não registada de um gate (`approve` com a política `strict`) | Segue: só procura nomes na lista, e para uma stage que não é do code knowledge base nem isso | Seguia | Recusa a linha pelo nome |
| `review-freeze` | Atrás do desvio dos recibos de revisão (fatia 3): não comparável ainda | Seguia | Recusa, como o resto do hook recusa o que não lê |
| `workspace_has_work` (uma stage que produz código) | Não alcançável por um comando: as stages de Construction são recusadas até à fatia 5 | Seguia | Recusa (o teste unitário de `registry`) |
| O baseline de um salto para trás sobre o code knowledge base | O baseline lê a linha primeiro: é o desvio da 1d | | |

**Decisão.** Uma regra só, para todos os leitores: `registry::intent_repos` devolve um erro para um `repos` que não seja uma lista de texto, e `source_repos` (o do passeio) recusa além disso um nome vazio. É um desvio declarado também onde o original segue (o gate): lá o original só não cai porque não usa os nomes, e o que faria com eles numa stage do code knowledge base é o que o runtime fizer de um número num `Set`. Focus não porta essas coerções e por isso recusa, em vez de responder por uma lista que não leu. Um nome vazio continua a ser o que cada leitor do original faz dele: o code knowledge base deixa-o de fora (igual nos dois, `advance/repos_empty_name`).

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial | 12 checks novos. Iguais, 4: o backstop sob `strict` sem e com repositórios membros (não havia nenhum check que lá chegasse: com a política `off` o backstop não corre), e um nome vazio nos dois leitores. Desvios declarados, 8: as quatro formas no code knowledge base e no gate |
| Testes | O teste marcado de `registry` (as oito formas recusadas pelos dois leitores, o nome vazio só pelo do passeio, `null` e a falta do campo como nenhum); um teste de CLI marcado: um salto recusa a linha e não move nada |
| Diferencial, tudo | 4 268 checks com `--fuzz 600`: 4 060 iguais, 208 desvios declarados, nenhum diferente; a gravação (3 008 checks sobre 933 árvores) repete-se igual |
| `cargo test` | 567 testes |
| Mutação | 9 mutantes: 7 apanhados (6 pelo teste de `registry`; o leitor do passeio trocado pelo outro só depois do teste de CLI, que o diferencial não vê por ser um desvio) e 2 que levaram a código mais simples ou a uma nota: a saída antecipada do backstop com o ledger vazio era redundante e saiu; ler a lista só para uma stage do code knowledge base é onde o original a lê, e o único chamador já leu a mesma linha |

### Correção: o pai de um processo, sem `ps`

`resolve_finds_the_calling_conversation_through_the_process_tree` falhou uma vez no CI de macOS do pull request #38 e passou na repetição, sem o código ter mudado: o comando caiu para o cursor partilhado em vez de encontrar a conversa pela árvore de processos. A causa era de Focus, não do teste. O passeio pela árvore tem um orçamento de 50 ms (o do original), e Focus, fora de Linux, perguntava o seu próprio pai com um `ps` depois de o orçamento já estar a contar; num runner sob carga esse `ps` gastava-o todo e a conversa registada passava por não existir. O original lê `process.ppid`, que não custa nada. `ancestry::parent_pid` passa a ler o pai do sistema em Unix (`std::os::unix::process::parent_id`), e o trace dos hooks, que em macOS escrevia `0` como pai, escreve o pai. Em Linux nada muda: o pai já vinha de `/proc`. Em Windows o pai continua a vir da identidade do processo, que lá é uma leitura e não um subprocesso.

### Fatia 2: `log`, partida em cinco

`aidlc-log.ts` são 4 486 linhas e cinco verbos de tamanhos muito diferentes: `review` tem cerca de 1 440, `answer` 720, `decision` 215, `link` 205 e `answers` 135. Os verbos partilham o parser, a recusa de um valor que chegou partido em vários argumentos e o erro registado; o resto é de cada um. A fatia parte-se pelo verbo, e o checkpoint de sumário, que atravessa `decision` e `answer`, tem a sua.

| Fatia | O que traz | Recusa pelo nome até à fatia que o traz | Como se verifica | Estado |
| --- | --- | --- | --- | --- |
| 2a. O comando e `decision` | `focus engine log`: `--project-dir`, o parser de flags, a recusa de valores partidos (`refuseSplitValues`: uma palavra fora de qualquer flag, um fragmento que começa por `--`, as aspas simples do Command Prompt), o erro que fica na auditoria; um workspace sem trabalho selecionado. `decision`: a linha `DECISION_RECORDED` (stage, decision, options, rationale, `--single`), a frase que diz que a pessoa já confirmou (`impliedConfirmationMarker`), `--message` com a prova da mensagem (`proveMessage`: existe, traz palavras da pessoa, não está gasta, veio deste chat), as perguntas protegidas retiradas | `--checkpoint verification-command` e `construction-policy` (fatia 5), `plan-approval` e `--batch-file` (fatia 11), `--unit` (fatia 5) | Diferencial (`log/decision/*`, um grupo novo com a sua ferramenta); corpus para a recusa de valores partidos e para o marcador de confirmação, que são funções puras | Feito |
| 2b. `answers` e `answer` | `answers` (só leitura: as perguntas de uma stage e as respostas que têm); `answer`: `QUESTION_ANSWERED`, `--details` e `--details-file`, `--on-instruction`, `--message` e a mensagem herdada da pergunta, a dica de uma mensagem que chegou antes da pergunta | As respostas às perguntas do engine (`PLAN_APPROVAL_RECORDED`, `PLAN_APPROVAL_OVERRIDDEN`, `--units`, `--reason`, `--park`: fatia 11; `VERIFICATION_COMMAND_RECORDED` e `CONSTRUCTION_POLICY_RECORDED`: fatia 5) | Diferencial | Feito (abaixo, em três partes) |
| 2c. O checkpoint de sumário | `--checkpoint summary-confirmation --questions-file` em `decision` e em `answer`: a evidência da pergunta (`summaryQuestionEvidence`), a decisão pendente (`pendingSummaryDecision`), `SUMMARY_CONFIRMATION_RECORDED`, e a recusa de uma pergunta ou resposta de sumário sem o checkpoint, com os comandos certos (`refusePlainSummaryConfirmation`) | | Diferencial. É o que a fatia 3 pesa (`checkSummaryConfirmationEvidence`) | Em curso: partida em três (abaixo), as duas primeiras feitas |
| 2d. `link` | `PIPELINE_LINK_COMPLETED`: o handoff de um elo de um pipeline, com o artefacto e o repositório | | Diferencial | Por fazer |
| 2e. `review` | `REVIEW_REQUESTED` e `REVIEW_COMPLETED`: o revisor de uma stage, a iteração, o veredicto e o ficheiro da revisão (os títulos despromovidos), o orçamento de revisões e a recuperação quando se esgota, o veredicto pendente; o `Source Fingerprint` de um pedido e o snapshot do workspace atrás dele (`workspaceSourceState`, `writeWorkspaceSourceSnapshot`, o que a fatia 1d deixou para aqui) | `--unit` e o registo de uma worktree de swarm | Diferencial. Lida, pode ter de se partir outra vez: o pedido e a conclusão são metades com a sua contabilidade | Por fazer |

O que Focus já tem e estas fatias usam: o armazém de mensagens que o turno humano escreve (`focus-hooks::messages`; falta ler de volta), `summary_confirmation_owed`, as perguntas protegidas retiradas num gate (`withdraw_protected_questions`), a vida dos hooks (`hook_liveness`) e a sessão de quem chama.

### Fatia 2a: o comando e `decision`

**Decisão de âmbito.** A pergunta de uma stage, inteira, menos a prova de uma mensagem (`--message`), que é o pull request a seguir para cada um ser verificado sozinho. O que o engine emparelha com um registo seu é recusado pelo nome até à fatia que traz o registo: o checkpoint de sumário (2c), o comando de verificação e a política de Construction e uma pergunta sobre uma unit (5), o Plan Approval e o seu lote (11). Os outros quatro verbos do comando recusam pelo nome.

**O que existe.** O crate `focus-log` e o comando `focus engine log`. `flags` tem o parser (`parseFlags`: um flag sem valor no fim, ou seguido de outro flag, é recusado; quatro flags não levam valor) e `refuse_split_values` (`refuseSplitValues`): `decision` só lê os seus flags, e por isso uma palavra fora de qualquer flag desaparecia e o registo ficava com um valor cortado. Um valor chega partido quando não foi passado como um só argumento, ou quando o PowerShell lhe tirou as aspas de dentro. A recusa anda sobre os argumentos crus (com o par `--project-dir`), nomeia o flag que as palavras seguiram de facto, distingue um flag que o verbo não lê, uma palavra que não é de flag nenhum e as aspas simples que o Command Prompt deixa nas palavras, e diz como passar o valor. `decision` escreve `DECISION_RECORDED` (`Stage`, `Decision`, e `Options`, `Rationale` e `Workflow` quando os há), recusa uma pergunta cujo texto diz que a pessoa já confirmou (`implied_confirmation_marker`, em `focus-lifecycle::reply`: a confirmação atribuída ao hook, ou a pessoa que "implicitamente" concordou; uma frase entre aspas é uma pergunta como outra), e retira as perguntas protegidas do chat que pergunta: o que `--session` nomeia, o que a árvore de processos diz, ou as de todos quando não se sabe.

Sem trabalho selecionado nada é escrito: a linha cairia na raiz dos registos do space, onde nunca vive uma auditoria.

**O que o diferencial encontrou.** A primeira corrida do grupo novo deu 30 checks iguais em 44; dos catorze diferentes, doze eram o que esta fatia recusa pelo nome, e dois não:

| Achado | Consequência |
| --- | --- |
| A pasta das perguntas protegidas atrás de um link: o original recusa com a frase da cadeia de links (`… is a symlink, and no path component may be a symlink here`), e Focus dizia a outra, a de um link no caminho. O defeito estava em `main` desde M4, em `gate-start`, sem check que o visse | `withdraw_protected_questions` usa a recusa da cadeia. Um check novo em `gates` compara-o |
| O marcador de autoatribuição cortava a frase do texto original com as posições medidas no texto mascarado. O original mascara cada unidade UTF-16 com um espaço; Focus mascarava cada carácter, e media em bytes: com um carácter não ASCII entre aspas antes da frase, a frase saía deslocada, e o corte podia cair a meio de um carácter (um pânico, num `reject` com feedback da pessoa). Também em `main` desde M4 | A máscara passa a ter um espaço por unidade, e a frase é cortada do original pelas unidades que a máscara conta. Dois passos de um check de `log` têm aspas com acentos e emoji antes da frase |
| Com `--unit`, o original grava a pergunta com a unit e a tentativa do claim, sem validar nada num trabalho que não tem units | Focus recusa o flag pelo nome (desvio): units são a fatia 5 |
| O subcomando é sempre o primeiro argumento depois de tirado `--project-dir`: uma palavra antes dele faz dela o subcomando, e a recusa de valores partidos não chega a correr | Três passos escritos para a recusa nunca lá chegavam (davam "Unknown subcommand", igual nos dois). A mutação mostrou-o; foram trocados por passos que chegam |
| O diferencial acrescenta `--project-dir` no fim de cada passo: nenhum passo acabava nas palavras soltas de um valor | Um check cujos passos nomeiam o projeto primeiro, para nada vir depois |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `log` | 46 checks: 34 iguais (o comando sem verbo, com um que não tem e sem trabalho selecionado; os flags; os valores partidos, em vinte e dois passos; a linha simples, inteira, duas vezes, com valores vazios, com uma stage que não é do grafo, com um flag dado duas vezes, com várias linhas; um observador; os checkpoints que o verbo não conhece; as duas escolhas do sumário onde nada é devido; a confirmação implícita, entre aspas, depois de aspas, com o guard desligado; as perguntas protegidas de uma sessão, de todas, de um nome que não dá ficheiro, da sessão de quem chama, e atrás de um link) e 12 desvios declarados |
| Diferencial, tudo | 4 315 checks com `--fuzz 600`: 4 095 iguais, 220 desvios declarados, nenhum diferente |
| Repetição da gravação | 3 052 checks sobre 937 árvores, todos iguais. A repetição não conhecia a ferramenta `log` e passou a conhecer; a gravação tem de ter `decision` a acabar bem e a recusar |
| Testes | 2 de CLI marcados (os verbos e as perguntas que não estão portados, com a frase de cada um e nada gravado; as duas escolhas do sumário reconhecidas com maiúsculas, o decorador e a pontuação) e 1 unitário (o que não é flag nem valor fica à parte, por ordem: nenhum verbo portado o lê) |
| Mutação | 60 mutantes (`flags`, `decision`, o comando, o marcador e a retirada das perguntas protegidas): 47 apanhados à primeira e 13 depois de checks e testes escritos para eles (a sessão dita pela árvore de processos, um nome de sessão que não dá nome de ficheiro com ficheiros desse nome lá, uma frase que atravessa aspas, as palavras no fim dos argumentos, duas palavras soltas). Nenhum equivalente |
| `cargo test` | 570 testes |

**Desvios deliberados.** Quatro (eram cinco: `--message` deixou de o ser com a prova da mensagem, abaixo), cada um com a entrada em `DEVIATIONS` e nos dois testes marcados de `focus-cli/tests/log.rs`.

| Desvio | Original | Focus |
| --- | --- | --- |
| `link`, `review` (`answers` e `answer` deixaram de o ser com a 2b) | Corre cada um | Recusa pelo nome até às fatias 2d e 2e |
| `--checkpoint summary-confirmation`, `verification-command`, `construction-policy`, `plan-approval` | Lê os flags que cada um pede e grava a pergunta com o seu registo | Recusa o checkpoint pelo nome (fatias 2c, 5 e 11) |
| `--unit` e `--batch-file` | Grava a unit com o seu claim; recusa o lote pela sua forma | Recusa os dois flags pelo nome (fatias 5 e 11) |
| Uma pergunta simples com as duas escolhas do sumário, numa stage que deve uma confirmação de sumário | Recusa com os dois comandos que gravam um sumário | Recusa pelo nome: esses comandos são a fatia 2c |

### Fatia 2a, segunda parte: a prova de uma mensagem

**O que existe.** `--message <id>` em `decision`: uma pergunta registada depois de a pessoa já ter respondido nomeia a mensagem que lhe respondeu, e a linha leva `Message Id`. O agente mapeia, o engine prova, sem ler palavra nenhuma (`focus-log::message::prove`, `proveMessage`): o registo existe no armazém; traz palavras da pessoa, e o turno dele não foi só um comando ao engine ou uma pergunta sobre um switch (as marcas `Reply` da linha `HUMAN_TURN`); não chegou antes da última decisão de outro tipo que não uma resposta (oito eventos gastam uma mensagem; um empate ao segundo favorece a mensagem, porque uma só pode dar uma aprovação e as respostas ao lado); e veio do chat que o comando serve, ou, quando o engine não sabe qual é, do único que falou desde essa decisão. Em `focus-hooks::messages`, onde o turno humano escreve o armazém, entram os leitores: `read_message` (um registo só é do armazém se cada campo lá estiver com o tipo que o escritor lhe dá) e `list_messages` (do mais antigo para o mais novo).

**O que o diferencial encontrou.** Nada que diferisse: os 50 checks da primeira corrida saíram iguais, e os 11 escritos depois da mutação também.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `log/decision/message*` | 61 checks, todos iguais: a mensagem aceite, com e sem a linha do turno, do terminal; um id que não é de registo nenhum, em seis formas; sem palavras; o turno marcado como comando, como pergunta e de outra forma, numa linha de outra mensagem e numa linha que não é um turno; gasta por cada um dos oito eventos, pela última de duas linhas em qualquer ordem, e não por respostas; o empate ao segundo; o chat do comando, outro, o terminal; dois chats, um deles o terminal, um deles antes da decisão, um deles no segundo da decisão; e vinte e seis formas de um ficheiro que não é um registo do armazém (cada campo com o tipo errado ou em falta, acima do tamanho, atrás de um link, com um nome que não é um id) |
| Diferencial, tudo | 4 375 checks com `--fuzz 600`: 4 156 iguais, 219 desvios declarados, nenhum diferente |
| Repetição da gravação | 3 108 checks sobre 993 árvores, todos iguais |
| Testes unitários | 1: o armazém lista-se do mais antigo para o mais novo, com o id a desempatar (o único leitor de hoje toma os chats da lista como um conjunto) |
| Mutação | 50 mutantes (a prova, os leitores do armazém, o campo na linha): 47 apanhados (36 à primeira, 11 depois dos checks e do teste escritos para eles) e 3 equivalentes, anotados no módulo: a última recusa nunca é alcançada (a mensagem nomeada é ela própria uma das contadas, salvo se for mais antiga do que a última decisão, e então já foi recusada como gasta; é assim no original); de duas linhas com o mesmo instante qualquer uma é a última; e o id escrito na linha é o do registo provado, que é o id por que foi lido |
| `cargo test` | 571 testes |

### Fatia 2b, primeira parte: `answers`

**O que existe.** `focus engine log answers --stage <slug> [--unit <unit>]` (`focus-log::answers`, `handleAnswers`): as perguntas de uma stage lidas da auditoria com as respostas que têm. Só lê: não escreve nada, e uma recusa sua é impressa sem deixar linha. Responde com as perguntas respondidas (a pergunta, as opções, o âmbito, quando foi feita, a resposta, quando, e a mensagem que a prova quando a linha a nomeia), as que estão em aberto (com as que um cancelamento gastou) e as respostas ambíguas.

Uma pergunta é uma linha `DECISION_RECORDED` e uma resposta uma `QUESTION_ANSWERED`, e nenhuma nomeia a outra. O emparelhamento é o do original, linha a linha. Uma resposta responde a todas as perguntas do seu âmbito (a unit, a tentativa, o workflow de uma stage sozinha) que estavam abertas quando chegou: o protocolo regista as perguntas de um menu e depois uma resposta. Uma resposta sem pergunta aberta que venha depois de uma troca fechada, sem pergunta registada entre as duas, continua essa troca (`a; b`). Um cancelamento gasta a pergunta sem a responder. E duas linhas em shards diferentes com o mesmo instante não têm ordem: uma resposta que possa ser de mais de uma leitura fica como ambígua, com as perguntas candidatas, em vez de ser dada à pergunta do shard cujo nome ordena primeiro; e uma pergunta de que uma resposta ambígua pode ter sido não é dada a mais nenhuma.

**O que o diferencial encontrou.** Nada que diferisse: os 40 checks da primeira corrida saíram iguais, e os 6 escritos depois da mutação também.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `log/answers` | 46 checks, todos iguais: os flags que o verbo aceita e os que não; sem trabalho selecionado, sem ficheiro de estado, com ele atrás de um link, com um shard ilegível; nada, perguntas abertas, respondidas, um menu, uma troca continuada uma e duas vezes, e não continuada por cima de uma pergunta, de uma pergunta que um cancelamento gastou, de uma pergunta empatada com a resposta, ou por uma resposta empatada com a que fechou a troca; uma resposta sozinha e antes da sua pergunta; seis formas do que é e não é um cancelamento; a mensagem da resposta e a da pergunta; uma linha que não é pergunta; as linhas de um checkpoint, de outra stage e de outros âmbitos; e, com dois shards, a resposta depois, empatada e antes, duas respostas empatadas, uma resposta empatada com um cancelamento, perguntas nos dois, a ordem das abertas, e um shard com as linhas fora da ordem dos seus instantes (o ciclo, em que todas as respostas têm outra antes) |
| Diferencial, tudo | 4 420 checks com `--fuzz 600`: 4 202 iguais, 218 desvios declarados, nenhum diferente |
| Repetição da gravação | 3 152 checks sobre 1 034 árvores, todos iguais; a gravação tem de ter `answers` a responder e a recusar |
| Mutação | 47 mutantes: 44 apanhados (38 à primeira, 6 depois de checks escritos para eles: a ordem dos campos do âmbito, e cinco arestas da continuação de uma troca) e 2 equivalentes, anotados no módulo (todas as linhas lidas são da stage pedida, e uma linha nunca é comparada consigo). Um levou a tirar código: as notas do histórico não são perguntas nem respostas, e filtrá-las não mudava nada |
| `cargo test` | 571 testes |

### Fatia 2b, segunda parte: `answer`

**Decisão de âmbito.** A resposta à pergunta de uma stage. Recusado pelo nome até à fatia que o traz: o texto da resposta num ficheiro (`--details-file`) e uma escolha que a pessoa deixou ao agente (`--on-instruction`, `--on-instruction-file`), que são o pull request a seguir (a terceira parte, abaixo); e as perguntas do próprio engine, cada uma com o seu registo (a pergunta de recuperação, fatia 4; a confirmação de sumário, 2c; os checkpoints de Construction e `--unit`, 5; o Plan Approval, com `--override`, `--override-file` e `--batch-file`, 11).

**O que existe.** `focus engine log answer --stage <slug> --details <texto>` (`focus-log::answer`, `handleAnswer`) escreve `QUESTION_ANSWERED`. O agente passa a escolha que leu na resposta; o engine grava-a ao lado das palavras da própria pessoa e não lê sentido nenhum nelas. O que verifica é que houve resposta:

- Uma caixa de pergunta dispensada, cancelada ou expirada não é resposta.
- Uma escolha que o texto atribui ao assistente não é da pessoa (o mesmo marcador dos gates, com as frases de uma resposta).
- Uma aprovação é de um gate e não de uma pergunta. Com a stage num gate aberto e nenhuma pergunta feita desde que abriu (`has_pending_decision_at_gate`), a resposta é reconhecida e não é escrita, para não gastar a resposta de que a aprovação precisa; e mesmo essa pede uma resposta nova da pessoa.
- Uma resposta precisa de uma mensagem da pessoa que ainda não tenha respondido a nada. Uma só responde a todas as perguntas abertas quando chegou; uma pergunta feita depois espera pela seguinte. A exceção é a mensagem nomeada (`--message`, provada como em `decision`) ou herdada da pergunta com que foi registada. Quando a pergunta foi registada depois de a resposta chegar, a recusa nomeia as mensagens que chegaram antes dela (`messages_before_question`, `late_question_hint`), para o agente nomear uma em vez de perguntar outra vez.

Ao lado da escolha vão o id da mensagem, as palavras da pessoa (`Person Reply`: as da mensagem provada, ou as que o chat guardou logo a seguir à linha do último turno, `latest_person_turn_words`), e uma nota quando a resposta não é nenhuma das que a pessoa escolheu numa caixa de pergunta (`picker_answer_note`). A pergunta do modo de resposta ("How would you like to answer them?") fica gravada pela opção que a resposta nomeia, pelo rótulo ou pelo número, que é o que as stages seguintes reutilizam. Em Construction autónoma não há ninguém ao teclado: nada disto é pedido nem guardado.

**O que o diferencial encontrou.**

| Achado | Consequência |
| --- | --- |
| Os dois checks que diferiram na primeira corrida diferiam no ficheiro da auditoria, não no que o comando disse: o normalizador conhecia `aidlc-state.ts` e `aidlc-orchestrate.ts` dentro de um ficheiro, e não `aidlc-log.ts` | O normalizador passa a conhecer os três |
| A varredura de mutação parou ao fim de meia hora, a meio, e deixou um mutante aplicado; os resultados, que passavam por um filtro, perderam-se | O ficheiro foi reposto. As varreduras longas correm com o limite alargado e gravam cada resultado num ficheiro à medida que sai |
| Sete mutantes da lista nunca correram: o formatador tinha mudado o texto das suas âncoras, e a lista só avisava | Reescritos e corridos; três pediram um check cada |
| O âmbito de uma pergunta de sumário aberta só se vê num desvio (a recusa pelo nome até à 2c): o diferencial não o compara | O teste de CLI marcado cobre-o: a pergunta gravada com o checkpoint, e três perguntas em ciclo entre dois shards, em que as do último instante valem por todas |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `log/answer` | 124 checks: 110 iguais e 14 desvios declarados. Os iguais: os flags e os valores partidos (com a ajuda própria de uma resposta); a resposta que chegou, a que não chegou, um menu, uma resposta gasta por outra resposta e por um gate, um turno que foi só um comando ou uma pergunta sobre um switch, o guard desligado, sem ninguém na sessão; cinco formas de uma caixa dispensada e quatro de uma escolha do assistente; o gate (com e sem resposta, com uma pergunta aberta, respondida, de antes do gate, de outra stage, o gate aberto duas vezes, o gate de outra stage depois); catorze formas da pergunta do modo; as palavras da pessoa (dentro, antes e depois do turno, noutro shard, do último turno, sem sessão); catorze formas da nota da caixa; a mensagem nomeada, herdada, herdada sem prova, da pergunta mais recente e de outra stage; dezassete formas da pergunta registada depois da resposta; o sumário devido com outra pergunta aberta, de outra stage, com outro checkpoint, fora de ordem; e Construction autónoma |
| Diferencial, tudo | 4 543 checks com `--fuzz 600`: 4 312 iguais, 231 desvios declarados, nenhum diferente |
| Repetição da gravação | 3 274 checks sobre 1 111 árvores, todos iguais; a gravação tem de ter `answer` a gravar e a recusar |
| Testes | 1 de CLI marcado novo (as doze formas recusadas pelo nome e a pergunta de sumário aberta em três formas) |
| Mutação | 87 mutantes (o verbo, os leitores de mensagens, os leitores de perguntas abertas, as palavras da pessoa, a nota da caixa): 86 apanhados (55 à primeira, 31 depois de 28 checks e do teste de CLI escritos para eles) e 1 equivalente, anotado onde está (a linha do próprio gate não é uma pergunta) |
| `cargo test` | 572 testes |

### Fatia 2b, terceira parte: o texto num ficheiro, e a escolha que a pessoa deixou ao agente

**O que existe.**

- **`--details-file` e `--on-instruction-file`** (`focus-log::answer_text`, `readAnswerTextFile`). Um texto com caracteres de shell chega num ficheiro que o agente escreveu com a sua ferramenta de ficheiros, para nenhuma shell o ler pelo caminho. O ficheiro tem de estar na pasta `.focus-engine/answer-text/` do registo, nomeado a partir do registo, do projeto ou por inteiro; é um ficheiro simples com um só nome, alcançado sem link nenhum, de 64 KiB no máximo (`files::read_regular_within`, o limite que `read_regular` passou a aceitar). É lido uma vez e removido, antes de qualquer outra verificação: é uma mensagem em trânsito, não um registo. Perde um fim de linha final e não pode ficar vazio. O texto e o ficheiro do mesmo campo não se passam juntos.
- **`--on-instruction <palavras>`**. A pessoa disse "decide tu": o agente escolhe e grava a escolha com as palavras que lha deixaram. A linha leva `Answer Source: chosen by the agent as the person asked` e `Instruction`, e não leva mensagem, `Person Reply` nem nota da caixa, porque a escolha não é da pessoa. Não pede uma resposta nova, mas pede que a pessoa tenha falado neste trabalho: um turno que possa trazer um pedido, ou o pedido que criou o trabalho. Recusado: sem palavras; com qualquer `--checkpoint` (uma pergunta do engine é da pessoa); com `FOCUS_UNATTENDED=1`; e num gate, porque uma aprovação é da pessoa de cada vez.

**O que o diferencial encontrou.**

| Achado | Consequência |
| --- | --- |
| Os 44 checks saíram iguais à primeira | Nenhuma correção ao código |
| No caminho "nada do que a pessoa disse está registado", o original pede a dica de presença sem nomear o projeto: é a dica do projeto em que o seu próprio script está instalado | Focus tem um só projeto por invocação, aquele para que o comando corre, e os dois são o mesmo sempre que o engine corre para o projeto que o tem; é o que o diferencial compara. Anotado onde a dica é pedida |
| Dois mutantes sobreviveram sem mudar resposta nenhuma: um nome absoluto nunca começa pelo nome da pasta, e um campo de uma linha já é lido aparado | O primeiro fica anotado no módulo, onde o original o tem; o segundo aparar saiu |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `log/answer/details_file*`, `on_instruction*`, `both_files` | 44 checks, todos iguais. O ficheiro: lido e removido (a segunda leitura já não o encontra); com o texto ao lado; nomeado do projeto, por inteiro, com barras ao contrário, numa subpasta; onze nomes fora da pasta; em falta; vazio, em branco, só um fim de linha, com dois, com CRLF, sem nenhum, com linhas dentro, uma dispensa, no limite e um byte acima; uma pasta, um link, atrás de um link; sem trabalho selecionado; num observador; removido mesmo quando a resposta é recusada a seguir. A instrução: gravada (os espaços reduzidos a um), duas vezes seguidas; vazia; com um checkpoint; sem ninguém na sessão; sem nada dito, com um pedido em branco, depois de um turno, de um turno que foi só um comando e de uma pergunta sobre um switch; num gate, com e sem pergunta aberta; com uma dispensa e com uma escolha do assistente; sem nada do turno ao lado; na pergunta do modo; com `--single`; num ficheiro, com o ficheiro e as palavras, com os dois ficheiros; em Construction autónoma |
| Diferencial, tudo | 4 584 checks com `--fuzz 600`: 4 356 iguais, 228 desvios declarados (três a menos: os três flags deixaram de ser recusados pelo nome), nenhum diferente |
| Repetição da gravação | 3 313 checks sobre 1 140 árvores, todos iguais |
| Testes | O teste de CLI marcado deixa de listar os três flags; as doze formas recusadas pelo nome passam a nove |
| Mutação | 40 mutantes (o leitor do ficheiro, o limite de tamanho, os ramos da instrução): 38 apanhados à primeira e 2 equivalentes (acima) |
| `cargo test` | 572 testes |

### Correção: uma stage por unit sem o marcador

Ao ler a recusa de um sumário (fatia 2c-ii), que nomeia a unit numa stage que corre por unit, apareceu um defeito de M3 e M4. O original pergunta "esta stage corre por unit?" de duas maneiras, e `focus-lifecycle` só tinha uma:

- **`isPerUnitStage`**: o marcador `for_each: unit-of-work`, ou ser uma das cinco stages que correm sempre por unit (`functional-design`, `nfr-requirements`, `nfr-design`, `infrastructure-design`, `code-generation`). É uma verificação cruzada do original, para que um grafo compilado que perdeu o marcador numa delas continue a correr essa stage por unit. `focus-graph::query::is_per_unit` já a tinha.
- **`stage.for_each === "unit-of-work"`**: só o marcador.

`focus_lifecycle::stage::is_per_unit` lia só o marcador, e três dos seus oito chamadores perguntam a primeira: o status (a linha `Current Step`), o session start (a linha da unit) e a resolução de um artefacto consumido por outra stage (o recibo de validade). Num grafo compilado sem o marcador numa das cinco, Focus não nomeava o passo e resolvia o artefacto ao nível da stage.

**O que mudou.** Duas funções, e cada chamador pergunta a que o seu original pergunta: `stage::is_per_unit` (a regra do grafo) para esses três; `stage::has_unit_marker` para os cinco que leem só o marcador (a pasta e a existência dos outputs de uma stage, as entradas de revisão de um salto, a unit de um artefacto revisto, o gate de uma unit com `Unit Ownership: team`). O modelo com que o diferencial recalcula os hashes de um recibo usava só o marcador e passou a usar a mesma regra.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, antes da correção | 6 checks novos diferentes: 3 do session start (o grafo sem o marcador na stage da unit, na stage corrente, em todas) e 3 do recibo de validade (o artefacto debaixo das units, ao nível da stage, em lado nenhum) |
| Diferencial, depois | 18 checks novos, todos iguais: os 6 acima; o status com o marcador em falta na stage da unit, na corrente e em todas; nos dois, uma stage que não é das cinco e que só o marcador faz por unit; o gate de uma unit e três saltos para trás por cima de Construction, que leem só o marcador; e os três recibos com o marcador no sítio |
| Diferencial, tudo | 4 602 checks com `--fuzz 600`: 4 374 iguais, 228 desvios declarados, nenhum diferente |
| Repetição da gravação | 3 331 checks sobre 1 158 árvores, todos iguais |
| Mutação | 9 mutantes, um por chamador, cada um a perguntar a outra função: 6 apanhados (os três da regra do grafo, e dos cinco do marcador o gate de uma unit e as entradas de revisão, depois de 4 checks escritos para eles). 3 sobrevivem: a pasta e a existência dos outputs de uma stage e a unit de um artefacto revisto só são lidas num gate ou numa conclusão de uma das cinco stages, que são de Construction e Focus recusa pelo nome. Ficam para a fatia 5, que traz Construction e os prende |
| `cargo test` | 574 testes |

### Fatia 2c, partida em três

O checkpoint de sumário atravessa `decision` e `answer`, e assenta em duas leituras do ficheiro de perguntas que são funções e não comandos. Lido o código, a resposta ao sumário escreve mais do que uma linha: escreve a autorização que os outputs da stage passam a levar. A fatia parte-se em três, cada uma verificada por si:

| Fatia | O que traz | Recusa pelo nome até à fatia que o traz | Como se verifica | Estado |
| --- | --- | --- | --- | --- |
| 2c-i. O que a pessoa confirmou | `focus-evidence::summary`: a escolha na linha `[Answer]:` da secção de sumário (`summaryConfirmationAnswer`) e o hash do que foi confirmado (`summaryConfirmationContentHash`); `heading_key` (`headingKey`): a secção que um título nomeia, lido para lá de um emoji | | Corpus do oráculo: são funções | Feito (abaixo) |
| 2c-ii. A pergunta | `decision --checkpoint summary-confirmation --questions-file`: a evidência da pergunta (`summaryQuestionEvidence`) e a linha com `Checkpoint` e `Questions File`; a recusa de uma pergunta ou de uma resposta de sumário sem o checkpoint, com os dois comandos certos (`refusePlainSummaryConfirmation`, `summaryConfirmationCommands`) | `--unit` (fatia 5) | Diferencial | Feito (abaixo) |
| 2c-iii. A resposta | `answer --checkpoint summary-confirmation`: a decisão pendente (`pendingSummaryDecision`), o piso da tentativa (`summaryAttemptFloors`), a autorização (`.focus-engine/summary-authorization/`, escrita antes do recibo e reposta se o recibo falhar) e a linha `SUMMARY_CONFIRMATION_RECORDED` | `--unit` e uma stage por unit (fatia 5) | Diferencial; corpus para o piso e o id da autorização | Por fazer |

### Fatia 2c-i: o que a pessoa confirmou

**O que existe.**

- **`focus-evidence::summary::answer`** (`summaryConfirmationAnswer`): a escolha na única linha `[Answer]:` da secção `## Consolidated Summary Confirmation` de um ficheiro de perguntas; nada quando a secção não existe, ou quando tem zero linhas dessas ou mais de uma. A secção é a de um título `##` de topo, lido pelo que um leitor do ficheiro renderizado vê: um título dentro de um bloco de código, de um comentário ou de uma citação não é a secção.
- **`focus-evidence::summary::content_hash`** (`summaryConfirmationContentHash`): o SHA-256 do que a pessoa confirmou. É o ficheiro tal como está, com os fins de linha feitos `\n`, sem a única secção que pode ser acrescentada depois da confirmação e não faz parte dela: `## Assumption Confirmation`, com a linha horizontal que possa estar antes. Uma pergunta acrescentada depois do sumário (`## Q<n>`) e `## Requested Changes Feedback` entram no hash, para que uma confirmação posterior as cubra. Recusado, com a frase do original: o sumário em falta ou duas vezes, uma pergunta ou a secção de assumptions repetida, e qualquer outro título depois do sumário (`#`, sublinhado ou em HTML), porque nada diz se a pessoa confirmou o que está debaixo dele.
- **`focus-graph::vocabulary::heading_key`** (`headingKey`): a secção que um título nomeia, lido para lá de uma decoração de emoji. `ℹ️ Sources` nomeia `Sources`; um símbolo mostrado como texto (um `©` sem seletor de variação) não é decoração.
- `focus-markdown::visible_lines_of` passa a ser público: quem lê a estrutura e as linhas visíveis analisa o documento uma vez.

**Decisões.**

| Decisão | Porquê |
| --- | --- |
| A tabela de emoji é a do `regress` (`\p{RGI_Emoji}` sob a flag `v`), e o corpus tem as 3 953 sequências | Que sequências são emoji é uma tabela do runtime, e um padrão não diz o que aceita. A lista vem dos dados do Unicode 17 (`tools/oracle/rgi_emoji.txt`, com o aviso da licença do Unicode ao lado), e é verificada dos dois lados: `tools/oracle/emoji.ts`, que `refresh.sh` corre, confirma que o Bun aceita cada sequência da lista e nenhuma das 11 milhões que estão ao lado delas; o corpus (tipo `heading_key`) compara a tabela de Focus com essa, sequência a sequência, com o que está ao lado de cada uma. Fecha o risco R7 |
| O `regress` é compilado com otimização nos testes (`[profile.dev.package.regress]`) | Todos os testes passam pelo motor de regex, e sem otimização passam nele a maior parte do tempo: vinte mil títulos levavam doze segundos e levam seis décimos. A suite inteira desce de meia hora para quatro minutos e meio |
| A primeira linha de um ficheiro que começa por uma marca de ordem de bytes é lida sem a marca | O original procura uma tag de HTML na linha tal como o ficheiro a tem, com as colunas da linha sem a marca, e nessa linha as colunas caem uma unidade antes. Nenhuma resposta depende disso: antes da secção de sumário só conta um título `#`, e a primeira linha de um ficheiro está antes dela ou é ela |
| `nested` só é dito de um título `#` | O original calcula-o para as outras duas formas de título e não o lê em nenhuma |

**O que o corpus encontrou.**

| Achado | Consequência |
| --- | --- |
| As 783 leituras de sumário e as 73 mil de títulos saíram iguais à primeira | Nenhuma correção ao código; a varredura de mutação é que disse o que o corpus não prendia |
| 28 dos primeiros 104 mutantes sobreviveram | 17 pediram documentos (um título de cada profundidade e um sem palavras; o que parece um título dentro de um bloco de HTML; um sublinhado com espaços depois, e um título sublinhado dentro de um contentor; um bloco de HTML aberto por uma tag que não é de bloco, e os que são literais até ao fim; a tag de um título como código, num comentário, em duas linhas, dentro do atributo de outra tag; uma pergunta para lá da nona; uma linha horizontal de dois traços, indentada, indentada como código; uma linha escrita como fronteira onde o renderizador não lê título nenhum). 6 eram de código que saiu: a leitura em unidades UTF-16, que só a primeira linha de um ficheiro com marca distinguia, e o `nested` das outras formas de título. 5 são equivalentes, anotados no módulo |
| O teste do corpus de `focus-evidence` contava um caso de sumário como um par de listagens | Conta-os à parte: quantos têm hash, quantos são recusados, quantos têm escolha, em branco ou nenhuma |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Corpus, `summary` | 834 documentos, todos iguais: 382 com hash e 452 recusados (87 sem a secção, 104 com a secção, uma pergunta ou as assumptions repetidas, 182 com um título `#` que não pode seguir o sumário, 27 com um sublinhado e 52 com um em HTML); a escolha lida em 392, em branco em 71, nenhuma em 371 |
| Corpus, `heading_key` | 157 casos, todos iguais: 73 287 títulos dados pelos code points do que está antes das palavras (em 12 339 tudo é decoração, em 60 851 nada, em 97 uma parte) e 28 escritos por inteiro |
| Corpus, tudo | 34 680 casos (eram 33 689), todos iguais; `refresh.sh` gera o mesmo corpus, casos e respostas |
| `emoji.ts` | 3 953 sequências listadas, nenhuma recusada pelo Bun; 11 173 765 perguntadas ao lado, nenhuma aceite sem estar na lista |
| Testes | 2 novos: o que o hash é (a secção de assumptions acrescentada, com e sem linha horizontal, não muda o que foi confirmado; uma pergunta acrescentada e a própria escolha mudam), e a decoração de um título |
| Mutação | 108 mutantes (os três leitores de títulos, o hash, a escolha, `heading_key`): 97 apanhados (76 à primeira, 21 depois dos documentos escritos para eles), 5 equivalentes anotados e 6 em código que saiu |
| `cargo test` | 574 testes. A suite inteira, nesta máquina e com tudo compilado: 1 840 s antes, 270 s depois, com 991 casos a mais |

### Fatia 2c-ii: a pergunta de sumário

**O que existe.** `focus-log::summary`, usado por `decision` e por `answer`:

- **A pergunta com o checkpoint.** `focus engine log decision --checkpoint summary-confirmation --questions-file <ficheiro>` grava a pergunta de sumário de uma stage ligada ao ficheiro de perguntas que a pessoa lê (`summaryQuestionEvidence`): a linha leva `Checkpoint: Consolidated Summary Confirmation` e `Questions File`, o nome do ficheiro a partir do projeto. O ficheiro tem de estar dentro do registo e chamar-se `<algo>-questions.md`; a sua secção de sumário tem de ter a única linha `[Answer]:` ainda em branco; e tem de poder ser um (o hash de 2c-i recusa-o com a frase do original quando não pode).
- **A forma simples recusada com os comandos certos** (`refusePlainSummaryConfirmation`, `summaryConfirmationCommands`). Uma pergunta que oferece as duas escolhas do sumário, ou uma resposta à pergunta de sumário aberta para a stage, gravadas sem o checkpoint, são interações comuns que um gate nunca conta: a stage recusaria para sempre. Numa stage que deve um sumário são recusadas com os dois comandos que contam, já preenchidos: a stage, a unit (ou `<unit>` numa stage que corre por unit), `--single`, e o ficheiro de perguntas que a stage tem (o primeiro pelo nome, ou `<path to <stage>-questions.md>` quando não tem nenhum). A resposta leva a escolha que o agente leu: `Looks correct`, `Request changes` com o que a pessoa pediu para mudar, ou `<their choice>` quando a resposta não nomeia nenhuma. Três frases: a de uma pergunta, a de uma resposta a uma pergunta já gravada com o checkpoint, e a de uma resposta a uma pergunta feita na forma simples.

**Desvio deliberado: o ficheiro de perguntas não é aberto pelo nome.**

| Desvio | Original | Focus |
| --- | --- | --- |
| O ficheiro que `--questions-file` nomeia | Lê-o com `readFileSync`: atravessa um link, uma pasta que é um link e um segundo hard link; de um link para nada diz que o ficheiro não existe; numa pasta cai com o `EISDIR` do runtime | Lê-o como qualquer ficheiro do registo que decide alguma coisa (o que é lido entra, pelo hash, no recibo que um gate pesa): um ficheiro regular com um só nome, alcançado sem link nenhum. O resto é recusado pelo nome e nada é gravado. Teste marcado em `crates/focus-cli/tests/log.rs`; `DEVIATIONS`, `log/decision/summary_checkpoint_(a_link, a_dangling_link, behind_a_link, second_name, a_folder)` |

**O que o diferencial encontrou.**

| Achado | Consequência |
| --- | --- |
| Os checks diferiam só em como cada engine escreve um comando para o agente correr: o original nomeia o seu script (`bun .claude/tools/aidlc-log.ts decision ...`) e Focus o seu binário (`focus engine log decision ...`) | O normalizador passa a ler os dois como o mesmo comando, com os argumentos comparados um a um |
| O original também toma por sumário uma resposta que nomeia uma das duas escolhas, e depois deixa-a passar quando não há pergunta de sumário aberta: só a pergunta aberta decide | Focus pergunta só pela pergunta aberta. Sem pergunta aberta não lê o ficheiro de estado, como o original não o lê para decidir |
| A recusa por nome que `answer` tinha desde a 2b só se via num desvio, e o teste de CLI marcado é que cobria o ciclo de três perguntas entre dois shards | A recusa é agora a do original e é comparada: o ciclo, a pergunta noutro shard, empatada com outra, de uma unit e de uma stage isolada são checks |
| Ao nomear a unit de uma stage que corre por unit apareceu o defeito de `is_per_unit` | Corrigido à parte (acima) |

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `log/decision/summary*` e `log/answer/summary*` | @@SQ@@ |
| Diferencial, tudo | @@DIFF@@ |
| Repetição da gravação | @@REC@@ |
| Testes | 1 de CLI marcado novo (o ficheiro de perguntas: um link, um link para nada, atrás de um link, um segundo nome, uma pasta; e o ficheiro como deve ser, gravado com o seu nome). Os dois testes marcados de `decision` e `answer` deixam de listar o que passou a ser do original |
| Mutação | 59 mutantes (as duas escolhas, a pergunta aberta, o ficheiro que a recusa nomeia, os comandos, as três frases, a evidência da pergunta, os dois verbos): 55 apanhados (51 à primeira, 4 depois de 6 checks escritos para eles), 2 equivalentes anotados (a pasta de uma unit lida pelo seu próprio nome, que só um sistema de ficheiros que ignora maiúsculas distingue; e uma resposta com checkpoint, que até à 2c-iii é recusada antes) e 2 em código que saiu |
| `cargo test` | @@TESTS@@ |

### A jornada

O original tem uma jornada sem modelo, `tests/harness/scope-run.ts` (1 798 linhas), em três papéis separados: um host que corre o CLI como o skill o corre e dispara os hooks que o harness dispararia; um agente de substituição que faz o que cada diretiva pede (escreve artefactos pequenos, corre os comandos que a diretiva nomeia tal como estão, e acaba o turno em cada pergunta ou gate); e um guião da pessoa, o único que responde, pelo hook do turno humano. O diferencial ganha o mesmo (`tools/oracle/journey.py`), agnóstico do motor: cada lado é conduzido nos seus próprios nomes, e o que o original diz é traduzido por `names.py`, como em tudo o resto.

A jornada existe a partir da fatia 7 e mede uma coisa: **até onde os dois motores dizem o mesmo**. Onde Focus ainda recusa pelo nome, a comparação pára e diz em que passo; cada fatia seguinte empurra essa frente, e o marco fecha quando chega a `done` em todos os scopes. As árvores de cada passo são gravadas e repetidas em `cargo test`, como as do diferencial.

Dois limites conhecidos. O agente de substituição do original dispara `continue-workflow` e `deliver-stage-rules`, que são de M6: em M5 a jornada dispara, nos dois lados, só os hooks que Focus tem. E o que é aleatório numa diretiva (os ids de pergunta, os tokens de entrega das regras, os ids de tentativa) não se pode semear no original: compara-se pela posição em que cada id aparece pela primeira vez, não pelo valor.

### Riscos próprios de M5

| Risco | Mitigação |
| --- | --- |
| `routeNext` é uma função de 1 876 linhas com sessenta e seis `if` de topo: a ordem das perguntas é comportamento | Portada na ordem do original; o fuzz ganha combinações de flags de `next` sobre estados gerados |
| As diretivas levam prosa (a narração, as mensagens, as notas para o agente) com invocações do engine dentro | `names.py` traduz as do original; cada normalizador novo é declarado e contado, como os de M4 |
| `next` e `report` compõem as outras ferramentas por subprocesso e repetem o stderr delas tal como veio | Em Focus são chamadas de função: o texto repetido tem de sair igual, incluindo o formato do erro de cada ferramenta. O diferencial compara a mensagem inteira |
| Tempo: as perguntas expiram, `wait` espera | Relógio injetado (regra 7); o que só o relógio real decide compara-se ao vivo e não se grava, como os `status/awaiting*` |
| O peso das fixtures (R10) cresce com uma árvore por passo de jornada | Decide-se na fatia 7 com números: gravar todos os passos, ou só os que mudam de tipo de diretiva |

### O que M4 não levantou

A secção 14 marca como M4 o que depende de units; M4 fechou sem units, que são da fatia 5 deste marco. Mantêm-se recusados pelo nome até lá: os verbos de units de `state`, `skip --unit`, `jump reopen` e `execute --units`, e uma stage de Construction em `advance`. O que é de equipa (`park` e `unpark` com `Unit Ownership: team`, o gate de uma unit de equipa, o board no `status`) está adiado com `unit` (secção 11). As concessões que o turno humano guarda para um chat antes de o trabalho existir e as linhas devidas à pessoa, também marcadas M4, são da porta da frente (fatia 9) e da emissão (fatia 7).
