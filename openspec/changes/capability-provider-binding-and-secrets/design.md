# Design

Capability is the stable product contract, Provider supplies it, Binding
connects it to a project, and Resource is the operational entity. Every
capability supports managed, external, and adopted modes. Generic bindings
provide portable environment contracts such as `DATABASE_URL`; deeper bindings
may configure frameworks or migrations. SecretStore uses encrypted PostgreSQL
with a master key outside the database in MVP and keeps values outside LLM
context.

