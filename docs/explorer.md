# Explorer e contexto explícito

O Explorer permite selecionar pastas e arquivos e definir, por conversa, exatamente
quais arquivos serão enviados aos agentes de IA. Ele foi projetado para não misturar
contextos de projetos diferentes.

## Como usar

1. Abra uma conversa e clique em **Explorer** no cabeçalho (ou abra **Files & context**
   e escolha a aba **Explorer**).
2. **Add folder** abre o seletor nativo de diretórios; **Add files** abre o seletor
   nativo de arquivos. Cada pasta/arquivo vira uma fonte independente.
3. Marque a caixa de uma fonte para selecionar a pasta inteira (recursivamente) ou
   marque arquivos individuais. Desmarcar um arquivo dentro de uma pasta selecionada
   cria uma exceção explícita.
4. Use **Context preview** para ver a quantidade de arquivos, o tamanho estimado e se
   a seleção excede o orçamento do modelo.
5. Envie a mensagem normalmente. Somente os arquivos marcados naquela conversa entram
   no bloco de contexto do agente.

Em uma conversa nova, **Open Explorer** permite preparar a seleção antes do primeiro
envio. Essa seleção de rascunho é transferida uma única vez para a conversa criada e
não é reaproveitada por outras conversas.

As seleções são salvas automaticamente em `explorer.json` no diretório de dados do
aplicativo. Conteúdo de arquivos nunca é persistido: apenas caminhos, metadados e
regras de seleção. Ao reiniciar, o Explorer restaura fontes, seleções e pastas
expandidas. Se uma pasta sumir, ela permanece registrada e marcada como indisponível.

## Isolamento entre projetos e conversas

- O estado é indexado pelo `sessionId` da conversa. Duas conversas nunca compartilham
  fontes ou seleções, mesmo que apontem para a mesma pasta.
- Adicionar uma pasta não a incorpora a outro projeto e não importa arquivos de outras
  fontes. A detecção de projeto (`Cargo.toml`, `composer.json`, `.git`, etc.) é apenas
  um rótulo informativo; ela nunca funde fontes.
- O bloco enviado ao modelo declara explicitamente que apenas os arquivos listados
  foram selecionados naquela conversa e que arquivos de outras conversas/projetos não
  estão disponíveis.
- Se você adicionar duas pastas na mesma conversa e marcar ambas, isso é uma
  autorização explícita; o Explorer não funde fontes por conta própria.
- Subagentes recebem o contexto que o agente principal passar; o Explorer não injeta
  automaticamente a seleção em subagentes.

## Semântica de seleção

- **Fonte do tipo arquivo:** a caixa seleciona ou remove aquele arquivo.
- **Fonte do tipo pasta, selecionada:** todos os arquivos permitidos são herdados,
  exceto os que estiverem na lista de exceções (`deselectedPaths`).
- **Fonte do tipo pasta, não selecionada:** apenas os arquivos marcados
  individualmente (`selectedPaths`) entram.
- **Pasta na árvore:** marcar/desmarcar uma subpasta aplica a regra a todos os arquivos
  permitidos daquela subárvore.
- **Estado visual:** marcado, desmarcado ou parcial (indeterminado) para pastas com
  exceções ou seleções parciais.

## Filtros e exclusões

Por padrão são ocultados e não selecionados automaticamente:

`.git`, `node_modules`, `target`, `dist`, `build`, `vendor`, `.cache`,
`__pycache__`, `.venv`, `.next`, `.turbo`, `.gradle`, `.idea`, `.DS_Store`.

- **Hidden** mostra arquivos/pastas ocultos.
- **Ignored / excluded** mostra itens excluídos e itens cobertos pelo `.gitignore` da
  raiz da fonte. Um arquivo mostrado pode ser selecionado manualmente.
- A caixa da **fonte** (pasta raiz) nunca inclui ocultos, ignorados ou gerados, mesmo
  com essas opções ligadas. Marcar uma subpasta visível ou um arquivo individual é a
  forma explícita de incluí-los.
- `extraExcludes` permite acrescentar padrões por nome ou caminho relativo.

## Leitura segura

- Caminhos relativos são normalizados; `..`, caminhos absolutos, sintaxe de unidade e
  fluxos alternativos do Windows são rejeitados.
- O destino é canonicalizado e precisa permanecer dentro da raiz canônica da fonte.
  Links simbólicos que escapam da raiz são bloqueados. Links internos podem ser
  navegados na árvore, mas a caminhada de seleção não os percorre.
- Arquivos são lidos com limite (`Take`), sem carregar o arquivo inteiro na memória.
- Falhas de permissão do sistema operacional são devolvidas como erro legível e o
  arquivo entra na lista de ignorados; o aplicativo não contorna ACLs.
- UTF-8 (com ou sem BOM) e UTF-16 LE/BE com BOM são decodificados; UTF-8 inválido é
  lido como `utf-8-lossy`. Arquivos com NUL ou muitos bytes de controle são tratados
  como binários e não são enviados como texto.
- O limite por arquivo padrão é 256 KiB e pode ser ajustado. O orçamento total padrão é
  200.000 caracteres, reduzido conforme a janela do modelo (aproximadamente
  `2 × tokens da janela`, com piso de 4.000).
- Quando o orçamento acaba, o bloco é truncado e informa quais arquivos ficaram de
  fora. Binários e arquivos ilegíveis aparecem em uma lista de ignorados, sem conteúdo.

## Arquitetura

| Camada | Arquivo | Responsabilidade |
| --- | --- | --- |
| Domínio | `crates/quixi-engine/src/explorer/mod.rs` | Estado por sessão, seleção, persistência, resumo e montagem do contexto. |
| Árvore e caminhos | `crates/quixi-engine/src/explorer/tree.rs` | Listagem sob demanda, exclusões, `.gitignore`, caminhada limitada e proteção contra traversal/links. |
| Leitura | `crates/quixi-engine/src/explorer/read.rs` | Limite de bytes, detecção de binário, decodificação e truncamento. |
| Projetos | `crates/quixi-engine/src/explorer/project.rs` | Detecção apenas informativa de marcadores de projeto. |
| IPC | `crates/quixi-ipc/src/lib.rs` (serviço `Explorer`) | Expõe estado, fontes, seleção, listagem, preview, refresh e estimativa. |
| Interface | `resources/ui/explorer.js` + `app.js`/`app.css` | Painel lateral, árvore preguiçosa, menus de contexto, contadores e avisos. |
| Prompt | `crates/quixi-engine/src/prompt_context.rs` + `lib.rs` | Injeta o bloco somente na sessão ativa, no caminho com ferramentas e no caminho texto-only (RunPod). |

### IPC

Todas as chamadas usam `rpc("Explorer", membro, [{ sessionId, ... }])`:

- `getState`, `contextPreview`
- `addSource`, `addSources`, `removeSource`, `refresh`
- `setSourceSelected`, `setFileSelected`, `setDirectorySelected`, `selectAll`
- `setExpanded`, `setPreferences`
- `listDirectory`, `previewFile`

## Limitações conhecidas

- Não há observador do sistema de arquivos. A árvore é atualizada ao abrir o painel,
  ao usar **Refresh** e por uma atualização periódica de 15 segundos enquanto o painel
  está visível.
- Apenas o `.gitignore` da raiz de cada fonte é considerado; `.gitignore` aninhados não
  são interpretados.
- Links simbólicos não são percorridos, mesmo quando apontam para dentro da raiz.
- A contagem e a caminhada têm limites de segurança (4.000 arquivos por fonte, 500
  entradas por diretório). O painel avisa quando o limite é atingido.
- A seleção é por conversa, não compartilhada automaticamente entre conversas do mesmo
  projeto.
- O Explorer complementa as ferramentas de workspace existentes; ele não substitui o
  seletor de pasta da conversa nem altera as permissões de ferramentas.

## Testes

- `cargo test -p quixi-engine --lib explorer`: 25 testes cobrindo detecção de projeto,
  isolamento entre sessões, seleção recursiva, exceções, persistência, pastas ausentes,
  binários, nomes especiais, traversal, limites, truncamento, limites de janela, permissões negadas e transferência de rascunho.
- `node --test tests/ui/explorer.test.cjs`: painel, árvore preguiçosa, seleção por
  conversa, preview, menus de contexto, seletor nativo e orçamento.
- `npm test --prefix tests/ui`: suíte completa da interface.
