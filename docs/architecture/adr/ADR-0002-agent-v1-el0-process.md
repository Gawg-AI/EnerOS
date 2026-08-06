# ADR-0002: Agent v1 is an EL0 ELF process

- Status: Accepted architecture; capability is Not Integrated at G0/G1
- Decision: a production Agent is a signed ELF process loaded from the signed init/package bundle into its own EL0 address space.

An Agent is not an in-kernel Rust trait object, statically linked plugin, host thread, WASM placeholder or log-only simulation. Its authority is conveyed through versioned syscalls and capabilities; storage, network, secrets and control access are explicit scoped resources. Package identity, ABI version, platform, resource limits and permissions are signed.

G0/G1 artifacts intentionally contain no claimed Agent capability. The capability remains Not Integrated until the G2 process/VSpace/ELF prerequisites and G6 package/supervisor work packages have system evidence.

Rollback: any temporary research plugin remains outside release profiles and cannot be advertised as AgentOS execution.
