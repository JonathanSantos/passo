---
title: API HTTP
match: [api, endpoint, rota, route, rest, http, graphql, webhook]
---
- Entrada inválida responde 4xx com uma mensagem clara, nunca 500.
- Recurso que não existe responde 404; sem permissão, 401 ou 403.
- Listas têm paginação ou um limite.
- Nada de segredo, token ou dado pessoal em log ou em resposta de erro.
- Mudou o contrato? Os clientes e a documentação acompanham.
