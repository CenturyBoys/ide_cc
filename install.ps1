# Instalador do code-intel-mcp para Windows (PowerShell). Baixa o binário do último release,
# verifica os language servers e (opcional) registra global no Claude Code.
#
# Uso (PowerShell):
#   irm https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.ps1 | iex
#
# "Tudo em um" (instala LSPs faltantes + registra global no Claude Code):
#   $env:INSTALL_LSP=1; $env:REGISTER=1; irm https://raw.githubusercontent.com/CenturyBoys/ide_cc/main/install.ps1 | iex
#
# Opções (env): BIN_DIR (padrão %LOCALAPPDATA%\Programs\code-intel)  VERSION=latest
#               INSTALL_LSP=1  REGISTER=1  WRITE_MCP=1 (escreve .mcp.json no diretório atual)

$ErrorActionPreference = "Stop"

$Repo    = "CenturyBoys/ide_cc"
$BinDir  = if ($env:BIN_DIR) { $env:BIN_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\code-intel" }
$Version = if ($env:VERSION) { $env:VERSION } else { "latest" }
$Target  = "x86_64-pc-windows-msvc"   # único alvo Windows publicado hoje

# 1. baixa e extrai o binário (.zip com code-intel-mcp.exe)
if ($Version -eq "latest") {
  $url = "https://github.com/$Repo/releases/latest/download/code-intel-mcp-$Target.zip"
} else {
  $url = "https://github.com/$Repo/releases/download/$Version/code-intel-mcp-$Target.zip"
}
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
$zip = Join-Path $env:TEMP "code-intel-mcp.zip"
Write-Host ">> baixando ($Target): $url"
Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
Expand-Archive -Path $zip -DestinationPath $BinDir -Force
Remove-Item $zip -Force
$exe = Join-Path $BinDir "code-intel-mcp.exe"
Write-Host ">> instalado: $exe"

# 2. verifica os language servers (instale só os das linguagens que usar)
Write-Host "`n>> language servers:"
function Check-Lsp($cmd, $hint) {
  if (Get-Command $cmd -ErrorAction SilentlyContinue) { Write-Host "  [ok]    $cmd" }
  else { Write-Host "  [falta] $cmd  ->  $hint" }
}
Check-Lsp "tsgo"                    "npm i -g @typescript/native-preview"
Check-Lsp "vtsls"                   "npm i -g @vtsls/language-server"
Check-Lsp "basedpyright-langserver" "pip install basedpyright   (ou: npm i -g basedpyright)"
Check-Lsp "dart"                    "instale o Dart/Flutter SDK"
Check-Lsp "rust-analyzer"           "rustup component add rust-analyzer"
Check-Lsp "csharp-ls"              "dotnet tool install --global csharp-ls"

# 2b. opcional: instala os language servers faltantes (best-effort)
if ($env:INSTALL_LSP -eq "1") {
  Write-Host "`n>> instalando language servers (INSTALL_LSP=1)..."
  if (Get-Command npm    -ErrorAction SilentlyContinue) { npm i -g @typescript/native-preview @vtsls/language-server 2>$null; Write-Host "  [ok] tsgo + vtsls (npm)" }
  if (Get-Command pip    -ErrorAction SilentlyContinue) { pip install -q basedpyright 2>$null; Write-Host "  [ok] basedpyright (pip)" }
  if (Get-Command rustup -ErrorAction SilentlyContinue) { rustup component add rust-analyzer 2>$null; Write-Host "  [ok] rust-analyzer (rustup)" }
  if (Get-Command dotnet -ErrorAction SilentlyContinue) { dotnet tool install --global csharp-ls 2>$null; Write-Host "  [ok] csharp-ls (dotnet)" }
  Write-Host "  (Dart: instale o SDK manualmente se precisar)"
}

# 2c. opcional: registra o MCP GLOBAL no Claude Code (vale em todos os projetos)
if ($env:REGISTER -eq "1" -and (Get-Command claude -ErrorAction SilentlyContinue)) {
  claude mcp remove code-intel --scope user 2>$null | Out-Null
  claude mcp add code-intel --scope user -- "$exe" 2>$null | Out-Null
  Write-Host "`n>> registrado GLOBAL no Claude Code (escopo user) — funciona em qualquer projeto"
}

# 3. opcional: escreve um .mcp.json no diretório atual
if ($env:WRITE_MCP -eq "1") {
  $exeJson = $exe -replace '\\','\\'
  @"
{
  "mcpServers": {
    "code-intel": {
      "command": "$exeJson",
      "env": {
        "TSGO_BIN": "tsgo", "VTSLS_BIN": "vtsls",
        "BASEDPYRIGHT_BIN": "basedpyright-langserver",
        "DART_BIN": "dart", "RUST_ANALYZER_BIN": "rust-analyzer",
        "CSHARP_LS_BIN": "csharp-ls"
      }
    }
  }
}
"@ | Set-Content -Path ".mcp.json" -Encoding UTF8
  Write-Host "`n>> .mcp.json escrito em $(Get-Location)\.mcp.json"
}

# 3b. IMPORTANTE: o install NÃO configura o workspace por projeto — isso é feito pelo `doctor`,
#     rodado DENTRO de cada projeto. Crítico em Python: sem [tool.basedpyright]/pyrightconfig.json
#     o basedpyright entra em modo "openFilesOnly" e o find_references sai INCOMPLETO EM SILÊNCIO.
Write-Host "`n>> ATENCAO - setup por projeto (o install nao faz isso):"
Write-Host "   rode a ferramenta 'doctor' (fix=true) DENTRO de cada projeto para configurar o workspace."
Write-Host "   Python e critico: sem a config, find_references retorna INCOMPLETO em silencio."
Write-Host "   No Claude Code, peca: 'rode o doctor com fix' - ou doctor(project=<repo>, fix=true)."

# 4. PATH + próximos passos
if (-not (($env:Path -split ';') -contains $BinDir)) {
  Write-Host "`n>> adicione ao PATH (permanente):"
  Write-Host "   [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$BinDir`", 'User')"
}
Write-Host "`n>> pronto. NB: o cache entre sessões (CODE_INTEL_DAEMON) não está disponível no Windows;"
Write-Host "   todas as ferramentas semânticas funcionam normalmente."
Write-Host "   docs: https://github.com/$Repo#install"
