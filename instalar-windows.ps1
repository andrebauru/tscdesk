# Instalador local do TSCDesk para Windows 11, sem exigir privilegios de administrador.
[CmdletBinding()]
param(
    [string]$Destino = (Join-Path $env:LOCALAPPDATA 'Programs\TSCDesk'),
    [switch]$Yes,
    [switch]$PularDependencias,
    [switch]$PularCompilacao,
    [switch]$Reinstalar,
    [switch]$SemAtalhoDesktop,
    [switch]$SemAtalhoMenuIniciar,
    [switch]$AbrirAoFinal,
    [switch]$NaoAbrirAoFinal,
    [switch]$Diagnostico
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$raizProjeto = $PSScriptRoot
$manifesto = Join-Path $raizProjeto 'Cargo.toml'
$diretorioRelease = Join-Path $raizProjeto 'target\release'
$marcadorGerenciado = '.tscdesk-instalador-windows'
$executaveis = @(
    'TsCIDE.exe',
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

# Solicita consentimento antes de instalar dependencias externas.
function Confirmar-InstalacaoDependencias {
    if ($Yes) {
        return
    }
    if ([Console]::IsInputRedirected) {
        Parar-Instalacao 'consentimento interativo indisponivel. Revise o script e use -Yes para aceitar as dependencias.'
    }

    Write-Host 'O TSCDesk precisa instalar componentes oficiais:' -ForegroundColor Yellow
    Write-Host '  - Microsoft Visual Studio Build Tools com C++ e Windows SDK'
    Write-Host '  - Rust stable pelo rustup'
    Write-Host '  - WebView2 Evergreen, somente se estiver ausente'
    Write-Host 'O Visual Studio Build Tools usa varios GB e pode solicitar permissao UAC.'
    $resposta = Read-Host 'Continuar? [s/N]'
    if ($resposta -notin @('s', 'S', 'sim', 'SIM', 'Sim')) {
        Parar-Instalacao 'instalacao cancelada pelo usuario.'
    }
}

# Retorna o executavel encontrado ou $null sem alterar o computador.
function Obter-Comando {
    param([Parameter(Mandatory)][string]$Nome)

    $comando = Get-Command $Nome -ErrorAction SilentlyContinue
    if ($comando) {
        return $comando.Source
    }
    return $null
}

# Localiza o Cargo no PATH ou na pasta padrao criada pelo rustup.
function Resolver-Cargo {
    $cargo = Obter-Comando 'cargo'
    if ($cargo) {
        return $cargo
    }

    $cargoRustup = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $cargoRustup -PathType Leaf) {
        return $cargoRustup
    }

    Parar-Instalacao 'cargo nao foi encontrado. Instale o Rust MSVC em https://rustup.rs, conclua a instalacao e execute este script novamente.'
}

# Verifica se o workload C++ do Visual Studio esta instalado.
function Testar-Msvc {
    if (Obter-Comando 'cl.exe') {
        return $true
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        return $false
    }

    $instalacao = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    return $LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace(($instalacao -join ''))
}

# Verifica os locais padrao do WebView2 Evergreen.
function Testar-WebView2 {
    $bases = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Microsoft\EdgeWebView\Application'),
        (Join-Path $env:LOCALAPPDATA 'Microsoft\EdgeWebView\Application')
    )
    foreach ($base in $bases) {
        if (Get-ChildItem -Path (Join-Path $base '*\msedgewebview2.exe') -File -ErrorAction SilentlyContinue | Select-Object -First 1) {
            return $true
        }
    }
    return $false
}

# Executa o winget e interrompe se o pacote oficial nao for instalado.
function Executar-Winget {
    param([Parameter(Mandatory)][string[]]$Argumentos)

    $winget = Obter-Comando 'winget.exe'
    if (-not $winget) {
        Parar-Instalacao 'winget nao foi encontrado. Atualize ou instale o App Installer pela Microsoft Store e tente novamente.'
    }
    & $winget @Argumentos
    if ($LASTEXITCODE -ne 0) {
        Parar-Instalacao "winget falhou com o codigo $LASTEXITCODE. Verifique a mensagem acima e tente novamente."
    }
}

# Instala somente os pre-requisitos ausentes usando distribuicoes oficiais.
function Instalar-Dependencias {
    param(
        [Parameter(Mandatory)][bool]$PrecisaCargo,
        [Parameter(Mandatory)][bool]$PrecisaMsvc,
        [Parameter(Mandatory)][bool]$PrecisaWebView2
    )

    Confirmar-InstalacaoDependencias

    if ($PrecisaMsvc) {
        Write-Host '[dependencias] Instalando Microsoft C++ Build Tools e Windows SDK...'
        Executar-Winget -Argumentos @(
            'install', '--id', 'Microsoft.VisualStudio.BuildTools', '--exact', '--source', 'winget',
            '--accept-package-agreements', '--accept-source-agreements',
            '--override', '--passive --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
        )
    }

    if ($PrecisaCargo) {
        Write-Host '[dependencias] Instalando Rustup e Rust stable...'
        Executar-Winget -Argumentos @(
            'install', '--id', 'Rustlang.Rustup', '--exact', '--source', 'winget', '--silent',
            '--accept-package-agreements', '--accept-source-agreements'
        )
        $rustup = Obter-Comando 'rustup.exe'
        if (-not $rustup) {
            $rustup = Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe'
        }
        if (-not (Test-Path -LiteralPath $rustup -PathType Leaf)) {
            Parar-Instalacao 'rustup foi instalado, mas o executavel nao foi localizado em %USERPROFILE%\.cargo\bin.'
        }
        & $rustup toolchain install stable --profile minimal
        if ($LASTEXITCODE -ne 0) {
            Parar-Instalacao "rustup falhou com o codigo $LASTEXITCODE."
        }
        & $rustup default stable
        if ($LASTEXITCODE -ne 0) {
            Parar-Instalacao "nao foi possivel selecionar o Rust stable; codigo $LASTEXITCODE."
        }
    }

    if ($PrecisaWebView2) {
        Write-Host '[dependencias] Instalando Microsoft Edge WebView2 Evergreen...'
        $bootstrapper = Join-Path ([IO.Path]::GetTempPath()) ('MicrosoftEdgeWebview2Setup-' + [guid]::NewGuid().ToString('N') + '.exe')
        try {
            Invoke-WebRequest -UseBasicParsing -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrapper
            $assinatura = Get-AuthenticodeSignature -LiteralPath $bootstrapper
            if ($assinatura.Status -ne 'Valid' -or $assinatura.SignerCertificate.Subject -notmatch 'Microsoft') {
                Parar-Instalacao 'o instalador WebView2 baixado nao possui assinatura Microsoft valida.'
            }
            $processo = Start-Process -FilePath $bootstrapper -ArgumentList '/silent', '/install' -Wait -PassThru
            if ($processo.ExitCode -notin @(0, 3010)) {
                Parar-Instalacao "a instalacao do WebView2 falhou com o codigo $($processo.ExitCode)."
            }
        }
        finally {
            if (Test-Path -LiteralPath $bootstrapper) {
                Remove-Item -LiteralPath $bootstrapper -Force
            }
        }
    }
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
    $atalho.Description = 'TsCIDE'
    $iconeTsCIDE = Join-Path $DiretorioTrabalho 'resources\tscide.ico'
    if (Test-Path -LiteralPath $iconeTsCIDE -PathType Leaf) {
        $atalho.IconLocation = $iconeTsCIDE
    }
    $atalho.Save()
}

# Decide se a primeira instalacao deve abrir o aplicativo.
function Confirmar-PrimeiraAbertura {
    param([Parameter(Mandatory)][bool]$PrimeiraInstalacao)

    if ($AbrirAoFinal) {
        return $true
    }
    if ($NaoAbrirAoFinal -or -not $PrimeiraInstalacao -or [Console]::IsInputRedirected) {
        return $false
    }

    $resposta = Read-Host 'Deseja abrir o TsCIDE agora? [s/N]'
    return $resposta -in @('s', 'S', 'sim', 'SIM', 'Sim')
}

# Marca a proxima abertura para solicitar uma nova senha mestra apos reinstalacao.
function Solicitar-NovaSenhaMestra {
    $diretorioDados = Join-Path $env:USERPROFILE '.quixi'
    if (Test-Path -LiteralPath $diretorioDados) {
        $itemDados = Get-Item -LiteralPath $diretorioDados -Force
        if (-not $itemDados.PSIsContainer -or ($itemDados.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            Parar-Instalacao "o diretorio de dados '$diretorioDados' nao e uma pasta local segura."
        }
    }
    else {
        New-Item -ItemType Directory -Path $diretorioDados | Out-Null
    }

    $marcadorSenha = Join-Path $diretorioDados 'master-password-setup-required'
    if ((Test-Path -LiteralPath $marcadorSenha) -and
        ((Get-Item -LiteralPath $marcadorSenha -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        Parar-Instalacao "o marcador de senha mestra '$marcadorSenha' e um link e nao sera sobrescrito."
    }
    Set-Content -LiteralPath $marcadorSenha -Value 'Configure uma nova senha mestra apos reinstalar o TsCIDE.' -Encoding UTF8
    Write-Host 'O TsCIDE solicitara uma nova senha mestra na proxima abertura.'
}

# Impede que binarios substituiveis sejam instalados sobre dados pessoais persistentes.
function Validar-DestinoForaDosDados {
    param(
        [Parameter(Mandatory)][string]$DestinoInstalacao,
        [Parameter(Mandatory)][string]$DiretorioDados
    )

    $destinoNormalizado = ([IO.Path]::GetFullPath($DestinoInstalacao)).TrimEnd('\') + '\'
    $dadosNormalizados = ([IO.Path]::GetFullPath($DiretorioDados)).TrimEnd('\') + '\'
    if ($destinoNormalizado.StartsWith($dadosNormalizados, [StringComparison]::OrdinalIgnoreCase) -or
        $dadosNormalizados.StartsWith($destinoNormalizado, [StringComparison]::OrdinalIgnoreCase)) {
        Parar-Instalacao "o destino '$DestinoInstalacao' se sobrepoe ao diretorio de dados '$DiretorioDados'. Historico, configuracoes e credenciais nao serao colocados em risco."
    }
}

if (-not (Test-Path -LiteralPath $manifesto -PathType Leaf)) {
    Parar-Instalacao 'execute este script a partir da copia completa do projeto.'
}

if ([Environment]::OSVersion.Version.Build -lt 22000) {
    Parar-Instalacao 'este instalador requer Windows 11.'
}
if ($AbrirAoFinal -and $NaoAbrirAoFinal) {
    Parar-Instalacao 'use apenas uma opcao: -AbrirAoFinal ou -NaoAbrirAoFinal.'
}

$precisaCargo = -not (Obter-Comando 'cargo') -and -not (Test-Path -LiteralPath (Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe') -PathType Leaf)
$precisaMsvc = -not (Testar-Msvc)
$precisaWebView2 = -not (Testar-WebView2)

if ($Diagnostico) {
    Write-Host "Cargo/Rust: $(if ($precisaCargo) { 'ausente' } else { 'ok' })"
    Write-Host "MSVC/Windows SDK: $(if ($precisaMsvc) { 'ausente' } else { 'ok' })"
    Write-Host "WebView2: $(if ($precisaWebView2) { 'ausente' } else { 'ok' })"
    if ($precisaCargo -or $precisaMsvc -or $precisaWebView2) {
        exit 1
    }
    exit 0
}

$precisaCargoParaBuild = $precisaCargo -and -not $PularCompilacao
$precisaMsvcParaBuild = $precisaMsvc -and -not $PularCompilacao
if ($PularDependencias) {
    if ($precisaCargo -and -not $PularCompilacao) {
        Parar-Instalacao 'Cargo/Rust esta ausente e -PularDependencias foi informado.'
    }
    if ($precisaMsvc -and -not $PularCompilacao) {
        Parar-Instalacao 'MSVC/Windows SDK esta ausente e -PularDependencias foi informado.'
    }
    if ($precisaWebView2) {
        Parar-Instalacao 'WebView2 esta ausente e -PularDependencias foi informado.'
    }
}
elseif ($precisaCargoParaBuild -or $precisaMsvcParaBuild -or $precisaWebView2) {
    Instalar-Dependencias -PrecisaCargo $precisaCargoParaBuild -PrecisaMsvc $precisaMsvcParaBuild -PrecisaWebView2 $precisaWebView2
    if ((-not $PularCompilacao) -and (-not (Testar-Msvc))) {
        Parar-Instalacao 'MSVC/Windows SDK ainda nao foi detectado. Reinicie o Windows e execute o instalador novamente.'
    }
    if (-not (Testar-WebView2)) {
        Parar-Instalacao 'WebView2 ainda nao foi detectado. Reinicie o Windows e execute o instalador novamente.'
    }
}

if (-not $PularCompilacao) {
    $cargo = Resolver-Cargo
    Write-Host 'Observacao: linhas iniciadas por warning sao avisos do compilador, nao falhas.'
    Write-Host '[1/4] Compilando o TSCDesk em modo release...'
    & $cargo build --locked --release --workspace --manifest-path $manifesto
    if ($LASTEXITCODE -ne 0) {
        Parar-Instalacao "a compilacao falhou com o codigo $LASTEXITCODE."
    }
}
else {
    Write-Host '[1/4] Usando os executaveis ja compilados...'
}

$executavelPrincipal = Join-Path $diretorioRelease 'TsCIDE.exe'
if (-not (Test-Path -LiteralPath $executavelPrincipal -PathType Leaf)) {
    Parar-Instalacao "TsCIDE.exe nao existe em '$diretorioRelease'. Execute novamente sem -PularCompilacao."
}

$destinoCompleto = [IO.Path]::GetFullPath($Destino)
$diretorioDadosPersistentes = [IO.Path]::GetFullPath((Join-Path $env:USERPROFILE '.quixi'))
Validar-DestinoForaDosDados -DestinoInstalacao $destinoCompleto -DiretorioDados $diretorioDadosPersistentes
$marcadorDestino = Join-Path $destinoCompleto $marcadorGerenciado
$primeiraInstalacao = -not (Test-Path -LiteralPath $destinoCompleto)
if ($Reinstalar -and $primeiraInstalacao) {
    Parar-Instalacao "nao ha uma instalacao em '$destinoCompleto' para reinstalar. Execute sem -Reinstalar para a primeira instalacao."
}
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
    if ($Reinstalar) {
        Write-Host "Reinstalacao segura: preservando historico, configuracoes e credenciais em $diretorioDadosPersistentes" -ForegroundColor Cyan
    }
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
    $executavelInstalado = Join-Path $destinoCompleto 'TsCIDE.exe'
    if (-not $SemAtalhoMenuIniciar) {
        $menuIniciar = Join-Path ([Environment]::GetFolderPath('Programs')) 'TsCIDE\TsCIDE.lnk'
        try {
            Criar-Atalho -Caminho $menuIniciar -Executavel $executavelInstalado -DiretorioTrabalho $destinoCompleto
        }
        catch {
            Write-Warning "O aplicativo foi instalado, mas o atalho do menu Iniciar falhou: $($_.Exception.Message)"
        }
    }
    if (-not $SemAtalhoDesktop) {
        $desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'TsCIDE.lnk'
        try {
            Criar-Atalho -Caminho $desktop -Executavel $executavelInstalado -DiretorioTrabalho $destinoCompleto
        }
        catch {
            Write-Warning "O aplicativo foi instalado, mas o atalho do Desktop falhou: $($_.Exception.Message)"
        }
    }

    Write-Host "TsCIDE instalado com sucesso em: $destinoCompleto" -ForegroundColor Green
    if ((-not $primeiraInstalacao) -and (-not $Reinstalar)) {
        Solicitar-NovaSenhaMestra
    }
    if (Confirmar-PrimeiraAbertura -PrimeiraInstalacao $primeiraInstalacao) {
        Start-Process -FilePath $executavelInstalado -WorkingDirectory $destinoCompleto
    }
    else {
        Write-Host 'O TsCIDE nao foi aberto. Use o atalho quando desejar.'
    }
}
finally {
    if (Test-Path -LiteralPath $pastaTemporaria) {
        Remove-Item -LiteralPath $pastaTemporaria -Recurse -Force
    }
}
