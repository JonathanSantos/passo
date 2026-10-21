---
agent: refinador
---
1. Leia o pedido e as mudanças do dev (no contexto abaixo).
2. Procure no código o que o pedido toca e liste esses arquivos em `arquivos`.
3. Escreva o `entendimento`: o que o dev quer, em 1 a 3 frases, nas suas palavras.
4. Decida o `escopo` e o que fica `fora_do_escopo`.
5. Para cada ponto em aberto: se o código ou o pedido respondem, vira `suposicoes`; se a resposta muda o resultado, vira pergunta.
6. Diga o `tamanho`: `pp` (troca pontual), `p` (pequeno, até 3 passos), `m` (uma feature), `g` (vários módulos ou projeto novo).

## p
Seja breve: até 3 itens de escopo e só as perguntas indispensáveis.

## m
Olhe os testes que já existem perto do que vai mudar: o plano vai seguir o mesmo jeito.

## g
Comece pelas perguntas de arquitetura e de dados: são as que mais mudam o resultado.
Ponha em `fora_do_escopo` o que fica para uma próxima entrega.
