---
name: passo
description: Conduz um pedido de código pelo Passo, passo a passo, com o dev aprovando. Use quando pedirem algo "com o passo", ou para qualquer mudança de código, feature, bug ou projeto num repositório com a pasta .passo/.
---

# Passo

O Passo conduz o pedido por cinco etapas, sempre nesta ordem: **Refinar → Planejar → Construir → Verificar → Entregar**. Ele decide o fluxo, roda as checagens e faz os commits. Você faz o que a etapa pede, uma coisa por vez.

## Começar

```bash
{{bin}} start "<pedido do dev, com as palavras dele>" --size <pp|p|m|g>
```

- `pp`: troca pontual (um texto, uma cor, um bug de uma linha). Vai direto para construir.
- `p`: pequeno e claro, até 3 passos.
- `m`: uma feature. É o padrão.
- `g`: vários módulos ou um projeto novo.

Na dúvida entre dois tamanhos, escolha o maior.

## O ciclo

1. `{{bin}} next` mostra a tarefa da vez, o agente indicado e como entregar.
2. Faça a tarefa. Se ela indica um subagente (`passo-refinador`, `passo-planejador`, `passo-construtor`, `passo-revisor`) e você pode delegar, delegue e passe a saída inteira do `{{bin}} next` para ele. Se não pode delegar, faça você mesmo, seguindo o papel descrito na tarefa.
3. Entregue com `{{bin}} done …`. A saída já traz o próximo passo.
4. Repita até a saída dizer que é a **vez do dev** ou que o run terminou.

## Vez do dev

Quando a saída disser "vez do dev", **pare**. Mostre ao dev o que o Passo mostrou (o plano, as perguntas, a entrega) e espere. O dev responde aqui no chat ("ok", "ok 1b", "não: o que mudar") ou no terminal. Nunca responda por ele.

Depois que o dev responder, rode `{{bin}} next`.

## Regras

- Dúvida que muda o resultado: `{{bin}} ask "pergunta" "opção recomendada — porquê" "outra opção — porquê"` e pare.
- Entregas em JSON (pedido refinado, plano, revisão): escreva num arquivo temporário (`mktemp`) e rode `{{bin}} done <arquivo>`. Se o Passo apontar erros, corrija só os campos apontados e entregue de novo.
- Não faça commit nem troque de branch: o Passo faz.
- Não diga que os testes passaram: o Passo roda e mostra.
- Não leia nem altere `.passo/`. Use `{{bin}} status` e `{{bin}} plan`.
- `{{bin}} ok`, `no`, `change`, `size` e `stop` são do dev; a trava os nega para você.
- Quando a trava negar algo, leia o motivo e siga por ele.
