# Finalização e validação da integração RunPod — TSCDesk

## Confirmado na auditoria inicial

- O projeto possui janela nativa Rust (`tao` + `wry`), engine de IA, ponte IPC e interface HTML/JavaScript próprios.
- Já havia catálogo de provedores, configurações de modelos, chat, execução de ferramentas e política de permissões.
- Credenciais usam criptografia AES-256-GCM com chave protegida pelo cofre do sistema.
- Já existiam instalador e launcher Linux em `scripts/`; faltava um ponto de entrada na raiz para instalar e abrir.
- Já havia uma entrada RunPod e um arquivo de adaptador. A etapa anterior, porém, **não havia validado compilação**, usava uma URL incompatível com o endpoint Queue-based confirmado e anunciava SSE/tool calling sem comprovação. Esses defeitos exigiram correções localizadas, não uma reconstrução dos provedores.

## Escopo desta etapa

- Arquivo `iniciar-chromeos.sh` na raiz: instala na primeira execução, reutiliza instalação nas seguintes e abre TSCDesk; `--instalar` atualiza explicitamente e `--check` somente diagnostica.
- Configuração RunPod na interface existente: Endpoint ID, API Key protegida e modelo, com ações explícitas Salvar e Testar conexão.
- API Queue-based `/v2`, submissão única, acompanhamento de job, prazos e cancelamento quando possível; consulta de saúde sem inferência.
- `FINAL`/`INICIAR` como comandos locais isolados e controle visual, com estado de pausa persistido e proteção contra respostas tardias.
- Correções de sigilo de logs/estado público, mantendo o mecanismo de credenciais existente.
- Testes locais com servidores e arquivos simulados, sem uso do endpoint real.

## Restrições respeitadas

Na etapa de implementação e validação descrita neste relatório, não houve publicação, push, merge, deploy, alteração das credenciais normais, modificação do painel RunPod ou teste pago. Dependências de validação Rust/GTK foram utilizadas em um container local de teste; não foram instaladas dentro do TSCDesk como dependências de OpenCode.

## Uso e limitações

Veja [configuração, contratos e cancelamento](runpod.md) e [instalação no ChromeOS](../README.md). Em resumo:

```bash
# Dentro do checkout, no Terminal Linux do Chromebook:
bash iniciar-chromeos.sh

# Para instalar esta versão se já havia uma versão instalada:
bash iniciar-chromeos.sh --instalar
```

Abra Settings → Providers → RunPod Serverless. Cole o Endpoint ID do painel, a API Key no campo de senha e o nome exato do modelo (`qwen2.5-coder:7b`). Salvar vazio no campo de chave preserva a chave já armazenada; Testar conexão consulta saúde/configuração e não valida inferência.

`FINAL` mantém a IDE aberta e para o uso de IA por ela; `INICIAR` permite novos pedidos sem reexecutar a tarefa anterior. Nenhum deles garante GPU fisicamente desligada ou custo zero. Cancelamento depende do Job ID, da API remota e dos limites de chamadas já em trânsito. A imagem `latest` e a presença do modelo precisam de validação real autorizada. Streaming/tool calling RunPod, ChromeOS real, ARM64 e builds nativos macOS/Windows não devem ser apresentados como testados quando não executados.

## Resultados de testes

Validação final em container Debian 12 x86_64, Rust stable, GTK 3 e WebKitGTK 4.1; execução Rust com `--network none` e Cargo `--locked --offline`, impedindo chamadas externas:

| Verificação | Resultado |
| --- | --- |
| `cargo check --workspace --all-targets` | Passou |
| `cargo build --workspace` (debug, incluindo executável e helpers) | Passou |
| `cargo test -p quixi-engine --lib -- --test-threads=1` | 174 passaram |
| `cargo test -p quixi-ipc --lib -- --test-threads=1` | 15 passaram |
| `cargo test -p quixi-app --bin QuixiDesk` | 1 passou |
| `node --test tests/ui/*.test.cjs` | 107 passaram |
| Fixtures de instalação/launcher no WSL | 26 passaram; 2 casos exclusivos de root ignorados na execução normal |
| Casos de rejeição de root, executados separadamente como root | 2 passaram |
| ShellCheck dos três scripts e sintaxe Bash | Passaram |
| Auditoria de recursos distribuídos | Passou |
| `git diff --check` | Passou |

Na primeira execução Rust faltava `git` no container, causando uma falha de regressão existente; foi instalado somente no ambiente descartável de validação e a suíte passou. Dois testes IPC dependiam do checkout de desenvolvimento e de OAuth sem endpoint simulado: foram corrigidos para usar workspace mínimo e servidor local, mantendo as asserções e o prazo original.

Ainda existem warnings de compilação (imports/métodos não usados e `unused_mut`); não foi executado Clippy com warnings como erros. Build release completo, janela real em Crostini, hardware ARM64, Windows e macOS não foram executados nesta etapa. Nenhuma inferência real RunPod foi feita.

## Arquivos criados/alterados nesta etapa

- `iniciar-chromeos.sh` — novo ponto de entrada na raiz.
- `crates/quixi-engine/src/agent_control.rs` — novo controle persistente de admissão/pausa.
- `crates/quixi-engine/src/provider_tool_runpod.rs` — corrigido adaptador existente.
- `crates/quixi-engine/src/providers.rs` — configuração, criptografia reutilizada, máscaras e roteamento de modelos compartilhados.
- `crates/quixi-engine/src/lib.rs`, `chat_workflows.rs`, `context.rs`, `permissions.rs`, `subagents.rs`, `jobs.rs`, `scheduling.rs`, `workflows.rs` — integração dos comandos e guards contra trabalho tardio.
- `crates/quixi-ipc/src/lib.rs` — rotas de configuração/controle e fixtures offline.
- `crates/quixi-app/src/main.rs` — diagnóstico sem registrar corpos potencialmente sensíveis.
- `resources/ui/app.js`, `resources/ui/app.css` — configuração RunPod, indicador e controles.
- `tests/ui/runpod-agent-controls.test.cjs`, `tests/install/test_chromeos_start.py` — novas regressões.
- `README.md`, `docs/chromeos.md`, `docs/runpod.md`, `docs/runpod-validacao.md` — instruções e relatório.

As alterações Linux anteriores, incluindo `scripts/install-linux.sh` e `scripts/launch-linux.sh`, foram preservadas. A publicação no GitHub é uma etapa separada, realizada somente após autorização do responsável. Para obter e instalar o código atualizado no Chromebook, siga as instruções de clone ou atualização no [README](../README.md); após baixar o código, execute `bash iniciar-chromeos.sh --instalar` para atualizar uma instalação já existente.
