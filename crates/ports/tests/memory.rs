//! The `Memory` implementations pass the conformance suites they define; and the suites fail what
//! they are meant to fail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aap_ports::memory::{
    MemoryActivity, MemoryAuthenticator, MemoryDirectory, MemoryObjects, MemoryRuntime, MemoryStore,
};

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

mod objects {
    use super::*;

    async fn make() -> Option<MemoryObjects> {
        Some(MemoryObjects::new())
    }
    aap_ports::agent_objects_conformance!(make);
}

mod authenticator {
    use super::*;

    async fn make() -> Option<MemoryAuthenticator> {
        Some(MemoryAuthenticator::new())
    }
    aap_ports::authenticator_conformance!(make);
}

mod activity {
    use super::*;

    async fn make() -> Option<MemoryActivity> {
        Some(MemoryActivity::new())
    }
    aap_ports::activity_source_conformance!(make);
}

mod waker {

    async fn make() -> Option<aap_ports::memory::MemoryWaker> {
        Some(aap_ports::memory::MemoryWaker::new())
    }
    aap_ports::waker_conformance!(make);
}

mod upstream {

    async fn make() -> Option<aap_ports::memory::MemoryUpstream> {
        Some(aap_ports::memory::MemoryUpstream::new())
    }
    aap_ports::upstream_conformance!(make);
}

mod cards {

    async fn make() -> Option<aap_ports::memory::MemoryCards> {
        Some(aap_ports::memory::MemoryCards::new())
    }
    aap_ports::card_source_conformance!(make);
}
