# Focus: plano de arquitetura e migração

O projeto chamava-se Sulca até 2026-10-09; o workspace, os crates e o binário passaram a `focus`.

Estado em 2026-10-13, com os marcos M0, M1 e M2 fechados (secção 13 para o último). Referência: `awslabs/aidlc-workflows` no commit `2a555f7` (v2.11.0, licença MIT-0). Focus é um fork congelado nesse commit; o que o original publicar depois não é seguido.

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
| `focus-guards` | Lexer de shell, integridade de runtime, fences, decisões de guard | Existe a resolução da Guard Policy, as fences e a decisão de uma fence (M3, fatia 1); o resto é M4 |
| `focus-hooks` | Os corpos dos hooks e o dispatcher | M4 e M6 |
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
| `focus-orchestrate` | `orchestrate` (16 133), `log` (4 486), `unit` (3 769), `directive` (2 976), `review-brief` (2 365), `construction-checkpoints` (1 618), `bolt` (1 511), `learnings` (1 252), `rules-held` (595), `includes` (481), `inline-context` (398), `steering` (128) | M5 |
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

**M3. Estado e criação de intents.**
- Ficheiro de estado, transições (`advance`, `approve`, `reject`, `skip`, `gate-start`, `park`), `intent-create`, `jump`, `validity`, `--status`.
- Critério: uma sequência de comandos deixa o mesmo ficheiro de estado e os mesmos shards nos dois motores, a menos do mapa de nomes.

**M4. Guards e hooks observadores.**
- O lexer de shell `review-freeze-command.ts` (2 083 linhas, só importa built-ins do Node); dependem dele dois guards e `runtime-integrity`.
- `runtime-integrity`, fences, os quatro guards de `PreToolUse` (um só processo, o grupo `guard-tool-call`), e os hooks que só registam.
- Critério: um corpus de payloads de hook dá o mesmo código de saída, stdout, stderr e linhas de auditoria.

**M5. Orchestrator.**
- `next` e `report`, diretivas, plan approval, testing posture, review brief, checkpoints de Construction em série.
- Critério: uma jornada completa produz a mesma sequência de diretivas JSON.

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

Tamanho: M5 é o maior bloco (35 712 linhas atribuídas a `focus-orchestrate`, mais a fatia de `lib` e parte de `utility`). As 21 335 de `focus-project` são um limite superior largo: grande parte de `init` trata dos sete harnesses e da instalação nativa. M0 cobriu, pela minha soma dos intervalos de linhas portados, cerca de 3 000 linhas de TypeScript, e M1 cerca de 2 700; não há um mapa função a função que o confirme. Não dou estimativa em tempo: os dois marcos feitos são os mais autocontidos, e a partir de M2 o código lê e escreve estado partilhado.

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

**R7. Regex fora do comum.** O original usa lookbehind (9 ocorrências) e a flag `v` com `\p{RGI_Emoji}` (2). O `regress` compila ambos e aplica-os a uma amostra com o resultado esperado (há um teste em `focus-js`); ainda não passaram por um corpus diferencial. A flag `i` passou: o `regress` compara caracteres de outra maneira que o JavaScript em quatro sítios, `focus-js` corrige-o e recusa o padrão que a correção não cobre (secção 13, "Entre a fatia 5 e a 6").

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
- Dependências externas diretas: `serde`, `serde_json` e `regress`; em testes, `sha2`.

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
| Sessões registadas por processo (`focus/.focus-sessions/pids/`) sem sessão nomeada | Percorre a árvore de processos para saber que conversa chama | Recusa e pede a sessão ou `--space` e `--intent`. Só acontece depois de haver hooks de sessão, que são de M4 |

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
| `> [Answer]:` seguido de `> ---` | Definição e regra, e `ANSWER` na lista de rótulos definidos | Título setext, como no CommonMark |
| `<!x y>` (declaração com letra minúscula) | Parágrafo com uma etiqueta; a linha fica visível, e o que vier a seguir até ao próximo `>` também | Bloco HTML (CommonMark 0.31, condição 4) |
| `[Answer]: <!--<!---->` | Definição | Parágrafo: um destino entre `<>` não pode conter `<` |
| Span de código que fecha no início de uma linha | Acaba o span uma linha antes; a última fica visível | O span cobre a linha onde fecha |
| Span aberto depois de uma crase escapada | Não regista o span; as linhas que cobre ficam visíveis | A crase escapada é texto e o span começa na seguinte |
| Etiqueta indentada quatro colunas sob um parágrafo | Não a regista, e dá um span que continue para essa linha uma coluna antes | Regista-a onde está |
| Linha só com um espaço que não é espaço nem tabulação (um espaço não separável) | Em branco | Parágrafo: para o CommonMark é texto |

Em todos, o árbitro lê como Focus. Os três que mudam o que `validate-grid` decide (o documento longo, a crase escapada e a declaração em minúsculas) estão também no diferencial, com nome, e em testes de `guard_memory.rs`.

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
| `resolve` num projeto com sessões registadas por processo | Percorre a árvore de processos para saber que conversa chama, e escreve no registo dela | Recusa escrever; com `--stdout` imprime o plano, que não precisa de registo |
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
| Sessões registadas por processo; worktree delegada; claim de unit | Resolve cada um | Recusa, como no comando `audit` (os mesmos três desvios, pelas mesmas funções) |
| `stage-graph.json` que não tem a forma que o compile escreve: um stage que o workflow usou sem fase em texto; num resumo, uma fase que não é texto; uma linha `null`; um ficheiro que não é uma lista | Falha com o que o runtime diz do valor (`The "paths[1]" property must be of type string, got undefined`, `null is not an object (evaluating 's.enabled')`) ou, no resumo, conta o stage debaixo do valor escrito como texto (`7`) | Diz que o stage graph não é o que o compile escreveu e como o compilar de novo. O que o compile podia ter escrito lê-se como no original: um slug em duas linhas é a segunda; um stage sem fase que o workflow não usou não impede nada; num resumo, sem fase é `unknown` |
| Fase com nome de membro de objeto (`constructor`) num resumo | Guarda as fases num objeto simples: conta o stage no valor herdado e não lista a fase | Lista-a como qualquer outra |
| JSON que o `JSON.parse` lê e o leitor de Focus não guarda: um número que não cabe num double (`1e400`), metade de um par de substituição escrita com escape (`"\ud800"`), mais de 127 níveis | Lê | Diz que o ficheiro não é JSON que leia. Vale para todo o JSON que Focus lê e está fixado em `focus-js` |
| O diretório do workspace é um link | Deixa de fora os shards atrás do link, não vê workflow nenhum e escreve um grafo vazio através dele | Não toma o lock nem escreve através de um link, e diz qual é |
| Barra invertida no caminho do projeto | O leitor de shards toma-a por separador, não encontra nenhum e compila um grafo sem stages | É um carácter do nome, e a auditoria é lida |

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

M3 está em curso. O que o plano chamava "estado e criação de intents" é, no original, `aidlc-state.ts` (8 867 linhas, 33 subcomandos), `aidlc-jump.ts`, `aidlc-validity.ts` e a parte de `aidlc-utility.ts` que cria intents e mostra o estado; atrás deles, outra vez, está `aidlc-lib.ts`: a resolução da Guard Policy, as fences e a autoridade de uma ação, que as transições de stage consultam antes de mexer no ficheiro de estado. Entra de baixo para cima, como M2.

| Fatia | O que traz | Como se verifica | Estado |
| --- | --- | --- | --- |
| 1. Guard Policy e fences | `focus-guards`: a resolução da política de um workflow (a linha do estado, o default do scope, as camadas de memória), as linhas `Guards Off` e `Guards On`, os kill switches, o que cada política baixa, a decisão de uma fence e a autoridade de uma ação sem marcadores de hooks | Corpus | Feita |
| 2. O ficheiro de estado como dados | `focus-state`: campos, checkboxes, listas de refs, progresso de fase; ler e escrever o ficheiro (barreira `writeStateFile`, ficheiro só de leitura); `focus engine state get`, `set`, `checkbox` e `count`, com a porta das transições e o `ERROR_LOGGED` de cada recusa | Corpus e diferencial | Feita |
| 3. Criação de intents | `intent-create`: o registo, o cursor, o ficheiro de estado a partir do template; antes dele, o scanner do workspace que lhe classifica o projeto (`detect`) | Diferencial | Feita (3a `detect`, 3b `intent-create`) |
| 4. Transições de stage | `advance`, `gate-start`, `approve`, `reject`, `revise`, `skip`, `resume`, `finalize`, `complete-workflow`, em série e sem units | Diferencial e fuzz de sequências | |
| 5. Parar, saltar, validar | `park`, `unpark`, `jump`, `validity`, `--status` | Diferencial | |

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
| Sessões por processo; worktree delegada | Resolve | Recusa, como nos outros comandos |
| `FOCUS_STATE_TRANSITION_OWNER` em Windows | Compara com o pid do pai | Recusa: Focus não lê o pid do pai aí ainda. Em Linux e macOS compara |

**O que a fatia 2 não tem.** O registo das escritas do estado (`state-writes.json`), que o orchestrator lê para dizer o que pôs um passo fora de data; entra com ele (M5). O lock é o do workspace, não um por intent (decisão 3, R5).

### Fatia 3a: o scanner do workspace, e o comando `utility`

A criação de um intent começa por classificar o projeto: Greenfield ou Brownfield, e com que linguagens, frameworks e sistema de build. O original fá-lo com regras sobre os ficheiros que estão lá (`detectWorkspace`, em `aidlc-utility.ts`): conta ficheiros por extensão na raiz e, até seis níveis, em `src`, `app`, `lib`, `pages`, `components` e `tests`; lê alguns manifestos (`package.json`, `Gemfile`, `pom.xml`, `pyproject.toml`); e, quando a raiz não mostra código, desce até três níveis à procura de projetos aninhados, nomeando também cada repositório git que passa. Um `.gitmodules` com uma entrada válida declara código, mesmo por buscar. O que depende da ordem em que o sistema lista um diretório (qual das linguagens empatadas vem primeiro) segue essa ordem, como no original; o passeio aninhado ordena as entradas, como no original.

`focus engine utility` é o `aidlc-utility.ts`: `detect [--json]` imprime o scan e onde vivem os scopes (para o composer não ter de o derivar), `now` imprime a hora do engine. Os outros 32 verbos são recusados pelo nome, cada um com uma frase que diz o que faz; `intent-create` entra na fatia 3b. Os argumentos leem-se como o original os lê (`parseArgs`: tudo é flag, `--json` sozinho vale `true`), e uma recusa deixa o `ERROR_LOGGED` ao lado do registo que `--intent` ou `--space` nomeiam, senão do ativo. A recusa e a linha que deixa passaram a ser de `focus-cli::report`, partilhadas com `state`.

**O que existe.** `focus-workspace::scan` (`detect`, `parse_gitmodules`, os ficheiros que o engine escreveu inteiros, lidos do descritor de projeção de cada harness instalado), `focus-cli::utility`, `focus-cli::report`, `Parsed` em `focus-cli::args`.

**Verificado.**

| Verificação | Resultado |
| --- | --- |
| Diferencial, `utility/*` | 133 verificações com nome: 120 iguais, 13 desvios, 0 diferentes. Cobrem cada extensão, cada framework e cada sistema de build, o limiar das linguagens secundárias, os diretórios excluídos, a profundidade, os projetos aninhados e a sua ordem, os repositórios git, o `.gitmodules` (entradas inválidas, CRLF, caminhos com `\`), os manifestos que são diretórios ou JSON inválido, os links (não gravados) e os ficheiros escritos inteiros pelo engine |
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
| Diferencial, `utility/create/*` | {CREATE} |
| Diferencial, `state/marker/*` | 7 verificações: 5 iguais (os marcadores de turno, nas duas ordens, com e sem `FOCUS_UNATTENDED`, e um marcador que é diretório), 2 desvios (o `active-directive.json`) |
| Diferencial completo | {FULL} |
| Mutações | {MUT} |
| `cargo test` | {TESTS} testes |

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
| `Source Baseline` na linha `WORKFLOW_STARTED`, e o ficheiro `baseline-<hash>.tsv` ao lado do registo | O passeio da atestação por todos os ficheiros fonte do projeto | Não escreve nenhum dos dois: é M5. O diferencial retira o campo e o ficheiro antes de comparar (`SHAPES`), e compara todo o resto da criação |
| Um observador (`FOCUS_STOP_HOOK_PROBE`, `FOCUS_ROUTE_CHECK`) que chega à criação | Pára no `writeFileAtomic` da baseline | Pára no append da primeira linha da auditoria; os dois deixam o diretório cunhado, o clone id e o título do shard |
| `--request <id>` | Vai buscar a pergunta do engine | Recusa: a loja de perguntas é M5 |
| O uso de `intent-create` | Oferece `--request` | Lista as flags que tem |
| Um projeto com o layout plano `focus-docs/` | A primeira criação migra-o para um registo | Recusa |
| As concessões que o hook de turno humano guarda para um chat (`*-at-creation-<sessão>.json`), e um bypass de presença gravado | Gasta-as | Recusa quando há uma para a sessão (M4) |
| Os blocos de `.gitignore` e `AGENTS.md` numa cópia em que o config nunca correu | Acrescenta-os na primeira criação | Não lhes toca: é do instalador (M6). O diferencial não compara os ficheiros da raiz; fica dito aqui |
| `--project-dir` sem instalação, sessões por processo, worktree delegada | Resolve | Recusa, como nos outros comandos |

**O que a fatia 3b não tem.** `reclassify`, `status`, os verbos `intent` (list, switch, archive, unarchive) e `space`: entram com as transições e o estado visível (fatias 4 e 5).

Fora de M3 e recusados pelo nome, como `fragment-fork` ficou em M2: os subcomandos de units, bolts e swarm (`unit`, `fork`, `merge`, `fold-unit-merge`, `refresh-unit-progress`, `sync-unit-scope-stage`, `set-unit-ownership`, `set-unit-gate-rhythm`), as políticas de Construction (`set-construction-*`, `set-skeleton-stance`), as práticas (`practices-event`, `practices-promote`), `reuse-artifact`, `lookup` e `acknowledge-compaction`.
