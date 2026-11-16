# LOGBOOK — eGPU Painel

Log append-only. Cada entrada: o que foi feito, evidência, pendências, próximo passo.
Escrito para quem chega sem contexto (outro chat, outra máquina, outra IA).

---

## 2026-11-15 — Sondagem do hardware e base do app

**Hardware alvo:** notebook Dell (Win 11 26300) com host USB4 Intel `PCI\VEN_8086&DEV_A73E` (fw 23.3,
DROM "INTEL Gen12") → dock WIKINGOO WKG-L19 (Intel JHL7440 Titan Ridge DD, roteador
`USB4\VID_8086&PID_15EF`, fw 67.0, DROM "Intel / Module 2 PD 85W") → switch PCIe 15EF com 3 portas
downstream: GPU RX 7600 XT (atrás do switch interno 1478/1479 da placa), xHCI 15F0 (USB-A do dock),
porta TB extra (vazia). O xHCI do dock negocia PCIe Gen1 x4 internamente.

**Toolchain (importante para recompilar):** só existe o Rust `stable-x86_64-pc-windows-gnu`
self-contained, sem MinGW nem MSVC. Dois problemas e as correções, em `~/.cargo/config.toml` do
usuário (fora do repo):
1. `dlltool.exe` embutido falha com "CreateProcess" porque precisa de `as.exe`, que não vem no pacote.
   → `rustup component add llvm-tools`; copiar `llvm-ar.exe` como `~/.cargo/tools/llvm-dlltool.exe`
   e passar `-Cdlltool=...`.
2. O `ld` do GNU não entende os import libs do LLVM: o exe crasha com 0xC0000005 **antes do main**
   (módulo "unknown" no Application Error 1000). Evidência: os executáveis tinham
   MajorLinkerVersion 2.44 (GNU ld) mesmo com `-fuse-ld=lld`. → linkar com `rust-lld.exe`
   (`linker = .../bin/rust-lld.exe`, `-Clinker-flavor=ld.lld -Clink-self-contained=yes`).
   O exe passou a ter MajorLinkerVersion 14 e roda.

**ADLX:** o crate `adlx` 0.0.0-alpha.1 (Traverse Research) traz bindings pré-gerados e só depende de
`libloading` → usado em vez de FFI própria. Funciona: temp, hotspot, TBP, clocks, fan, VRAM.
`Power` (GPU power) não é suportado nesta placa; `TotalBoardPower` é. O ADLX vê o barramento como
**PCIe 3.0 x4** (o túnel), enquanto o PnP mostra Gen4 x8 (enlace interno da placa).
Ordem de drop importa: soltar objetos ADLX antes do helper, senão o Terminate reclama.

**Chaves USB4 `{5DF7E321-1C1B-4CE2-B4FA-55F4A5BC2CB6}` — decodificação empírica** (dock vs host):
| pid | dock | host | leitura |
|---|---|---|---|
| 9/10 | Intel / Module 2 PD 85W | INTEL / Gen12 | fabricante/modelo DROM (certo) |
| 16/17/18 | 0x8086 / 0x15EF / 6 | 0x8087 / 0xA73E / 1 | VID/DID/REV do roteador = HardwareIds (certo) |
| 13/14 | 0x8086 / 0x0011 | 0x8087 / 0x463E | vendor/model ID do DROM (forte) |
| 11 | 01 00 00 00 00 00 00 | zeros | route string: dock na porta 1 do host (forte) |
| 15 | 1 | 0 | profundidade na árvore (forte) |
| 8 | true | false | roteador TBT3 legado; só o dock tem CompatibleId TBT3 (forte) |
| 20 | 0x8 | — | **velocidade por faixa**: no spec USB4 (Lane Adapter CS1) 0x8=Gen2 10G, 0x4=Gen3 20G (hipótese) |
| 21 | 2 | — | faixas ativas (hipótese) |
| 2, 12, 19, 22–24, 7 | 7, 0x4D0001, 16, 0, 2, 2 | 15, 0x4D0001, 32, 0, 0, 2, 0 | desconhecidas |

**Teste de banda D3D12 (fila COPY, 4 × 1 GiB):** host→GPU **1,58 GB/s**, GPU→host **1,86 GB/s**,
estável. O esperado para TB3 a 40 Gb/s é 2,5–2,8 GB/s. Junto com a chave 20 = 0x8 isso aponta
para **enlace a 20 Gb/s (2 faixas × 10 Gb/s)** — cabo ou negociação, não a GPU.
**Teste que confirma a decodificação:** trocar por um cabo TB3/TB4 40 Gb/s certificado (passivo
≤ 0,8 m). Se a chave 20 virar 0x4 e a banda subir para ~2,5 GB/s, a hipótese está provada.

**Quedas reais do dock em 2026-11-15 (madrugada), do Event Log:**
- 01:42:14 Kernel-PnP 1010: switch PCIe do dock removido (24 dependentes) + Kernel-Power 105
  (mudança de fonte) no mesmo segundo; 01:42:15 Overwatch grava `LastTDR=1794717735`; 01:42:21
  roteador TB do dock removido; reinício às ~01:43:35.
- 01:44:43 Kernel-PnP 1010 (switch e roteador) **e** Kernel-Power 105 `AcOnline=false` no mesmo segundo.
- 01:45:03 e 01:46:28 Kernel-PnP **1011** "removido inesperadamente, relatado como falhando".
- Leitura: a energia PD do dock para o note some junto com o enlace → o dock inteiro apaga/reseta
  (fonte ATX, controlador do dock, PD), não é só o túnel PCIe. O painel classifica cada queda por
  isso (PD caiu junto ou não).
- Nenhum Display 4101 (TDR real) em 30 dias: o "TDR" do Overwatch é o dispositivo D3D sumindo.

**Feito:** `pnp.rs` (cfgmgr32, topologia, D0/D3, chaves USB4), `history.rs` (Event Log com backfill
de 30 dias + EvtSubscribe, CM_Register_Notification ALL_DEVICE_INSTANCES, LastTDR do Overwatch,
JSONL append-only em `%LOCALAPPDATA%\eGPU-Painel\historico.jsonl`), `collect.rs` (ADLX, PDH Copy/3D,
WMI BatteryStatus, só com janela aberta), `bench.rs`, `tray.rs` (Win32 puro, GetMessageW bloqueante).
`--dump` e `--bench` funcionam no console.

**Pendente:** UI egui (topologia animada, cards do dock, gráficos, linha do tempo), verificação na
tela, medição de RAM/CPU (aberto e na bandeja), README.

## 2026-11-15 (tarde) — UI completa, temas, dois processos, fonte

**Feito**
- UI egui com abas (Visão geral / Cabo e energia / Desempenho / Histórico / Opções). Topologia animada
  com ilustrações vetoriais (notebook, dock com LED, placa com fans girando pela RPM, portas USB-A/C).
- 5 temas: claro/escuro manuais; AMD/NVIDIA/Intel automáticos pelo Vendor ID da GPU do dock. AMD no
  molde do Adrenalin (barra com bloco vermelho, abas grandes, cartões retos com faixa preta); NVIDIA e
  Intel com menu lateral. Logotipos oficiais (SVG do Wikimedia Commons em `assets/logos`, resvg na
  escala nativa). Fonte Segoe UI Variable, quebra de linha global, DPI + zoom em Opções.
- Revisão por 2 agentes de "psicologia de UI": veredito no topo com causa provável, vermelho só para
  problema (cor de dados separada da marca), D3 = "Em repouso" neutro, menos jargão, mais contraste.
- Coletor otimizado por agente: topologia completa só por evento PnP ou a cada 30 s, WMI a cada 5 s;
  ~9 → ~2 ms CPU/s. Modo `--perfil`.
- Bandeja e painel em processos separados: bandeja 3,7 MB WS / 2,2 MB privados / 0% CPU, única que
  grava `historico.jsonl`; painel `--painel` lê por tail (1×/s) e morre ao fechar. Linha do JSONL
  escrita com um `write_all` só (dois processos acrescentam).
- Fonte do dock: modelo digitado → busca (curl.exe, DuckDuckGo/Bing + até 4 páginas de resultado) →
  W, +12 V (aceito só se A×12 ∈ [0,88; 1,02]×W), 80 PLUS (≥2 citações ou no nome). Carga ao vivo =
  TBP medido + PD/dock/portas estimados; pico transitório 1,6×190 W; consumo na tomada pela eficiência
  do selo. Modo `--fonte "MODELO"`. Testado: RM650x 650 W Gold; SF750 750 W 62,5 A Platinum.
- Bug achado: o egui guarda estilos separados para claro/escuro e seguia o tema do Windows (claro),
  deixando campos/botões brancos no tema escuro. Corrigido com `ctx.set_theme` em `apply_theme`.
- Capturas: `shot.ps1` usa PrintWindow (PW_RENDERFULLCONTENT) — funciona com a janela coberta.
  Cliques via PostMessage NÃO chegam ao egui; por isso existem `EGPU_TEMA` e `EGPU_ABA`.

**Pendente / não verificado**
- Tail do histórico no painel não foi exercitado com um evento real ao vivo (só o caminho de leitura).
- +12 V não aparece em muitas fontes (fica vazio, pede a etiqueta). Busca na web é frágil por natureza.
- Aviso "ADLX terminated with outstanding objects" ao encerrar continua (inofensivo).
- E-mail do git não é noreply: trocar antes de tornar o repositório público.
- Ideia mapeada (do usuário): página de métricas no estilo "Desempenho → Métricas" do Adrenalin
  (anel + número grande + sparkline por linha).

**Próximo**
- Trocar o cabo por um TB3/TB4 40 Gb/s certificado e ver se a chave 20 vai a 0x4 e a banda a ~2,5 GB/s.
- Informar o modelo real da fonte do dock e conferir se os picos explicam as quedas com PD caindo junto.

## 2026-11-16 — três rodadas de polimento por workflow (projeto → implementação em série → revisão)

**Feito** (25 commits, de c83ba5b a 98f34e5)
- Fonte do dock: cartão em grade com barra de composição da carga (placa medida; notebook/dock/portas
  hachurados como estimativa), "uso pesado" e "picos rápidos", risco estimado em âmbar ⚠ (vermelho só
  para queda medida). Um único `psu::compute` por quadro. Na topologia, nó da fonte embaixo do dock com
  ligação de energia animada; sem fonte informada, nó fantasma "+ Informar a fonte".
- Painel lateral: herói com a ilustração do item, pílula de estado (o do Dock segue o veredito),
  grade de blocos com sparkline, técnico recolhido; tópicos "Como funciona" em blocos com ícone. PCIe
  rotulado sem contradição (túnel até o notebook 3.0 x4 × placa↔dock Gen4 x8).
- Visual: elevação/sombras por tema, brilho na seleção (neutro, nunca vermelho de marca), verde de estado
  separado do verde NVIDIA, links neutros no AMD, cor de dado nos gráficos, vírgula decimal, espaço não
  separável entre número e unidade. Banda medida mostrada em Gb/s junto da conexão.
- Equilíbrio: `eq_cols` iguala a altura dos cartões de uma linha (medida do quadro anterior, tolerância
  1 px) e `flex()` faz um elemento de cada cartão ocupar a sobra (sem vão interno). Blocos em grade
  uniforme; topologia e cabeçalho responsivos (980 pt com painel aberto, abas viram ícones).
- Janela: barra de título na cor do tema via DWM (HWND do eframe; modo escuro via winit SetTheme);
  rodapé com versão/MIT, Feedback (Issues), Apoiar (GitHub Sponsors) e Pix (LivePix); `.github/FUNDING.yml`.
- Testes: `EGPU_TEMA=amd|nvidia|intel|claro|escuro`, `EGPU_ABA`, `EGPU_SEL=gpu|dock|host|cable|psu|port0..2`;
  `scratchpad/f1/cap.sh` captura em largura estreita (SetWindowPos).

**Pendente / não verificado**
- Cliques, hover, tooltips e animações de entrada não foram vistos (eventos simulados não chegam ao egui).
- Com tema explícito o winit não acompanha troca de tema do Windows (aceito; cores DWM continuam certas).
- Usuário precisa ativar GitHub Sponsors e criar o LivePix "lightwolfman" para os links funcionarem.
- E-mail do git não é noreply: trocar antes de tornar o repositório público.

## 2026-11-15 (noite) — licença GPL-3.0 e preparação para publicar

- Licença trocada de MIT para **GPL-3.0-or-later**: `LICENSE` (texto oficial da GNU), `Cargo.toml`
  (`license`, `description`, `repository`), seção Licença do README (logos fora da concessão) e rodapé
  do painel ("código aberto (GPL-3.0)", conferido em captura real).
- Auditoria de dados pessoais (árvore + histórico inteiro): o único vazamento era o e-mail de autor nos
  commits; histórico reescrito para o e-mail noreply do GitHub, `user.email` local idem.
- GitHub Sponsors: perfil, identidade, 2FA e Stripe (banco no Brasil, BRL) concluídos; falta clicar
  **Publish** e criar as faixas. LivePix "lightwolfman" ainda não existe.

**Pendente**
- O remoto privado ainda guarda o histórico antigo (com o e-mail). Plano: renomear o repositório privado
  atual, criar `egpu-painel` novo e público e enviar só o histórico reescrito.
- Release com `.exe`: anexar as licenças das fontes embutidas pelo egui (`epaint_default_fonts`: OFL,
  Ubuntu Font License, MIT/Apache) — `cargo about` gera o arquivo.
- Aviso "Instável" ganhou botão **✔ Ciente** (reconhecer, estilo Zabbix): grava em `config.json` o início
  da última queda (`ack_ts`); o banner e a pílula do cabeçalho voltam ao verde até surgir uma queda mais
  nova. As quedas continuam no Histórico. "Dock desconectado" não é reconhecível (é estado atual).
  Conferido com clique real: banner fica verde, aba não muda, Histórico intacto.
- **Publicado**: `LightWolfMan/egpu-painel` agora é público (GPL-3.0 detectada pelo GitHub, 38 commits,
  todos com e-mail noreply). O repositório privado antigo virou `egpu-painel-old` (guarda o histórico
  com o e-mail pessoal; apagar quando quiser). Ref local `refs/backup/pre-noreply` idem — não enviar.
- Falta: clicar **Publish** no GitHub Sponsors + criar faixas; criar conta LivePix com usuário
  `lightwolfman` (se o nome não estiver livre, trocar o link em `src/ui.rs` e `.github/FUNDING.yml`).
- 2026-11-16: LivePix recusou (só para streamers). Trocado por **Pix direto com chave aleatória**: QR em
  `assets/pix-qr.png` (BR Code estático gerado e conferido decodificando o QR de volta) e seção "Apoie" no
  README; o botão Pix do rodapé e o `FUNDING.yml` apontam para `README#apoie`. Falta o dono escanear
  com o app do banco para confirmar que abre o pagamento.

## 2026-11-16 — interface em inglês + português (idioma escolhível)

- Novo `src/i18n.rs`: `t("pt", "en")` e `tf!("pt {x}", "en {x}", ..)` com as duas frases lado a lado no
  próprio lugar (sem tabela de chaves). Decimais seguem o idioma via `dec()`/`i18n::dec_sep` (vírgula em pt,
  ponto em en).
- Escolha: `EGPU_LANG=pt|en` (teste) > `config.json` `lang` (0 auto, 1 pt, 2 en; `#[serde(default)]`) >
  `GetUserDefaultUILanguage` (pt-* → português, resto → inglês). Cartão **Idioma** em Opções troca na hora.
- Traduzidos: ui.rs inteiro, rótulos/mensagens de evento (history), psu, bandeja (menu, dica, balões — relê
  o config a cada uso, então segue a troca feita no painel), collect, pnp, bench, cabeçalho do CSV.
- Ficam em pt de propósito: chaves comparadas em código (`"Tipo de dispositivo PCIe"`, confiança USB4,
  `src` "PnP ao vivo") — traduzidas só na exibição; saída de console (`--dump` etc.); nome "eGPU Painel".
- Eventos já gravados no `historico.jsonl` mantêm o texto (`msg`) no idioma em que foram gravados; o
  rótulo do tipo de evento é calculado na exibição e segue o idioma.
- Conferido em captura real (en e pt): Visão geral, Cabo e energia, Histórico, Opções — sem texto cortado.
- Pendente: linha sobre idioma no README (fica com a sessão que está reescrevendo o README).

## 2026-11-16 — GPU NVIDIA e Intel no dock (branch `gpu-vendors`)

- **Detecção pela posição, não pelo fabricante** (`pnp::pick_dock_gpu`): aceita adaptadores de vídeo PCI
  AMD `VEN_1002`, NVIDIA `VEN_10DE` e Intel `VEN_8086`, mas só o que está atrás de um switch PCIe (≥ 3 pais
  PCI: porta raiz → US do dock → DS do dock → placa) e, quando a xHCI do dock (`DEV_15F0`) existe, descendo
  do mesmo US dela. iGPU (0 pais) e dGPU soldada (só a porta raiz) ficam de fora. A porta do dock com a GPU é
  marcada pelo caminho até a placa, não por `VEN_1002`. 7 testes unitários com IDs de exemplo.
- **DXGI** (`bench::dock_adapter`): adaptador com o mesmo Vendor/Device ID do nó PnP (LUID do PDH e teste de
  banda servem a qualquer fabricante). **ADLX** só se a placa do dock for AMD, casado por `DEV_`.
- **NVML** (`src/nvml.rs`): `nvml.dll` via `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)`, sem
  dependência nova; temperatura, consumo, clocks, uso, VRAM. **Não testado em placa física.**
- **Intel / sem fonte do fabricante**: uso do motor 3D e VRAM do PDH; o resto aparece "não exposto".
- UI: nome da placa no nó vem da fonte (antes "Radeon RX 7600 XT" estava fixo no código), "Dados via
  {fonte}", "—" + "não exposto" no lugar de zeros.
- Conferido por captura real na máquina AMD, build base (586ca43) × branch: Visão geral, painel da GPU e
  Desempenho iguais (só valores ao vivo mudam). Build release e 9 testes passando.
- Pendente: `history::dev_role` ainda reconhece a GPU só por `VEN_1002&DEV_7480` (eventos de outra placa
  saem sem o rótulo de GPU; generalizar sem confundir com a função de áudio HDMI da placa). Merge em
  master fica com a sessão principal (ordem: i18n → feedback → gpu).
