# Instala o Passo no Windows a partir das releases do GitHub.
#   irm https://raw.githubusercontent.com/JonathanSantos/passo/main/install.ps1 | iex
# Variáveis: PASSO_VERSION (padrão: a última), PASSO_INSTALL_DIR (padrão: %LOCALAPPDATA%\Programs\passo)
$ErrorActionPreference = 'Stop'

$repo = 'JonathanSantos/passo'
$version = if ($env:PASSO_VERSION) { $env:PASSO_VERSION } else { 'latest' }
$dir = if ($env:PASSO_INSTALL_DIR) { $env:PASSO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\passo' }

# Há build para x86_64; o Windows em ARM64 roda esse binário emulado.
$asset = 'passo-x86_64-pc-windows-msvc.zip'
if ($version -eq 'latest') {
  $url = "https://github.com/$repo/releases/latest/download/$asset"
} else {
  $url = "https://github.com/$repo/releases/download/$version/$asset"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("passo-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Write-Host "passo: baixando $url"
  Invoke-WebRequest $url -OutFile (Join-Path $tmp $asset) -UseBasicParsing
  Invoke-WebRequest "$url.sha256" -OutFile (Join-Path $tmp "$asset.sha256") -UseBasicParsing
  $expected = ((Get-Content (Join-Path $tmp "$asset.sha256") -Raw).Trim() -split '\s+')[0].ToLower()
  $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLower()
  if ($expected -ne $actual) { throw "passo: checksum não confere" }
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  Expand-Archive (Join-Path $tmp $asset) -DestinationPath $dir -Force
  Write-Host "passo: instalado em $dir\passo.exe"
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not (($userPath -split ';') -contains $dir)) {
    Write-Host "passo: adicione $dir ao PATH (ex.: [Environment]::SetEnvironmentVariable('Path', `"$dir;`" + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User'))"
  }
  if (-not (Get-Command sh -ErrorAction SilentlyContinue) -and -not (Test-Path "$env:ProgramFiles\Git\bin\sh.exe")) {
    Write-Host "passo: o Passo roda as checagens e os hooks com o sh do Git Bash; instale o Git for Windows (https://git-scm.com)"
  }
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
