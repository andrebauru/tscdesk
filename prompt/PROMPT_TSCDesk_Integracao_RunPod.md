# PROMPT PARA OPENCODE: INTEGRAR RUNPOD SERVERLESS AO TSCDesk

## Papel e objetivo
Você é o agente de implementação trabalhando **no repositório da minha IDE independente, TsCDesk**. Sua tarefa é adicionar **RunPod Serverless como provedor de modelos de IA** para a própria IDE. **OpenCode é apenas a ferramenta que executará este prompt e editará o repositório. Não integre, instale, incorpore nem crie dependência do OpenCode dentro do TsCDesk.**

## Contexto confirmado
- Tenho um endpoint RunPod Serverless do tipo **Queue-based** já criado.
- Imagem configurada: `svenbrnn/runpod-ollama:latest` (terceiros).
- Variável configurada: `MODEL_NAME=qwen2.5-coder:7b`.
- GPU escolhida: 24 GB; `Active workers=0`; `Max workers=1`.
- O endpoint ainda **não teve inferência bem-sucedida comprovada**.
- O ID do endpoint e a chave API serão inseridos pelo usuário na configuração da IDE. Não os invente nem os grave no código.
- A IDE deve manter seus provedores e funções atuais; identifique-os no código antes de alterar qualquer coisa.

## Resultado esperado
Na interface de configuração de IA do TsCDesk, o usuário poderá escolher **RunPod Serverless**, informar Endpoint ID, API Key e modelo, salvar com segurança, testar conexão e utilizar o endpoint para conversar sobre código. Integre às funções de IA **já existentes** na IDE, sem reconstruir seu sistema de agentes. Se não houver infraestrutura de IA, implemente apenas o menor conjunto necessário e documente o que falta.

## 1. Auditoria obrigatória antes de codificar
1. Examine o repositório, README, dependências, estrutura de frontend/backend e arquitetura de provedores de IA.
2. Identifique o ponto real de extensão dos provedores, a interface de configuração, o armazenamento de segredos e o fluxo de chat/inferência.
3. Verifique se há núcleo Rust/Tauri e reutilize-o **somente se existir**. Preserve suporte Windows, Linux e macOS conforme o projeto.
4. Analise o código-fonte ou documentação efetiva da imagem `svenbrnn/runpod-ollama:latest` para descobrir as variáveis aceitas, payload de entrada, formato de saída e suporte a chat, histórico, ferramentas e streaming.
5. Se não conseguir verificar o contrato do worker, não invente campos. Implemente um cliente desacoplado, testes mockados e marque a inferência real como pendente de validação.
6. Antes de mudanças amplas, apresente resumo conciso dos arquivos e da estratégia de integração.

## 2. Provedor RunPod
Implemente um adaptador independente com configurações:
- `endpointId` (obrigatório), `apiKey` (segredo), `model` (padrão `qwen2.5-coder:7b`);
- tempo limite de conexão, espera de inicialização fria e inferência;
- modo de execução `auto`, `sync` ou `async`, de acordo com o contrato real;
- opções de temperatura e limite de tokens apenas se aceitas pelo worker;
- configuração persistente e seleção do provedor, sem afetar os existentes.

API-base: `https://api.runpod.ai/v2/{ENDPOINT_ID}`.
Verifique os contratos atuais antes de utilizar:
- `POST /runsync` para execução síncrona;
- `POST /run` para submissão assíncrona;
- `GET /status/{JOB_ID}` para acompanhar jobs;
- `POST /cancel/{JOB_ID}` quando disponível.

Autentique usando o esquema exigido pela API RunPod. Faça validação rigorosa do ID do endpoint e não permita URLs arbitrárias como destino padrão. Nunca exponha a chave em logs, mensagens de erro ou arquivos versionados.

## 3. Adaptação de mensagens
- Converta o histórico e as mensagens do TsCDesk para o **payload realmente aceito** pelo worker Ollama.
- Normalize a saída para o contrato interno de respostas da IDE.
- Preserve papéis, ordem, erros, códigos de status e metadados disponíveis.
- Não presuma que a API Queue-based é uma API OpenAI-compatible nem que possui `/v1/chat/completions`.
- Não crie proxy HTTP local se o backend da IDE puder consumir o RunPod diretamente.
- Só habilite tool calling e streaming depois de comprovar suporte real do worker e do modelo. Se não houver suporte, use resposta completa e modo chat; não anuncie recursos indisponíveis.

## 4. Experiência de usuário
Na configuração de provedores, adicione:
- RunPod Serverless como opção;
- campos Endpoint ID, API Key protegida e nome do modelo;
- botão **Salvar**;
- botão **Verificar configuração**, sem disparar GPU quando houver alternativa;
- botão separado **Testar inferência (pode gerar cobrança)**, com confirmação explícita;
- estados: desconectado, autenticando, em fila, iniciando worker, executando, concluído, falha e cancelado;
- mensagens claras para chave inválida, endpoint incorreto, worker indisponível, timeout, erro de modelo e resposta inválida.

O chat deve permanecer responsivo, permitir cancelamento e não duplicar requisições em reconexões ou rerenders.

## 5. Execução e custos
- Trate inicialização fria e fila sem bloquear a UI.
- Em jobs assíncronos, faça polling com backoff, prazo máximo e encerramento limpo.
- Ao cancelar, interrompa o polling e tente cancelar o job remoto se suportado.
- `Active workers=0` não garante custo zero: evite testes automáticos, retries infinitos e inicializações sem ação do usuário.
- Não afirme que a GPU desligou sem confirmação externa.
- Não exponha estimativas financeiras sem dados confiáveis.

## 6. Segurança
- Armazene a chave no mecanismo seguro já usado pela IDE; se inexistente, proponha uma solução adequada à plataforma, sem plaintext por padrão.
- Faça chamadas autenticadas no backend seguro, quando a arquitetura permitir.
- Remova tokens de logs, telemetria, erros e snapshots.
- Não transmita arquivos do workspace automaticamente: use apenas o contexto autorizado pelo usuário.
- Não altere `.env`, segredos ou configuração de outros provedores.
- Preserve as permissões atuais de execução de comandos e edição de arquivos.

## 7. Testes e critérios de aceitação
Crie testes mockados para autenticação, montagem do payload, conversão da resposta, estados de job, polling, cancelamento, timeout, falhas HTTP, proteção de segredos e regressão dos provedores atuais.

**Aceitação funcional:**
1. RunPod aparece como provedor configurável no TsCDesk.
2. Endpoint ID, API Key e modelo podem ser configurados e persistidos com segurança.
3. O fluxo de inferência usa o contrato real do worker.
4. O chat da IDE recebe e exibe uma resposta válida, após teste real autorizado.
5. Erros, fila, inicialização fria e cancelamento são tratados.
6. As funções existentes e os builds relevantes continuam funcionando.
7. Qualquer limitação de streaming, tools ou compatibilidade é exibida com precisão.

Nunca execute teste real cobrado sem solicitar minha autorização.

## 8. Sequência de execução
1. Audite o código e verifique o contrato do worker.
2. Mostre o plano mínimo de arquivos a alterar.
3. Implemente cliente, adaptador e configuração.
4. Conecte o provedor ao fluxo de IA existente.
5. Implemente estados, cancelamento e UX.
6. Execute testes automatizados locais sem cobrança.
7. Relate arquivos alterados, resultados, limitações e instruções para configurar o endpoint.
8. Solicite autorização antes de qualquer teste real com GPU.



## 9. Comandos de sessão: FINAL e INICIAR (obrigatório)

Implementar no **chat de IA da IDE TsCDesk** dois comandos locais. **Não são comandos do OpenCode, não fecham a IDE e não são enviados ao modelo.**

### `FINAL`
- Reconhecer somente quando o conteúdo integral da mensagem, após remover espaços externos, for `FINAL` (sem diferenciar maiúsculas/minúsculas). Não disparar ao encontrar a palavra dentro de código, textos maiores ou mensagens citadas.
- Interromper imediatamente o loop de agente e impedir o agendamento de novas inferências ou ferramentas de IA.
- Cancelar operações locais em andamento e, para jobs RunPod conhecidos e ainda ativos, solicitar cancelamento remoto pela API quando suportado. Se o cancelamento remoto falhar, informar o estado real e não prometer que o processamento terminou.
- Impedir que respostas tardias ou callbacks assíncronos reiniciem o agente ou executem ferramentas após a pausa.
- Persistir o estado **IA pausada** de maneira coerente com a arquitetura de sessões da IDE.
- Manter editor, arquivos, terminal e demais recursos normais da IDE funcionando.

### `INICIAR`
- Reconhecer apenas como mensagem isolada, localmente.
- Retomar a permissão para **novas** requisições de IA, sem ressuscitar jobs cancelados ou executar automaticamente a tarefa anterior.
- Exibir estado **IA disponível**. A GPU só deve iniciar mediante uma nova solicitação efetiva do usuário.

### Interface e estados
- Mostrar indicador visível: **IA disponível**, **IA processando**, **IA pausada**, **Cancelando** ou **Erro ao cancelar**, conforme o estado real.
- Permitir também pausar/retomar por controle visual, reutilizando a mesma lógica dos comandos.
- Quando pausada, bloquear envios comuns à IA e explicar que `INICIAR` reativa a sessão.
- Evitar condições de corrida, múltiplos cancelamentos e execução duplicada.

### Economia de GPU / Scale to Zero
- O endpoint RunPod Serverless foi configurado com `Active workers=0` e `Max workers=1`; preservar esses valores como orientação operacional, não como configurações que a IDE deva alterar silenciosamente.
- `FINAL` **não é um comando para desligar fisicamente a GPU**. Ele cessa o uso do endpoint pelo TsCDesk e tenta cancelar jobs. Quando não houver trabalho, o RunPod poderá reduzir workers a zero de acordo com suas regras de escalonamento.
- Não alegar desligamento instantâneo, standby garantido nem custo zero. Não criar jobs, verificações de inferência ou processos de aquecimento enquanto a IA estiver pausada.
- Ao retomar, tratar a inicialização fria e o tempo de fila sem bloquear a interface.

### Testes específicos
- `FINAL` isolado pausa; `final` também; `resultado FINAL do teste` não pausa.
- `INICIAR` isolado retoma; palavras em código não disparam comandos.
- Nenhuma nova requisição é enviada após `FINAL`, mesmo com respostas tardias, retries ou eventos concorrentes.
- Jobs remotos são cancelados quando possível; falhas de cancelamento são exibidas sem ocultação.
- A IDE continua utilizável durante a pausa.
- `INICIAR` não inicia GPU sozinho; nova pergunta inicia o fluxo normal.
- Testes automatizados devem usar mocks e não consumir créditos do RunPod.

**Critério adicional de aceitação:** os dois comandos devem funcionar no TsCDesk, sem dependência de OpenCode em runtime, e sem alterar os demais provedores ou funcionalidades da IDE.

**Restrições finais:** não instale OpenCode dentro da IDE; não reescreva o TsCDesk; não altere provedores existentes sem necessidade; não publique, não faça push, release ou deploy sem autorização. Não invente que a integração foi validada se o endpoint não respondeu a uma inferência real.
