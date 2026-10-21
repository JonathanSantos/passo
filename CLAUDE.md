# Passo

Harness em Rust para conduzir agentes de IA do pedido à entrega. A arquitetura está em `docs/ARQUITETURA.md`.

- Código, nomes e comentários em inglês; textos para quem usa (CLI, painel, agentes) em português.
- Etapas, tamanhos e agentes têm nomes comuns. Não crie jargão.
- O fluxo mora em `passo-core` e é puro: nada de IO, relógio ou git ali. IO entra como fatos num `Command` e sai como `Effect`.
- Plataformas (Claude Code, Copilot) só existem em `passo-adapters`.
- Antes de commitar: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
