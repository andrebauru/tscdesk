# Validação do TSCDesk no ChromeOS (Crostini)

O alvo é o aplicativo Linux dentro do container Debian do Chromebook. Testes em Docker, WSL ou Xvfb não comprovam a integração com Sommelier, o cofre de senhas ou o desktop hospedeiro.

## Validação realizada nesta adaptação

- `cargo check --locked --workspace --all-targets`: passou em um container Debian 12 x86_64, com Rust 1.99.1, GTK 3.24.38 e WebKitGTK 2.50.6.
- Sete testes Rust específicos de informações Linux, captura, cancelamento de seletor e cache de clipboard: passaram.
- Fixtures de instalação/launcher no WSL Ubuntu 26.04: 19 testes passaram; o teste de rejeição de root foi validado separadamente como root.
- 94 testes da interface JavaScript, ShellCheck dos scripts e auditoria dos recursos: passaram.
- Não foram executados um build release completo, a janela em Crostini ou testes em hardware ARM64. O suporte ChromeOS continua experimental até completar o checklist abaixo.

## Arquivo de entrada na raiz

Use `bash iniciar-chromeos.sh` no Terminal Linux do Chromebook. Ele instala na primeira execução e abre a IDE; nas seguintes reutiliza a instalação. `--instalar` força uma atualização e `--check` faz diagnóstico sem abrir a janela ou chamar RunPod. Não configura início no login nem aquecimento da GPU. O script fica [na raiz do projeto](../iniciar-chromeos.sh).

## Antes de começar

- Ative o ambiente Linux e confirme que a política do dispositivo permite utilizá-lo.
- Mantenha o checkout e a instalação no armazenamento Linux, não em `/mnt/chromeos`.
- Registre a versão do ChromeOS manualmente e, no Terminal Linux, execute:

```bash
cat /etc/os-release
uname -m
printf 'DISPLAY=%s\nWAYLAND_DISPLAY=%s\n' "${DISPLAY:-}" "${WAYLAND_DISPLAY:-}"
pkg-config --modversion gtk+-3.0 webkit2gtk-4.1
rustc --version
```

Não inclua chaves de API, conteúdo de projetos privados nem tokens de automação em relatórios.

## Checklist no Chromebook

1. **Instalação:** `bash scripts/install-linux.sh --jobs 1`; nenhuma etapa de build deve rodar como root. Confira o atalho TSCDesk no menu.
2. **Diagnóstico:** `"$HOME/.local/bin/tscdesk" --check`. Confira recursos, sessão gráfica e D-Bus. Um aviso sobre Secret Service requer verificação do cofre, não a remoção de sua senha.
3. **Janela:** abra pelo menu e pelo Terminal; confira redimensionamento, tema e texto. Não configure flags para desativar o sandbox do WebKit.
4. **Clipboard manual:** copie texto de uma resposta e cole no Terminal/Chrome com os atalhos normais. A cópia não deve depender de instalar `xclip` ou `wl-copy`. Isso não implica leitura programática do clipboard hospedeiro.
5. **Seletor:** abra o seletor de pasta e cancele. Nenhuma pasta nem arquivo padrão deve ser adicionado. Escolha depois uma pasta de teste no Linux.
6. **Arquivos compartilhados:** compartilhe uma pasta descartável pelo app Arquivos → Compartilhar com Linux. Informe seu caminho em `/mnt/chromeos` e teste apenas arquivos descartáveis.
7. **Credenciais:** em um perfil normal, use um Secret Service protegido e desbloqueado. Configure seu provedor e confirme que o salvamento funciona. Não use perfis de teste com chaves reais.
8. **Tarefa:** usando modo Manual, peça a explicação de um arquivo de teste. Confira leituras, aprovações de edições/comandos e os resultados; não comece em Auto.
9. **Persistência:** encerre normalmente, reabra e confira o histórico. Reinicie o ambiente Linux e confira novamente. Faça backup de `~/.quixi` antes de testes que envolvam seu perfil normal.
10. **Atualização:** repita o instalador após um novo build. O atalho deve continuar funcionando e os dados do usuário devem permanecer intactos. Releases anteriores não são removidos automaticamente.
11. **Captura:** se o ambiente não oferecer um backend, a ferramenta deve retornar erro explícito — não imagem antiga nem PNG fictício. Uma captura de XWayland não prova acesso à tela inteira do ChromeOS.
12. **Limites:** agendamentos exigem aplicativo e container ativos. Não instale daemons privilegiados de entrada nem presuma acesso aos apps Android ou às abas do Chrome do sistema.

Se o comportamento gráfico variar, teste XWayland apenas como diagnóstico, quando disponível:

```bash
GDK_BACKEND=x11 "$HOME/.local/bin/tscdesk"
```

Não altere globalmente `DISPLAY`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS` nem permissões de dispositivos para contornar erros.

## Perfis descartáveis sem credenciais reais

Para validar a interface isoladamente:

```bash
profile="$(mktemp -d)"
QUIXI_TEST_PROFILE="$profile" "$HOME/.local/bin/tscdesk"
```

O launcher mantém o workspace do perfil, salvo override explícito de `QUIXI_WORKSPACE`. O perfil separa os dados do aplicativo, mas não é um sandbox de arquivos, rede ou ferramentas. O fixture não deve conter chaves de produção.

## Referências

- [Ativar Linux — Google](https://support.google.com/chromebook/answer/9145439?hl=pt-BR)
- [Crostini, Sommelier, compartilhamento e limites — Chromium](https://www.chromium.org/chromium-os/developer-library/guides/containers/containers-and-vms/)
- [README e instalação](../README.md)
