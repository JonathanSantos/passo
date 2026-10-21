---
title: Dados e banco
match: [banco, database, sql, migração, migracao, migration, tabela, schema, coluna]
---
- Migração tem volta (down) ou o motivo de não ter.
- Coluna nova em tabela com dados tem valor padrão ou backfill.
- Consultas novas usam índice; nada de N+1 em listas.
- Escritas que precisam andar juntas estão numa transação.
- Testes cobrem o dado antigo, não só o novo.
