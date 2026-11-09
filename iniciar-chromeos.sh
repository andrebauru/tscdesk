#!/usr/bin/env bash
# Instala na primeira execução e abre TSCDesk no Linux do ChromeOS (Crostini).
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
usage() {
    cat <<'EOF'
Uso: bash iniciar-chromeos.sh [opções] [-- argumentos do aplicativo]
  --help, -h       Mostrar ajuda; não instala nem abre nada.
  --instalar      Instalar/atualizar explicitamente antes de abrir.
  --reinstalar    Substituir versões anteriores gerenciadas sem apagar dados.
  --jobs N        Jobs de compilação (padrão 1, adequado a Chromebooks).
  --yes           Consentir com APT e instalação/atualização do Rust.
  --skip-deps     Reutilizar dependências Linux já instaladas.
  --skip-build    Instalar os binários existentes em target/release.
  --abrir         Abrir o aplicativo após instalar, sem perguntar.
  --nao-abrir     Instalar e encerrar sem abrir o aplicativo.
  --check         Diagnóstico da instalação existente, sem instalar/abrir.

Execute no Terminal Linux do Chromebook (contêiner Crostini `penguin`), como seu usuário normal.
Na primeira execução utiliza scripts/install-linux.sh e pede consentimento.
Em um Crostini novo, instala Python, compiladores, GTK/WebKit e Rust stable.
Nas seguintes abre a instalação existente sem recompilar.
Não configura início no login nem faz inferências ou aquecimento de GPU.
EOF
}
fail() { printf 'TSCDesk: %s\n' "$*" >&2; exit 1; }
for arg in "$@"; do
    case "$arg" in
        --) break ;;
        --help|-h) usage; exit 0 ;;
    esac
done
[[ "$(uname -s)" == Linux ]] || fail 'Execute no Terminal Linux do ChromeOS (contêiner Crostini penguin).'
(( EUID != 0 )) || fail 'Não execute como root/sudo; apenas o instalador usa sudo para APT.'
INSTALL=0 CHECK=0 JOBS=1 OPEN_MODE=ask
OPTIONS=()
APP_ARGS=()
while (( $# )); do
    case "$1" in
        --instalar) INSTALL=1; shift ;;
        --reinstalar) INSTALL=1; OPTIONS+=("--reinstall"); shift ;;
        --check) CHECK=1; shift ;;
        --abrir) OPEN_MODE=yes; shift ;;
        --nao-abrir) OPEN_MODE=no; shift ;;
        --jobs) (( $# >= 2 )) || fail 'Informe o valor de --jobs.'; JOBS="$2"; shift 2 ;;
        --yes|--skip-deps|--skip-build) OPTIONS+=("$1"); shift ;;
        --) shift; APP_ARGS=("$@"); break ;;
        *) fail "Opção desconhecida: $1 (use --help)." ;;
    esac
done
[[ "$JOBS" =~ ^[1-9][0-9]*$ ]] || fail '--jobs deve ser um inteiro positivo.'
(( ! CHECK || ! INSTALL )) || fail '--check não pode ser usado com --instalar.'
if (( CHECK )) && [[ "$OPEN_MODE" != ask ]]; then
    fail '--check não pode ser combinado com --abrir ou --nao-abrir.'
fi
[[ -n "${DISPLAY:-}" || -n "${WAYLAND_DISPLAY:-}" ]] || fail 'Sessão gráfica ausente. Abra o Terminal Linux do contêiner penguin no Chromebook.'
PREFIX="$HOME/.local/opt/tscdesk"
LAUNCHER="$HOME/.local/bin/tscdesk"
FIRST_INSTALL=0
[[ -x "$LAUNCHER" ]] || FIRST_INSTALL=1
if (( CHECK )); then
    [[ -x "$LAUNCHER" ]] || fail 'Instalação ausente. Execute bash iniciar-chromeos.sh primeiro.'
elif (( INSTALL )) || [[ ! -x "$LAUNCHER" ]]; then
    bash "$ROOT/scripts/install-linux.sh" --prefix "$PREFIX" --jobs "$JOBS" "${OPTIONS[@]}"
fi
# Não executar um comando homônimo de outro programa ou um symlink estrangeiro.
command -v python3 >/dev/null 2>&1 || fail 'python3 ausente; instale-o no Terminal Linux.'
python3 - "$PREFIX" "$LAUNCHER" <<'PY'
import hashlib, os, pathlib, re, sys
prefix, launcher = (pathlib.Path(os.path.abspath(value)) for value in sys.argv[1:])
mark = 'TSCDesk Linux installer managed v1\n'
def managed_marker(path):
    return not path.is_symlink() and path.is_file() and path.read_text() == mark
try:
    for path in (prefix, launcher):
        if any(part.is_symlink() for part in (path, *path.parents)):
            raise ValueError('Instalação/launcher não pode conter um symlink estrangeiro.')
        if path.stat().st_uid != os.getuid():
            raise ValueError('Instalação/launcher pertence a outro usuário.')
    if not managed_marker(prefix / '.tscdesk-managed'):
        raise ValueError('Instalação não gerenciada pelo TSCDesk.')
    identity = hashlib.sha256(str(prefix).encode()).hexdigest()
    if '# TSCDesk-managed-v1 ' + identity not in launcher.read_text().splitlines():
        raise ValueError('Comando tscdesk existente pertence a outra instalação; ele foi preservado.')
    current = prefix / 'current'
    target = os.readlink(current) if current.is_symlink() else ''
    if not re.fullmatch(r'releases/[A-Za-z0-9_.-]+', target) or target.split('/')[-1] in ('.', '..'):
        raise ValueError('Release instalada inválida.')
    release = prefix / target
    if release.is_symlink() or release.parent.is_symlink():
        raise ValueError('Release instalada contém symlink estrangeiro.')
    if not managed_marker(release / '.tscdesk-release'):
        raise ValueError('Release não gerenciada.')
    for path in (release / 'bin', release / 'bin/launch-linux.sh'):
        if path.is_symlink() or not path.exists() or path.stat().st_uid != os.getuid():
            raise ValueError('Launcher de release inválido ou estrangeiro.')
except (OSError, ValueError) as exc:
    sys.exit('TSCDesk: ' + str(exc))
PY
if (( CHECK )); then exec "$LAUNCHER" --check; fi
if [[ "$OPEN_MODE" == no ]]; then
    printf 'TSCDesk instalado. Abra pelo lançador ou execute %s quando desejar.\n' "$LAUNCHER"
    exit 0
fi
if (( FIRST_INSTALL )) && [[ "$OPEN_MODE" == ask ]]; then
    if [[ ! -t 0 ]]; then
        printf 'TSCDesk instalado e não aberto. Use --abrir para abrir automaticamente.\n'
        exit 0
    fi
    read -r -p 'Deseja abrir o TSCDesk agora? [s/N] ' answer
    [[ "$answer" == s || "$answer" == S || "$answer" == sim ]] || {
        printf 'TSCDesk instalado. Abra pelo lançador quando desejar.\n'
        exit 0
    }
fi
exec "$LAUNCHER" "${APP_ARGS[@]}"
