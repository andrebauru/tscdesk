# Instalador local do TSCDesk para Windows 11, sem exigir privilegios de administrador.
[CmdletBinding()]
param(
    [string]$Destino = (Join-Path $env:LOCALAPPDATA 'Programs\TSCDesk'),
    [switch]$PularCompilacao,
    [switch]$SemAtalhoDesktop,
    [switch]$SemAtalhoMenuIniciar,
    [switch]$AbrirAoFinal
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$raizProjeto = $PSScriptRoot
$manifesto = Join-Path $raizProjeto 'Cargo.toml'
$diretorioRelease = Join-Path $raizProjeto 'target\release'
$marcadorGerenciado = '.tscdesk-instalador-windows'
$executaveis = @(
    'QuixiDesk.exe',
    'quixi-disclaimer.exe',
    'quixi-chrome-native-host.exe',
    'quixi-ssh-askpass.exe',
    'quixi-guest-daemon.exe',
    'quixi-mcp-builtin.exe',
    'quixi-mcp-worker.exe'
)

# Encerra a instalacao com uma mensagem objetiva e codigo de erro.
function Parar-Instalacao {
    param([Parameter(Mandatory)][string]$Mensagem)

    Write-Host "TSCDesk: $Mensagem" -ForegroundColor Red
    exit 1
}

# Localiza o Cargo no PATH ou na pasta padrao criada pelo rustup.
function Resolver-Cargo {
    $comando = Get-Command 'cargo' -ErrorAction SilentlyContinue
    if ($comando) {
        return $comando.Source
    }

    $cargoRustup = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $cargoRustup -PathType Leaf) {
        return $cargoRustup
    }

    Parar-Instalacao 'cargo nao foi encontrado. Instale o Rust MSVC em https://rustup.rs, conclua a instalacao e execute este script novamente.'
}

# Cria um atalho do Windows para o executavel instalado.
function Criar-Atalho {
    param(
        [Parameter(Mandatory)][string]$Caminho,
        [Parameter(Mandatory)][string]$Executavel,
        [Parameter(Mandatory)][string]$DiretorioTrabalho
    )

    $pasta = Split-Path -Parent $Caminho
    New-Item -ItemType Directory -Path $pasta -Force | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    $atalho = $shell.CreateShortcut($Caminho)
    $atalho.TargetPath = $Executavel
    $atalho.WorkingDirectory = $DiretorioTrabalho
    $atalho.Description = 'TSCDesk'
    $atalho.Save()
}

if (-not (Test-Path -LiteralPath $manifesto -PathType Leaf)) {
    Parar-Instalacao 'execute este script a partir da copia completa do projeto.'
}

if ([Environment]::OSVersion.Version.Build -lt 22000) {
    Parar-Instalacao 'este instalador requer Windows 11.'
}

if (-not $PularCompilacao) {
    $cargo = Resolver-Cargo
    Write-Host '[1/4] Compilando o TSCDesk em modo release...'
    & $cargo build --locked --release --workspace --manifest-path $manifesto
    if ($LASTEXITCODE -ne 0) {
        Parar-Instalacao "a compilacao falhou com o codigo $LASTEXITCODE."
    }
}
else {
    Write-Host '[1/4] Usando os executaveis ja compilados...'
}

$executavelPrincipal = Join-Path $diretorioRelease 'QuixiDesk.exe'
if (-not (Test-Path -LiteralPath $executavelPrincipal -PathType Leaf)) {
    Parar-Instalacao "QuixiDesk.exe nao existe em '$diretorioRelease'. Execute novamente sem -PularCompilacao."
}

$destinoCompleto = [IO.Path]::GetFullPath($Destino)
$marcadorDestino = Join-Path $destinoCompleto $marcadorGerenciado
if ((Test-Path -LiteralPath $destinoCompleto) -and -not (Test-Path -LiteralPath $marcadorDestino -PathType Leaf)) {
    Parar-Instalacao "o destino '$destinoCompleto' ja existe e nao pertence a este instalador. Escolha outro com -Destino."
}

$pastaTemporaria = Join-Path ([IO.Path]::GetTempPath()) ('tscdesk-install-' + [guid]::NewGuid().ToString('N'))
$conteudoTemporario = Join-Path $pastaTemporaria 'TSCDesk'
$backup = "$destinoCompleto.backup"

if (Test-Path -LiteralPath $backup) {
    Parar-Instalacao "o backup '$backup' ja existe. Preserve ou remova essa pasta antes de tentar novamente."
}

try {
    Write-Host '[2/4] Preparando os arquivos...'
    New-Item -ItemType Directory -Path $conteudoTemporario -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $raizProjeto 'resources') -Destination (Join-Path $conteudoTemporario 'resources') -Recurse

    foreach ($nome in $executaveis) {
        $origem = Join-Path $diretorioRelease $nome
        if (Test-Path -LiteralPath $origem -PathType Leaf) {
            Copy-Item -LiteralPath $origem -Destination (Join-Path $conteudoTemporario $nome)
        }
        elseif ($nome -eq 'quixi-mcp-worker.exe') {
            Parar-Instalacao "o auxiliar obrigatorio '$nome' nao foi gerado."
        }
        else {
            Write-Warning "Auxiliar opcional nao encontrado: $nome"
        }
    }

    Set-Content -LiteralPath (Join-Path $conteudoTemporario $marcadorGerenciado) -Value 'Gerenciado por instalar-windows.ps1' -Encoding UTF8

    Write-Host '[3/4] Instalando no perfil do usuario...'
    if (Test-Path -LiteralPath $destinoCompleto) {
        Move-Item -LiteralPath $destinoCompleto -Destination $backup
    }

    try {
        New-Item -ItemType Directory -Path (Split-Path -Parent $destinoCompleto) -Force | Out-Null
        Move-Item -LiteralPath $conteudoTemporario -Destination $destinoCompleto
    }
    catch {
        if (Test-Path -LiteralPath $backup) {
            Move-Item -LiteralPath $backup -Destination $destinoCompleto
        }
        throw
    }

    if (Test-Path -LiteralPath $backup) {
        Remove-Item -LiteralPath $backup -Recurse -Force
    }

    Write-Host '[4/4] Criando atalhos...'
    $executavelInstalado = Join-Path $destinoCompleto 'QuixiDesk.exe'
    if (-not $SemAtalhoMenuIniciar) {
        $menuIniciar = Join-Path ([Environment]::GetFolderPath('Programs')) 'TSCDesk\TSCDesk.lnk'
        Criar-Atalho -Caminho $menuIniciar -Executavel $executavelInstalado -DiretorioTrabalho $destinoCompleto
    }
    if (-not $SemAtalhoDesktop) {
        $desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'TSCDesk.lnk'
        Criar-Atalho -Caminho $desktop -Executavel $executavelInstalado -DiretorioTrabalho $destinoCompleto
    }

    Write-Host "TSCDesk instalado com sucesso em: $destinoCompleto" -ForegroundColor Green
    if ($AbrirAoFinal) {
        Start-Process -FilePath $executavelInstalado -WorkingDirectory $destinoCompleto
    }
}
finally {
    if (Test-Path -LiteralPath $pastaTemporaria) {
        Remove-Item -LiteralPath $pastaTemporaria -Recurse -Force
    }
}
