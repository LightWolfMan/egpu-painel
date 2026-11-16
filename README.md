# eGPU Painel

Painel desktop nativo para Windows que monitora e diagnostica um dock eGPU Thunderbolt 3, com foco na **placa do dock** (WIKINGOO WKG-L19, controlador Intel JHL7440 "Titan Ridge"). Escrito em Rust com egui/eframe (renderizador glow), sem WebView, sem runtime e sem serviço em segundo plano.

![Painel](docs/painel.png)

## O que é

O dock eGPU é uma cadeia de coisas que o Windows enxerga em pedaços: um controlador USB4 no notebook, um roteador Thunderbolt no dock, um switch PCIe com três portas downstream (a GPU, o xHCI das portas USB-A e uma porta TB extra), e por trás disso uma fonte ATX e um controlador PD que o sistema operacional nem sabe que existem. Quando o dock cai, o Windows só diz "dispositivo removido", e descobrir se foi o cabo, o enlace, a energia ou a GPU exige juntar evidência de vários lugares.

O painel faz essa junção. Ele lê a topologia PnP, decodifica as propriedades USB4 do roteador, correlaciona os eventos do Event Log (PnP, energia, vídeo, WHEA, PCI) e guarda tudo numa linha do tempo persistente. Cada queda é classificada: o PD caiu junto (o dock inteiro apagou ou resetou) ou só o enlace caiu.

A regra de projeto é **não inventar dado**. Se o Windows não expõe um valor, o painel diz que não expõe, em vez de estimar. As chaves decodificadas carregam um nível de confiança, e o que é hipótese está marcado como hipótese.

## O que ele mostra

**Veredito no topo.** Uma frase só responde "meu dock está bem?": *Tudo certo*, *Instável: caiu N vezes nas últimas 24 h* ou *Desconectado*, com a causa provável da última queda (energia caiu junto → fonte/tomada; só a conexão → cabo). A pílula do cabeçalho segue a mesma cor.

**Temas.** Claro e escuro são escolhidos à mão. No automático o painel detecta a GPU do dock pelo Vendor ID e aplica a paleta e o layout do fabricante: AMD com abas no topo no molde do Adrenalin, NVIDIA e Intel com menu lateral. O logotipo oficial aparece centralizado, como no HWiNFO. Vermelho fica reservado para problema real, e a cor de marca nunca pinta dado. A interface segue a escala de DPI do Windows e tem zoom próprio em Opções.

**Topologia animada.** Notebook, enlace, dock e GPU num desenho único. Os pulsos que correm pelo enlace usam a atividade do motor *Copy* da GPU como proxy do tráfego (são as cópias DMA host↔GPU, o que mais atravessa o túnel). A linha fina amarela representa a energia PD.

**Enlace USB4/Thunderbolt.** As propriedades `{5DF7E321-1C1B-4CE2-B4FA-55F4A5BC2CB6}` do roteador do dock e do host, decodificadas, cada uma com seu nível de confiança: *certo*, *forte*, *hipótese* ou *desconhecido* (veja a seção de decodificação abaixo). Velocidade por faixa e número de faixas aparecem como visualização de lanes.

**Switch PCIe do dock.** Estado D0/D3 de cada uma das 3 portas downstream, o que está atrás de cada uma, firmware, e o enlace PCIe que o PnP reporta.

**PD e energia.** Energia externa sim/não, bateria carregando/descarregando e taxa em mW, lidas do driver de bateria ACPI (WMI `root\wmi`).

**Estabilidade.** Quedas em 24 h, 7 e 30 dias, gráfico de quedas por dia e a classificação de cada incidente: **"PD caiu junto"** (o note perdeu a energia externa no mesmo instante, o que aponta para o dock inteiro apagando) contra **"só o enlace"**. Mostra também há quanto tempo o dock está sem cair. Eventos a até 20 s um do outro entram no mesmo incidente, porque o switch e o roteador do dock saem com segundos de diferença.

**Fluxo de dados e teste de banda.** Gauges de Copy e 3D, VRAM dedicada e memória compartilhada (que atravessa o túnel) e um teste de banda D3D12 sob demanda: cópias na fila COPY entre heap de upload/readback do host e VRAM da GPU, 8 × 128 MiB por medição, nas duas direções. Nada fica residente depois do teste.

**Fonte do dock (estimativa em tempo real).** Você digita o modelo da fonte ATX e o painel busca as specs na web (potência, +12 V, selo 80 PLUS), só quando você pede, pelo `curl.exe` do Windows. Valor de +12 V só é aceito se for coerente com a potência; selo, só com duas ou mais citações ou quando o nome do modelo diz. Tudo pode ser corrigido à mão. Com isso, calcula ao vivo a carga (potência da placa medida pelo ADLX + notebook pelo PD, dock e portas estimados), a fração da fonte, o pico transitório possível da placa, o consumo na tomada pela eficiência do selo e a corrente no +12 V, e avisa se os picos podem desarmar a fonte (o que derruba o dock inteiro, PD junto).

**GPU via AMD ADLX.** Temperatura, hotspot, TBP, clocks, ventoinha e VRAM, pelo crate `adlx` (bindings pré-gerados, carrega `amdadlx64.dll` só com a janela aberta).

**Linha do tempo persistente e "o que veio antes".** Todos os eventos ficam num arquivo JSONL. Para cada queda, o painel mostra o que aconteceu nos 2 minutos anteriores, ou avisa que não havia nada no Event Log (queda sem aviso). Exporta para CSV.

### O que o Windows não expõe

Alguns dados simplesmente não chegam ao sistema, e o painel não os fabrica. Eles aparecem numa faixa à parte, marcados como invisíveis.

- **Fonte ATX:** tensões, temperatura e potência real. A fonte não tem barramento de dados ligado ao dock; o cartão "Fonte do dock" mostra uma estimativa, rotulada como tal.
- **Ventoinha:** não há sensor de RPM ligado a um controlador visível pelo host.
- **Temperatura do JHL7440:** o Titan Ridge não publica sensor térmico ao Windows.
- **Contrato PD:** tensão e corrente negociadas ficam no controlador PD do dock; o Windows só vê "energia externa sim/não" e a taxa de carga.
- **Tráfego real:** não existe contador de bytes por adaptador USB4/TB. O motor Copy é um proxy, não uma medida.
- **Erros do enlace:** contadores de erro e retreino das faixas não são publicados. Só aparecem quedas (PnP) e erros PCIe (WHEA/AER).

## Como funciona por baixo

O objetivo é custar quase nada quando você não está olhando.

Com a janela fechada, o processo vive só na **bandeja**, em Win32 puro, bloqueado em `GetMessageW` sem timeout: zero polling, zero timers. O ícone é gerado em código (dois nós ligados, a cor do nó do dock indica o estado) e os balões de aviso saem dos callbacks de eventos.

Os eventos chegam por assinatura, nunca por varredura periódica: `EvtSubscribe` para o Event Log e `CM_Register_Notification` (todas as instâncias de dispositivo) para o PnP ao vivo. Na primeira execução há um backfill de 30 dias do Event Log; nas seguintes, só desde a última varredura. A deduplicação usa o canal e o número do registro.

A **coleta ao vivo** (ADLX, contadores PDH dos motores e da memória, WMI de bateria, releitura das propriedades PnP do dock) existe só enquanto a janela está aberta: uma thread, 1 amostra por segundo, parada ao fechar. Há também um modo de animação econômica (5 quadros por segundo) e o painel só repinta por evento ou por animação.

Fontes de eventos e IDs:

| Fonte | IDs / filtro | Vira |
|---|---|---|
| Kernel-PnP / Device Management | 1010 (removido), 1011 (removido inesperadamente, relatado como falhando) | queda do dock, GPU ou USB-A do dock |
| Kernel-Power | 105 (mudança de fonte, `AcOnline`), 42 (suspensão), 107 (retorno), 506/507 (espera moderna), 41 (reinício sem desligamento limpo) | PD perdido, suspensão, crash |
| Display | 4101 (driver de vídeo parou e se recuperou) | TDR real |
| Provedores `amdkmdag`, `amdwddmg`, `amduw23g` | níveis crítico, erro e aviso | erro do driver AMD |
| WHEA-Logger e Kernel-WHEA/Errors | todos | erro de hardware |
| PCI/Operational | níveis 1–3 e ID 2 (transição cancelada) | erro PCIe ou informativo |
| Kernel-General, EventLog | 12/13 (boot/desligamento), 6008 (desligamento inesperado) | contexto do sistema |
| Overwatch, `Settings_v0.ini` | `LastTDR` | "TDR relatado pelo jogo" |
| PnP ao vivo | chegada e saída de dispositivos | linha do tempo com estado capturado no instante |

Sobre o `LastTDR` do Overwatch: ele não é um TDR do Windows. É o jogo registrando que perdeu o dispositivo D3D, o que acontece tanto num TDR quanto quando a GPU some do barramento. Nos dados do autor não houve nenhum Display 4101 em 30 dias, apesar de várias quedas.

## Decodificação empírica das chaves USB4

O Windows publica propriedades USB4 do roteador sob o GUID `{5DF7E321-1C1B-4CE2-B4FA-55F4A5BC2CB6}`, sem documentação. A tabela abaixo foi montada comparando o dock com o host (valores de um dock WKG-L19 no host Intel Gen12) e é a base da função `decode_usb4`.

| pid | dock | host | leitura | confiança |
|---|---|---|---|---|
| 9 / 10 | Intel / Module 2 PD 85W | INTEL / Gen12 | fabricante e modelo do DROM | certo |
| 16 / 17 / 18 | 0x8086 / 0x15EF / 6 | 0x8087 / 0xA73E / 1 | VID / DID / REV do roteador (batem com os HardwareIds) | certo |
| 13 / 14 | 0x8086 / 0x0011 | 0x8087 / 0x463E | vendor e model ID do DROM | forte |
| 11 | `01 00 00 00 00 00 00` | zeros | route string: dock na porta 1 do host | forte |
| 15 | 1 | 0 | profundidade na árvore USB4 | forte |
| 8 | true | false | roteador TBT3 legado (só o dock tem CompatibleId TBT3) | forte |
| 20 | 0x8 | — | velocidade por faixa; no spec USB4 (Lane Adapter CS1) 0x8 = Gen2 10 Gb/s, 0x4 = Gen3 20 Gb/s | **hipótese** |
| 21 | 2 | — | faixas ativas | **hipótese** |
| 2, 12, 19, 22–24, 7 | 7, 0x4D0001, 16, 0, 2, 2 | 15, 0x4D0001, 32, 0, 0, 2, 0 | sem leitura conhecida | desconhecido |

### A chave 20 e o teste de banda

A chave 20 vale 0x8 no dock. Se ela é o campo "Current Link Speed" do spec USB4, isso significa 10 Gb/s por faixa; com 2 faixas (chave 21), o enlace estaria a **20 Gb/s**, e não a 40 Gb/s que o TB3 entrega.

O teste de banda D3D12 concorda. Medido na fila COPY, 4 × 1 GiB, estável:

| Direção | Medido | Esperado em TB3 a 40 Gb/s |
|---|---|---|
| host → GPU | 1,57–1,58 GB/s | 2,5–2,8 GB/s |
| GPU → host | 1,86 GB/s | 2,5–2,8 GB/s |

Os dois indícios apontam para o mesmo lado: o gargalo é o enlace (cabo ou negociação), não a GPU. Mas é uma hipótese consistente, não uma prova.

**O teste que prova:** trocar por um cabo TB3/TB4 de 40 Gb/s certificado (passivo e curto, até 0,8 m). Se a chave 20 passar a valer **0x4** e a banda subir para perto de 2,5 GB/s, a decodificação está confirmada. Se a chave não mudar, a hipótese cai e a chave volta para "desconhecido".

## Consumo medido

Bandeja e janela são **processos separados**. A bandeja é mínima: fica bloqueada em `GetMessageW`, escuta o Event Log e o PnP por callback e é a única que grava o histórico. A janela (`--painel`) é um processo filho que lê o histórico, acompanha as linhas novas e **termina ao fechar**, devolvendo ao sistema toda a memória de egui, OpenGL e ADLX.

Medido no release, Windows 11, 12 threads lógicas:

| Estado | Working set | Privado | CPU |
|---|---|---|---|
| Bandeja | 3,7 MB | 2,2 MB | 0 ms em 30 s (0%) |
| Janela aberta (processo `--painel`) | ~120 MB | ~120–140 MB | ~19% de um núcleo (1,6% da máquina), quase tudo redesenho das animações a 15 qps |
| Janela fechada | o processo do painel deixa de existir | — | — |

O coletor em si (ADLX, PDH, WMI, topologia) custa ~2 ms de CPU por segundo (`--perfil`). Sem foco, ou no modo econômico, a janela redesenha a 5 qps.

## Compilando

Requer Rust (edição 2024) no Windows. Com o toolchain **MSVC** (`stable-x86_64-pc-windows-msvc` e as Build Tools do Visual Studio), basta `cargo build --release`; nada desta seção é necessário.

A seção é para o toolchain `stable-x86_64-pc-windows-gnu` **sem MinGW e sem MSVC instalados** (só o Rust self-contained). Ali há dois problemas, os dois resolvidos no `config.toml` do cargo do usuário, fora do repositório:

1. O `dlltool.exe` embutido falha com "CreateProcess" porque precisa do `as.exe`, que não vem no pacote. A saída é usar o do LLVM: `rustup component add llvm-tools`, copiar o `llvm-ar.exe` do componente para `%USERPROFILE%\.cargo\tools\llvm-dlltool.exe` e apontar `-Cdlltool` para ele.
2. O `ld` do GNU não entende os import libs gerados pelo LLVM, e o executável crasha com `0xC0000005` **antes do `main`** (Application Error 1000, módulo "unknown"). A evidência era o campo MajorLinkerVersion do exe marcando 2.44 (GNU ld) mesmo pedindo lld. Linkando com o `rust-lld`, o exe passa a ter MajorLinkerVersion 14 e roda.

Em `%USERPROFILE%\.cargo\config.toml` (o cargo aceita barras normais; troque `SEU_USUARIO` pelo seu perfil):

```toml
[target.x86_64-pc-windows-gnu]
linker = "C:/Users/SEU_USUARIO/.rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/bin/rust-lld.exe"
rustflags = [
    "-Cdlltool=C:/Users/SEU_USUARIO/.cargo/tools/llvm-dlltool.exe",
    "-Clinker-flavor=ld.lld",
    "-Clink-self-contained=yes",
]
```

Depois, `cargo build --release`. O binário sai em `target\release\egpu-painel.exe`. O perfil release usa `opt-level = 3`, LTO thin, `strip` e `panic = "abort"`.

## Uso

```
egpu-painel.exe                bandeja + abre a janela
egpu-painel.exe --bandeja      inicia só na bandeja, sem abrir a janela
egpu-painel.exe --painel       só a janela (é o que a bandeja dispara)
egpu-painel.exe --bench        roda o teste de banda D3D12 e imprime no console
egpu-painel.exe --dump         imprime topologia e chaves USB4 decodificadas (dock e host)
egpu-painel.exe --perfil [N]   mede o custo de cada parte do coletor, sem janela
egpu-painel.exe --fonte "MODELO"  busca as specs de uma fonte e imprime o que achou
egpu-painel.exe --icone ARQ.ico   grava o ícone do painel (16/32/48/256 px), útil para atalhos
```

Para instalar: copie `egpu-painel.exe` para uma pasta fixa (ex.: `%LOCALAPPDATA%\Programs\eGPU-Painel`), gere o ícone com `--icone` e crie um atalho apontando para o exe com esse ícone. O exe é autossuficiente (logotipos e ícones embutidos).

Variáveis para testes: `EGPU_TEMA=amd|nvidia|intel` simula a placa detectada; `EGPU_ABA=0..4` abre direto numa aba.

`--bench`, `--dump`, `--perfil` e `--fonte` se anexam ao console de quem chamou, então rode-os a partir de um terminal. Só uma instância roda por vez: abrir uma segunda pede à primeira que mostre a janela.

Os dados ficam em `%LOCALAPPDATA%\eGPU-Painel\`:

| Arquivo | Conteúdo |
|---|---|
| `historico.jsonl` | linha do tempo, um evento JSON por linha, só acrescenta |
| `estado.json` | início da última varredura do Event Log e dispositivos já vistos atrás da USB-A |
| `config.json` | preferências (tema, zoom, avisos, modo econômico) |
| `fonte.json` | modelo e specs da fonte do dock |
| `historico-AAAAMMDD-HHMMSS.csv` | exportações da linha do tempo (BOM, para o Excel abrir os acentos) |

A aba **Opções** tem o tema (automático pela placa, claro ou escuro), a escala de DPI detectada e o tamanho da interface, e ainda:

- **Iniciar com o Windows (na bandeja):** grava um valor em `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` apontando para `--bandeja`. Não precisa de administrador.
- **Avisar na bandeja quando o dock cair:** liga e desliga os balões.
- **Animação econômica (5 qps):** menos quadros por segundo na animação.
- **Exportar linha do tempo (CSV)**, **Abrir pasta de dados** e **Rodar teste de banda**.

O painel só lê: não instala driver, não escreve no sistema além da chave de inicialização opcional e dos arquivos acima.

## Licença

MIT. Os logotipos em `assets/logos/` (AMD, NVIDIA, Intel) vêm do Wikimedia Commons e são marcas registradas dos respectivos donos. O painel os usa só para identificar o fabricante da placa detectada, como fazem ferramentas de diagnóstico.
