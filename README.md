# mibBreak

Visualizador leve e aberto para câmeras **Intelbras Mibo** e outras câmeras de base Dahua (Imou, Dahua), **ao vivo pela rede local ou pela nuvem**, sem o app do fabricante, sem VPN e sem abrir portas no roteador.

*Lightweight open viewer for Intelbras Mibo and other Dahua-based cameras (Imou, Dahua), live over the LAN or the Easy4ip cloud, without the vendor app, a VPN or port forwarding. [English below](#english).*

> ⚠️ **Projeto independente.** Não tem nenhuma afiliação, aprovação ou vínculo com a Intelbras S.A. ou com a Zhejiang Dahua Technology. "Intelbras", "Mibo", "Dahua", "Imou" e "Easy4ip" são marcas dos seus donos e aparecem aqui só para identificar com quais aparelhos o programa funciona. O protocolo foi entendido por engenharia reversa para fins de **interoperabilidade com os seus próprios aparelhos**. Use apenas com câmeras suas, com o seu número de série e a sua senha. Uso pessoal e não comercial.

## O que faz

- **Mosaico de câmeras** com layouts livres: arraste, redimensione e salve quantos layouts quiser (modelos 1, 2×2, 3×3, 1+5, 4×4).
- **Modo econômico por padrão** (fluxo secundário); **HD** por câmera (botão), em tela cheia, ou sempre, se preferir.
- **Áudio** (AAC / G.711), **mudo por padrão**: liga por câmera.
- **Janela flutuante** sempre no topo (desktop) e **PiP** nativo (Android).
- **Celular**: lista vertical de câmeras, todas pausadas até você dar play; **modo horizontal** por botão, deslizando entre câmeras com play automático na que estiver na tela. Em tablet / Android TV, o mosaico.
- **Importar / exportar** a lista de câmeras, com opção de **criptografar o arquivo com senha** (AES-GCM) para mandar com segurança para outro aparelho.
- **Rede local primeiro, nuvem quando precisar**: dentro da empresa ou de casa, o vídeo vai direto pela rede; fora, pela nuvem Easy4ip. Antes de usar um IP da rede local, o app confirma pelo número de série que é a mesma câmera.
- **Reconexão automática** e **mensagens claras** quando algo dá errado (senha errada, câmera sem internet, câmera ocupada por outro app…).
- **Cadastro pelo QR code** da câmera (imagem ou texto) ou manual, com teste de conexão antes de salvar.
- Interface em monoespaço, toda no teclado (`?` mostra os atalhos), em português e inglês.

## Instalar

Baixe em [Releases](../../releases):

| Sistema | Arquivo | Observação |
|---|---|---|
| Windows 10/11 | `mibBreak_x.y.z_x64-setup.exe` | usa o WebView2, que já vem no Windows |
| Linux | `.AppImage` ou `.deb` | precisa do decodificador do GStreamer: `gst-libav` (Arch) / `gstreamer1.0-libav` (Debian/Ubuntu) |
| Android | `.apk` | permitir "instalar apps desconhecidos" |

## Uso

1. Abra o app e aperte **A** (ou "+ câmera").
2. Arraste a imagem do **QR code** da câmera, ou cole o texto dele (`{SN:...,DT:...,SC:...}`), ou digite o número de série.
3. Senha: a senha que você definiu na câmera. Se nunca trocou, é o **código de segurança (SC)** da etiqueta / QR, que o app preenche sozinho.
4. **Testar** e **salvar**.

### Uma sessão de nuvem por câmera

Cada câmera aceita **uma sessão de nuvem por vez**. Enquanto o mibBreak mostra uma câmera **pela nuvem**, o app oficial no celular pode não conseguir abrir essa mesma câmera, e vice-versa. O mibBreak avisa quando a câmera está ocupada e libera a sessão ~30 s depois que você para de assistir. Pela rede local não há essa limitação.

## Como funciona

```
 app ──UDP── nuvem Easy4ip (localiza a câmera pelo número de série)
  │            │
  └──UDP── relay ──── câmera      PTCP: "TCP sobre UDP" da Dahua
               (túnel)            RTSP → H.264/H.265 → WebCodecs (decodificação por hardware)
```

- `crates/mibbreak-core`: túnel P2P (handshake Easy4ip + PTCP confiável) e cliente RTSP, em Rust.
- `crates/mibbreak-cli`: ferramentas de linha de comando: `tunnel` (expõe o RTSP da câmera numa porta local, para VLC/ffplay), `snapshot`, `soak` (teste de estabilidade).
- `apps/mibbreak`: o app (Tauri 2: Rust + WebView do sistema).

O conhecimento do protocolo vem de [khoanguyen-3fc/dh-p2p](https://github.com/khoanguyen-3fc/dh-p2p) e [ludufre/dahua-p2p](https://github.com/ludufre/dahua-p2p). Obrigado!

### Privacidade

- As credenciais ficam **só no seu computador** (`cameras.json` na pasta de configuração do app).
- O app só fala com os servidores da nuvem Easy4ip (os mesmos que o app oficial usa) e com as suas câmeras. Não há telemetria.
- A senha nunca trafega em texto aberto: a câmera recebe só a resposta digest do RTSP, por dentro do túnel. Para localizar a câmera, a nuvem recebe apenas o número de série.

## Compilar

```bash
# requisitos: Rust (rustup), Node.js; no Linux: webkit2gtk-4.1 e gst-libav
cd apps/mibbreak && npm install && npx tauri dev     # app
cargo run -p mibbreak-cli -- --help                  # linha de comando
cargo test -p mibbreak-core                          # testes
```

## Limitações

- Gravações do cartão SD: ainda não (planejado).
- Câmeras que exigem autenticação do canal P2P (HTTP 403 em `p2p-channel`) não são suportadas.
- Testado com Intelbras iM5-SC e iMX-C.

---

## English

mibBreak shows Intelbras Mibo and other Dahua-based cameras live, on the LAN when they are reachable there and through the Easy4ip cloud relay otherwise — no vendor app, VPN or port forwarding.

**Independent project**, not affiliated with Intelbras or Dahua; trademarks belong to their owners and are used only to say which devices it works with. The protocol was reverse engineered for interoperability with **your own devices**. Personal, non-commercial use.

- Free-form mosaic layouts (drag, resize, save), economy sub stream by default, per-camera HD, audio muted by default, always-on-top floating window; on phones a vertical list (paused until you press play) and a horizontal swipe mode; Android picture-in-picture.
- Import / export the camera list, optionally encrypted with a file password.
- LAN first (address confirmed by serial number), cloud fallback, automatic reconnection, plain-language errors.
- Add cameras from their QR code; test before saving.
- **One cloud session per camera**: while mibBreak shows a camera through the cloud, the vendor phone app may not open it (and vice versa). The session is released ~30 s after you stop watching.
- Credentials stay on your machine; the app talks only to the Easy4ip cloud and your cameras. The password never travels in clear text: the camera gets an RTSP digest response through the tunnel; the cloud only sees the serial number.

Downloads: [Releases](../../releases) (Windows installer, Linux AppImage/deb — needs `gst-libav` / `gstreamer1.0-libav` —, Android APK).

Protocol knowledge from [khoanguyen-3fc/dh-p2p](https://github.com/khoanguyen-3fc/dh-p2p) and [ludufre/dahua-p2p](https://github.com/ludufre/dahua-p2p). MIT License.
