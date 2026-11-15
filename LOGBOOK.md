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
