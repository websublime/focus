# Focus: plano de arquitetura e migração

O projeto chamava-se Sulca até 2026-10-09; o workspace, os crates e o binário passaram a `focus`.

Estado em 2026-10-09, com os marcos M0 e M1 fechados. Referência: `awslabs/aidlc-workflows` no commit `2a555f7` (v2.11.0, licença MIT-0). Focus é um fork congelado nesse commit; o que o original publicar depois não é seguido.

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

O que existe hoje são os dois primeiros marcos. M0 deu o compile do grafo. M1 fechou o grafo: seleção de plugins e scopes compostos no compile, escrita no sítio sob um lock de workspace próprio, e todos os verbos de `focus engine graph` (`topo`, `cycles`, `scope`, `artifacts`, `producers`, `consumers`, `validate-scope`, `validate-grid`, `ars`, `resolve`, `export`). Estado, auditoria, guards, hooks e orchestrator ainda não existem.

Três conclusões condicionam tudo o resto:

1. **O port é guiado por um oráculo, não por leitura.** O engine original corre aqui (Bun), por isso cada marco termina com as mesmas entradas a passar pelos dois motores e as respostas comparadas como texto. Em M1: 1 825 comandos executados nos dois motores sobre árvores de projeto alteradas, com 0 diferenças fora de 16 verificações que são desvios deliberados (8 causas), e 208 030 casos de função com 0 diferenças (secção 10).
2. **`aidlc-lib.ts` não se porta de cima para baixo.** São 42 056 linhas importadas por 73 dos outros 110 ficheiros. Reparte-se por crates, à medida que cada marco precisa de uma fatia.
3. **O maior risco técnico é o Markdown, e já tem o primeiro consumidor.** O engine interpreta Markdown com o `Bun.markdown`. Em M1, `validate-grid` precisou de saber se um ficheiro de memória declara uma Guard Policy, o que depende de que linhas são visíveis. Focus resolve os ficheiros em que isso se decide linha a linha e recusa os restantes (secção 10); o parser a sério é de M2.

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
focus-cli ✔ ───────────► focus-hooks ──► focus-orchestrate ──► focus-state ──► focus-audit ◐
    │                        │                  │                   │               │
    │                        ▼                  ▼                   ▼               ▼
    ├──► focus-project   focus-guards      focus-sensors       focus-graph ✔   focus-workspace ✔
    │                        │                  │                   │               │
    └────────────────────────┴──────────────────┴───────────────────┴───────────────┘
                                                │
                     focus-harness ✔    focus-markdown    focus-frontmatter ✔    focus-js ✔
```

✔ assinala os crates que já existem; ◐ os que existem em parte.

| Crate | Responsabilidade | Estado |
| --- | --- | --- |
| `focus-js` | Semântica JS: espaço em branco, ordenações e colação, ordem de chaves, `JSON.parse` e `JSON.stringify`, impressão e arredondamento de números, regex ECMAScript | Existe |
| `focus-frontmatter` | Leitores de frontmatter (subconjunto de YAML do original) | Existe |
| `focus-harness` | Descritores dos três harnesses; mais tarde, os adapters de payload | Descritores existem |
| `focus-graph` | Schemas, compile do grafo e do grid, scopes compostos, consultas, validação de planos, ARS | Completo para o CLI de `graph` |
| `focus-workspace` | Projeto e harness de um comando, caminhos, escrita atómica, lock do workspace, recusa de links; mais tarde, settings, spaces e intents | Existe; settings em M2 |
| `focus-markdown` | Estrutura de blocos Markdown (`markdownBlocks`, `visibleMarkdownLines`) | M2 |
| `focus-audit` | Shards de auditoria, registo de eventos, runtime graph | Formato existe (M2, fatia 1) |
| `focus-state` | Ficheiro de estado, máquinas de workflow, fase e stage, criação de intents | M3 |
| `focus-guards` | Lexer de shell, integridade de runtime, fences, decisões de guard | M4 |
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

**M2. Auditoria, Markdown e settings.**
- Settings (`focus.settings.json` nas três camadas, switches gravados). Fecha o limite de M1 nos kill switches.
- Leitura e escrita de shards, registo dos 117 eventos, compile do runtime graph. Este compile é determinístico mas não é puro: lê o audit log, o ficheiro de estado, o stage graph e os `memory.md`, e acrescenta linhas `MEMORY_EMPTY` à auditoria.
- `focus-markdown`, com a decisão de parser tomada por medição (risco R1). Substitui o leitor por linhas de `guard_memory` e deixa de haver ficheiros de memória recusados.
- Critério: para os mesmos shards, `runtime-graph.json` byte a byte igual; linhas de auditoria iguais com relógio injetado.
- Entra em fatias, cada uma um pull request verificado por si (secção 13).

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

1. **Corpus de oráculo.** `gen_cases.py` gera casos com seed; `driver.ts` responde com o código do original; `focus dev oracle` responde com o de Focus; comparam-se como texto, incluindo a ordem das chaves. Treze tipos de caso: frontmatter de stage, sensor e regra, os dois leitores primitivos, colação, números, ciclos, registos de scopes compostos, fold-back, settings de proposta e Guard Policy em memória. Um corpus de 4 454 casos fica em `fixtures/oracle/`.
2. **Diferencial ao vivo.** `differential.py` constrói um projeto Claude Code a partir da distribuição do original, altera-o de uma forma específica, dá uma cópia a cada motor, corre o mesmo comando nos dois e compara o que um chamador observa: código de saída, stdout, stderr e, num compile, os ficheiros que ficam em disco. Com `--fuzz N` acrescenta propostas e pontuações aleatórias.
3. **Diferencial gravado.** Com `--record`, o mesmo script guarda o que o original fez em cada verificação (a árvore de partida, o comando, o resultado) em `fixtures/recorded/`. Um teste reconstrói cada árvore e exige de Focus o mesmo resultado.
4. **Goldens.** Os ficheiros compilados pelo packager original, em `fixtures/upstream/golden/`.

`cargo test` repete o corpus, o diferencial gravado e os goldens sem Bun e sem clone do original. `refresh.sh` regenera tudo a partir de um clone.

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

**R1. Markdown.** Continua a ser o maior risco, e deixou de ser hipotético: `validate-grid` já depende dele (secção 10). O original interpreta Markdown com `Bun.markdown` e tem código que compensa comportamentos de versões concretas do Bun (comentários em `aidlc-lib.ts`, linhas 40572 e 40957). Sensores e a verificação de autoridade de review dependem desse resultado. Um parser Rust (`pulldown-cmark`, `comrak`) vai divergir em casos-limite. Mitigação: antes de escolher, medir cada candidato contra `markdown-goldens/corpus.json` e contra todo o Markdown de `core/`; onde a divergência for um defeito do Bun, seguir a especificação e registar o desvio.

**R2. `aidlc-lib.ts`.** Não há fronteiras internas a seguir. Mitigação: em cada marco, extrair primeiro a lista de funções de `lib` que os módulos do marco importam (os imports são nomeados) e atribuí-las a um crate antes de portar.

**R3. `localeCompare`.** Resolvido para ASCII. O original chama `localeCompare` em 68 pontos: 64 com a locale por omissão do processo, 4 com `"en"` e ordenação numérica. Em M1 a ordem dos scopes passou a chegar à saída (`nearest_stock`), e um nome de scope pode ter maiúsculas e `_ . + @`. `focus-js` implementa agora a colação raiz do CLDR para ASCII imprimível, em dois níveis (caracteres sem distinção de caixa, depois minúscula antes de maiúscula), e coincide com o Bun em 528 491 pares, incluindo todos os pares de um carácter. Fora de ASCII continua a usar ordem de code units, que não é a da ICU. O único ponto portado onde isso se pode ver é a ordem dos agentes no `export`: o nome de um agente não tem gramática, e um nome com acentos ficaria noutra posição.

**R4. Alvo em movimento.** Fechado por decisão: Focus é um fork congelado no pin.

**R5. Coexistência.** Fechado por decisão: o lock é de Focus e não é compatível com o do original. Um projeto é conduzido por um motor ou pelo outro. Fica uma consequência a tratar em M2 e M3: os formatos de estado e auditoria continuam a ser os do original (é o que o Markdown reutilizado descreve), mas deixam de ter de ser escritos de forma que o engine original os aceite a meio de um workflow.

**R6. Números.** Resolvido. `focus-js::num` imprime e arredonda como o JavaScript (`String`, `toFixed`, `Number`, `Math.round`), e `JSON.stringify` usa-o. O corpus alargado encontrou um defeito na primeira passagem: quando um double fica exatamente a meio entre dois candidatos mais curtos, o JavaScript escolhe o de último dígito par e o formatador do Rust o de cima (`2126906910518472.25` imprime `...472.2` num e `...472.3` no outro). Corrigido; 5 casos em 124 818 antes, 0 em 208 030 depois.

**R7. Regex fora do comum.** O original usa lookbehind (9 ocorrências) e a flag `v` com `\p{RGI_Emoji}` (2). O `regress` compila ambos e aplica-os a uma amostra com o resultado esperado (há um teste em `focus-js`); ainda não passaram por um corpus diferencial.

**R8. Plataformas.** Medido pela primeira vez em 2026-10-09, no CI do primeiro pull request: os lints e os testes passam em Linux, macOS e Windows com a toolchain fixada (1.97.0), e o workspace compila com a 1.89. O lock usa `std::fs::File::lock` (`flock` em Linux e macOS, `LockFileEx` em Windows), sem FFI, e os seus testes correm nas três. Fica por medir o que só corre em Unix: o diferencial gravado (as mensagens trazem caminhos com o separador da plataforma) e os testes de links. A repetição do rename em Windows, quando outro processo tem o ficheiro aberto, continua sem um teste que a force. A primeira execução no `main` falhou nos lints por outra razão: o CI instalava a `stable` do dia, mais recente do que a usada aqui. A toolchain passou a estar fixada em `rust-toolchain.toml`.

**R9. Nomes no conteúdo reutilizado.** Fechado pela decisão 10. Focus adotou o Markdown como seu e renomeou-o: `aidlc` passou a `focus` em nomes de ficheiros, de diretórios e no texto, com a mesma substituição que as ferramentas de paridade aplicam (secção 7). Ficam dois efeitos a vigiar. Primeiro, os dois nomes ordenam de forma diferente em relação a outros (`aidlc` < `bravo` < `focus`): uma árvore com um plugin ou um ficheiro cujo nome caia entre os dois pode ser percorrida noutra ordem por cada motor, e duas mensagens de erro dependem da ordem. Nenhuma árvore do diferencial cai aí, e o diferencial mostrava-o. Segundo, a parte do Markdown que M1 ainda não lê (skills, knowledge, o texto dos harnesses) entra em M8 pelo mesmo mapa; as 408 invocações `{{INVOKE}}` e as 48 por caminho de ferramenta TS precisam de mais do que o mapa, e já estavam no âmbito de M8.

**R10. Peso das fixtures.** `fixtures/` tem 7,7 MB (corpus 2,2; diferencial gravado 3,7; referência 1,9). É o preço de `cargo test` provar paridade sem Bun. Se pesar no repositório, o diferencial gravado comprime bem ou pode viver fora.

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
| `resolve` sem `--stdout` | Escreve `plan.json` no registo do intent ativo | Recusa, a não ser que `FOCUS_PLAN_PATH` indique um ficheiro |
| `export --check` | Compara com uma fixture num caminho relativo ao repositório do original | `--check <ficheiro>` |

### Limites de M1

O que M1 não faz, e o que acontece quando lá se chega:

1. **Guard Policy em memória lida por linhas.** O original decide que linhas de um ficheiro de memória são visíveis com um parser CommonMark completo. `focus-graph::guard_memory` lê os ficheiros em que isso se decide linha a linha: fora de comentários que abrem no início de uma linha, não pode haver fences, crases desemparelhadas ou escapadas, nem `<` fora de um span de código; e a secção `## Guard Policy` não pode ter linhas indentadas quatro espaços ou mais. Os três ficheiros que o original distribui cabem. Um ficheiro fora disso faz `validate-grid` parar com erro, a nomear o ficheiro e a linha, quando a proposta é `relaxed` ou `off`; uma proposta `strict` não consulta a memória. Dentro do domínio, 45 035 casos gerados deram a resposta do original.
2. **Kill switches só pelo ambiente.** `validate-grid` avisa quando `FOCUS_DISABLE_SENSORS` e semelhantes forçam uma cerimónia a desligado. Um switch gravado em `focus.settings.json` não é visto: falta um aviso, o veredicto não muda. Fecha em M2.
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
2. **Peso das fixtures (R10).** 7,7 MB no repositório, ou diferencial gravado fora dele.
3. **O nome do método na prosa.** O Markdown reutilizado diz "AI-DLC" quando fala do método (por exemplo, "AI-DLC workflow"). Não mexi: é o nome de uma metodologia, não um identificador, e trocá-lo por substituição de texto estraga a gramática ("an AI-DLC" passaria a "an Focus"). Se o queres fora, é uma edição de conteúdo à mão, que cabe em M8 com o resto do Markdown.

## 12. Como correr

```sh
cargo test                          # corpus, diferencial gravado e goldens, sem Bun
cargo build --release

cd um/projeto/com/harness/instalado
focus engine graph compile          # recompila no sítio, sob lock
focus engine graph validate-scope feature
focus engine graph --help

# Com Bun e um clone do original no commit do PIN:
tools/oracle/refresh.sh ../aidlc-workflows                          # regenera fixtures e grava o diferencial
python3 tools/oracle/differential.py ../aidlc-workflows --fuzz 600  # os dois motores lado a lado
```

## 13. Estado de M2

M2 está em curso. O arranque confirmou o que R2 previa: os módulos de M2 não são folhas. `aidlc-audit.ts` importa 35 nomes de `aidlc-lib.ts` e `aidlc-runtime.ts` importa 27, e atrás deles estão a resolução do registo de um intent, o lock de auditoria, bolts, worktrees e a leitura do ficheiro de estado. Por isso M2 entra de baixo para cima, em seis fatias; a segunda partiu-se em duas.

| Fatia | O que traz | Como se verifica | Estado |
| --- | --- | --- | --- |
| 1. Formato do registo | Os 117 tipos de evento e os seus títulos, validação e bloco de uma entrada, leitores de blocos e de campos, timestamp. Crate `focus-audit` | Corpus de oráculo | Feita |
| 2a. Qual é o registo | Spaces, intents, registo de intents, cursor do intent ativo, binding de sessão e a seleção do workflow, em `focus-workspace` | Corpus: projetos descritos como conjuntos de ficheiros | Feita |
| 2b. O shard | Identidade do clone (`focus/.focus-clone-id`), nome e caminho do shard (`auditFilePath`, `auditShardName`), listagem e leitura de todos os shards de um registo (`auditShards`, `readAuditShardEvents`, `readAllAuditShards`), cópias lidas uma só vez | Corpus: projetos em disco e conjuntos de shards | Feita |
| 3. Escrita e leitura em disco | Append sob lock de auditoria, com as verificações de links e de identidade do ficheiro; `focus engine audit append`, `append-batch` e `history`; relógio injetável | Diferencial de comandos: shards iguais a menos do timestamp | Por fazer |
| 4. Settings | `focus.settings.json` nas três camadas e os switches gravados. Fecha o limite 2 de M1 | Corpus e diferencial de `validate-grid` | Por fazer |
| 5. Markdown | `focus-markdown`, com o parser escolhido por medição contra os 391 documentos de `markdown-goldens/corpus.json` (R1). Fecha o limite 1 de M1 | Corpus próprio e os goldens do original | Por fazer |
| 6. Runtime graph | `runtime-graph.json` a partir dos shards, do ficheiro de estado e dos `memory.md` | `runtime-graph.json` byte a byte igual | Por fazer |

A fatia 6 lê o ficheiro de estado (`readStateFile`, `parseCheckboxes`, `parseStateStageSuffixes`), que o plano tinha em M3, e os registos de bolts e worktrees, que estão adiados. Porta-se só o leitor de estado de que o compile precisa; o resto do ficheiro de estado continua em M3. Se a fatia de leitura se revelar inseparável das transições, a fatia 6 passa para depois do arranque de M3 e M2 fecha com as cinco primeiras.

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
