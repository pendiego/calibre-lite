# Calibre Lite

Gerenciador leve de biblioteca de e-books, compatível com bibliotecas do [calibre](https://calibre-ebook.com/) e com envio sem fio para o [KOReader](https://koreader.rocks/). Feito com Tauri 2, SvelteKit e Rust, sem precisar do calibre instalado.

## Recursos

- **Biblioteca compatível com o calibre**: lê e escreve `metadata.db` e a estrutura `Autor/Título (id)/`, então a mesma pasta abre nos dois programas.
- **Adicionar livros** pelo seletor de arquivos, com leitura de metadados embutidos (OPF/EPUB) e capa.
- **Metadados automáticos e manuais**: busca por título/autor/ISBN no Google Books e Open Library, com escolha de candidato e download de capa.
- **Envio sem fio para o KOReader** pelo protocolo *Smart Device App* do calibre (o app atua como o lado servidor; o KOReader é o cliente), com progresso do envio.
- Interface leve e nativa (WebView2), instalador NSIS para Windows.

## Stack

| Camada | Tecnologia |
| --- | --- |
| Shell | Tauri 2 |
| Interface | SvelteKit 2 + Svelte 5 + TypeScript |
| Backend | Rust (edição 2024): `rusqlite`, `zip`, `roxmltree`, `image`, `ureq` |

## Estrutura

```
src/                  Interface (SvelteKit, adapter-static)
src-tauri/src/
  library.rs          Leitura/escrita da biblioteca calibre (SQLite + arquivos)
  metadata.rs         Busca de metadados e capas (Google Books, Open Library)
  device.rs           Servidor do protocolo Smart Device App (KOReader)
  lib.rs              Comandos Tauri
```

## Desenvolvimento

Requisitos: [Node.js](https://nodejs.org/) 20+, [Rust](https://rustup.rs/) estável e os [pré-requisitos do Tauri](https://tauri.app/start/prerequisites/).

```bash
npm install
npm run tauri dev      # app em modo desenvolvimento
npm run check          # svelte-check
cargo test --manifest-path src-tauri/Cargo.toml
```

## Build

```bash
npm run tauri build
```

O instalador NSIS é gerado em `src-tauri/target/release/bundle/nsis/`.

## Como enviar para o KOReader

1. Abra a biblioteca no Calibre Lite.
2. No KOReader: *Ferramentas → Conexão sem fio do calibre → Conectar*.
3. Com o dispositivo conectado, marque os livros e clique em **Enviar para KOReader**.

Os dois aparelhos precisam estar na mesma rede local.

## Licença

[MIT](LICENSE)
