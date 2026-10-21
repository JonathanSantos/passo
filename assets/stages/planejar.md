---
agent: planejador
---
1. Leia o pedido refinado, as respostas do dev e as suposições.
2. Escreva os `criterios` de aceite: comportamentos que se verificam olhando o resultado.
3. Quebre em `passos` pequenos. Cada passo: `titulo`, `arquivos`, `teste` (o teste que prova o passo), `criterios` (os ids dos critérios que ele atende: `C1` é o primeiro critério, `C2` o segundo) e, se ajudar, `detalhe` com o como.
4. Confira: todo critério aparece nos `criterios` de pelo menos um passo; nenhum passo sai do escopo. Um critério sem passo faz o Passo recusar o plano.

## p
De 1 a 3 passos e de 1 a 3 critérios.

## m
De 2 a 6 passos. Em `notas`, as decisões de design em 3 a 5 linhas (onde fica cada coisa e por quê).

## g
Em `notas`: a arquitetura, os riscos e o que fica para depois.
Passos agrupados por área, na ordem em que um destrava o outro.
