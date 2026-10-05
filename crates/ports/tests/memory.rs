//! The `Memory` implementations pass the conformance suites they define; and the suites fail what
//! they are meant to fail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aap_ports::memory::{MemoryDirectory, MemoryRuntime, MemoryStore};

mod runtime {
    use super::*;

    async fn make() -> Option<MemoryRuntime> {
        Some(MemoryRuntime::new())
    }
    aap_ports::runtime_provider_conformance!(make);
}

mod runtime_without_suspend {
    use super::*;

    async fn make() -> Option<MemoryRuntime> {
        Some(MemoryRuntime::new().without_suspend())
    }
    aap_ports::runtime_provider_conformance!(make);
}

mod store {
    use super::*;

    async fn make() -> Option<MemoryStore> {
        Some(MemoryStore::new())
    }
    aap_ports::store_provisioner_conformance!(make);
}

mod store_without_cnpg {
    use super::*;

    async fn make() -> Option<MemoryStore> {
        Some(MemoryStore::new().without_cnpg())
    }
    aap_ports::store_provisioner_conformance!(make);
}

mod directory {
    use super::*;

    async fn make() -> Option<MemoryDirectory> {
        Some(MemoryDirectory::new())
    }
    aap_ports::agent_directory_conformance!(make);
}
