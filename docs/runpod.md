# RunPod Serverless no TSCDesk

## Escopo e estado da validação

Esta integração é para um endpoint **Queue-based**, executando o worker Ollama de terceiros `svenbrnn/runpod-ollama:latest`. Mantém o catálogo e os protocolos dos outros provedores; não incorpora OpenCode, proxy local ou SDK RunPod em runtime.

O adaptador usa a API pública `/v2`, não uma URL fictícia `/v1/{ID}/runpod-ollama`. O servidor Ollama dentro do worker pode falar o protocolo OpenAI, mas isso **não torna a API Queue-based uma API OpenAI/SSE direta**.

O contrato foi consultado no repositório público do worker. A tag `latest` é mutável e não comprova a versão efetivamente implantada no seu endpoint. **Nenhuma inferência paga foi feita nesta validação.** A resposta real do endpoint e a presença do modelo continuam pendentes de teste autorizado.

## Configuração na IDE

1. Abra **Settings → Providers → RunPod Serverless**.
2. Informe somente o **Endpoint ID** mostrado no painel RunPod, sem URL, barra, espaços ou parâmetros.
3. Cole a **API Key** no campo de senha. Não coloque a chave em arquivos do projeto, `.env` versionado, código, screenshots ou mensagens de chat.
4. Informe o nome exato do modelo Ollama, inicialmente **`qwen2.5-coder:7b`**.
5. Clique em **Salvar**. O campo de chave não é preenchido com o segredo salvo; vazio preserva a chave existente.
6. **Testar conexão** faz uma consulta a `/health`, somente por ação explícita. Isso verifica configuração/autenticação, não inicia um job e não valida inferência.
7. Selecione o provedor/modelo no chat. Uma pergunta normal submete um job e **pode consumir créditos**; faça isso somente quando desejar usar o endpoint.

O armazenamento reutiliza a criptografia AES-256-GCM existente e o cofre do sistema. No Crostini, configure/desbloqueie Secret Service conforme o README. Os testes usam perfis descartáveis e credenciais sintéticas, nunca suas credenciais normais.

### Configuração do worker

A IDE envia explicitamente o nome do modelo no payload; não modifica variáveis, imagem, GPU ou escalonamento no painel RunPod.

Há uma divergência documental relevante: a descrição Docker Hub cita `MODEL_NAME`, enquanto o código público do worker utiliza `OLLAMA_MODEL_NAME` no caminho simplificado e no preload. Não alteramos sua variável existente `MODEL_NAME=qwen2.5-coder:7b`. Confira a versão/digest implantado e os logs do worker, sem expor segredos, para verificar se o modelo foi baixado. O nome fornecido no pedido não garante que os pesos existam.

## Fluxo de execução

- Autenticação HTTP: `Authorization: Bearer <API Key>`, somente no backend.
- Destino derivado do ID: `https://api.runpod.ai/v2/{Endpoint ID}`.
- Submissão única: `POST /run`.
- Acompanhamento: `GET /status/{Job ID}`, enquanto o job conhecido estiver ativo, com intervalo crescente e prazo máximo.
- Cancelamento: `POST /cancel/{Job ID}`, quando um job ativo conhecido precisar parar.
- Configuração: `GET /health`, sem job de inferência.

O payload utiliza o caminho explícito do worker:

```json
{
  "input": {
    "openai_route": "/v1/chat/completions",
    "openai_input": {
      "model": "qwen2.5-coder:7b",
      "messages": [
        {"role": "user", "content": "Explique ownership em Rust."}
      ],
      "stream": false
    }
  }
}
```

No worker público, `stream:false` gera um objeto de completions e `return_aggregate_stream:true` agrega o resultado. O adaptador normaliza a resposta para o chat da IDE. Não envia esquemas de ferramentas nem habilita execução de tool calls; o suporte do modelo/worker **não foi comprovado**. Anexos multimodais ou histórico de ferramentas não suportados precisam ser recusados, não executados ou silenciosamente interpretados como autorização.

Fila e cold start podem demorar, especialmente com zero workers ativos e download de pesos. Não há ETA, porcentagem de progresso ou promessa de latência fixa. O adaptador não reenvia automaticamente uma submissão cuja aceitação esteja incerta, evitando jobs/cobranças duplicados.

## FINAL e INICIAR

São comandos locais do **chat TSCDesk**, não comandos enviados ao modelo ou ao terminal. O conteúdo integral, após remover espaços externos, deve ser exatamente `FINAL` ou `INICIAR`, sem diferenciar maiúsculas/minúsculas.

- `FINAL`: persiste a pausa, cancela os loops locais e os tokens de solicitações já iniciadas; tenta cancelar jobs RunPod conhecidos. Não fecha a IDE nem encerra o terminal interativo do usuário.
- `INICIAR`: libera novos pedidos. Não recupera tokens cancelados, repete tarefas antigas, aquece worker ou submete inferência sozinho.
- `FINAL` também limpa filas de mensagens/steering e desarma agendamentos de IA. Para usá-los novamente, reative-os explicitamente em Agendamentos; `INICIAR` não restaura filas nem reativa schedules.
- `resultado FINAL do teste`, `"FINAL"`, blocos de código e mensagens com anexos não são comandos locais.
- Durante a pausa, mensagens comuns à IA são bloqueadas e orientam a usar `INICIAR`.

A interface distingue disponibilidade, processamento, pausa, cancelamento e erro ao cancelar. O indicador é estado da IA na IDE, **não telemetria de potência ou faturamento da GPU**.

### Limites do cancelamento

Uma chamada HTTP já em trânsito pode aguardar seu timeout antes de o cliente recuperar o Job ID e tentar cancelar. Se a submissão for aceita remotamente mas o ID não retornar, a IDE não consegue identificar esse job. Uma falha de API/cancelamento não prova que o worker parou. Jobs de outros clientes também não são cancelados pelo TSCDesk.

Cancelamento é feito apenas para jobs conhecidos desta instância; não há purge da fila inteira, mudança de `Max workers` ou desligamento forçado. Fechar/matar o processo antes de uma confirmação remota pode deixar trabalho ativo: confira o painel RunPod se houver dúvida.

## Economia de GPU

Mantenha como orientação operacional `Active workers=0` e `Max workers=1`, conforme sua configuração. A IDE não altera esses valores.

Não há jobs de aquecimento, inferências periódicas, heartbeat de GPU ou teste de modelo no startup. Polling de status ocorre somente durante um pedido em andamento; leitura local de estado da IA na interface não chama RunPod. O endpoint pode reduzir workers a zero após ficar sem trabalho segundo suas próprias regras, não necessariamente no instante em que você envia `FINAL`.

Uma IDE reaberta não deve ser confundida com uma máquina sem outros agendamentos configurados pelo usuário. A pausa protege o uso de IA; não é um recurso de administração/faturamento da conta RunPod.

## Privacidade

A chave não aparece no estado público, nos logs do adaptador ou no conteúdo da conversa. Respostas de erro externas não são reproduzidas indiscriminadamente, pois podem conter dados sensíveis.

As mensagens e o contexto autorizado da conversa são enviados ao worker/provedor. O código público da imagem imprime o conteúdo do job nos logs **remotos**; isso não pode ser controlado pela ocultação da chave no cliente. Não envie segredos no prompt. Para produção, revise essa imagem e prefira um digest/versionamento auditado.

## Referências consultadas

- [Operações Queue-based — RunPod](https://docs.runpod.io/serverless/endpoints/operation-reference)
- [Worker: engine.py](https://github.com/SvenBrnn/runpod-worker-ollama/blob/master/src/engine.py)
- [Worker: handler.py](https://github.com/SvenBrnn/runpod-worker-ollama/blob/master/src/handler.py)
- [Worker: utils.py](https://github.com/SvenBrnn/runpod-worker-ollama/blob/master/src/utils.py)
- [Worker: start.sh](https://github.com/SvenBrnn/runpod-worker-ollama/blob/master/src/start.sh)
- [Descrição da imagem Docker Hub](https://hub.docker.com/r/svenbrnn/runpod-ollama)
- [Instalação, cofre e ChromeOS](../README.md)
