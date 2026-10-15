//! The fixtures of the kind end-to-end of the coder (`deploy/operator/tests/e2e/`, S9) are valid and resolve, with no cluster,
//! so a broken fixture is not found by CI minutes into a job that pulls a 3 GB image: the AgentConfig and AgentService of
//! `coder.yaml` pass `aap_domain::resolve`, run the pinned real image on the token variant with the GitHub MCP sidecar, and
//! reference only the Secrets the script `coder-e2e.sh` makes.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::path::Path;

use aap_api::{AgentConfig, AgentService};
use serde::Deserialize;

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/operator/tests/e2e")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn coder() -> (AgentService, AgentConfig) {
    let text = read("coder.yaml");
    let (mut service, mut config) = (None, None);
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let value = serde_yaml::Value::deserialize(doc).expect("a YAML document");
        match value["kind"].as_str() {
            Some("AgentService") => {
                service = Some(serde_yaml::from_value(value).expect("an AgentService"))
            }
            Some("AgentConfig") => {
                config = Some(serde_yaml::from_value(value).expect("an AgentConfig"))
            }
            other => panic!("coder.yaml holds a {other:?}"),
        }
    }
    (
        service.expect("an AgentService"),
        config.expect("an AgentConfig"),
    )
}

#[test]
fn the_coder_fixture_resolves_to_the_real_image_with_its_sidecar() {
    let (service, config) = coder();
    let resolved = aap_domain::resolve(
        &service,
        &config,
        &aap_domain::Externals::default(),
        aap_ports::OwnerHandle::none(),
    )
    .unwrap_or_else(|issues| panic!("{issues:?}"));

    let workload = &resolved.runtime.workloads[0];
    let image = &workload.container.image;
    assert!(
        image.starts_with("ghcr.io/vymalo/another-adam-rs/coder:sha-")
            && image.contains("@sha256:")
            && image.len() == "ghcr.io/vymalo/another-adam-rs/coder:sha-0000000@sha256:".len() + 64,
        "the image is the real coder, by tag and digest: {image}"
    );
    assert_eq!(
        workload.sidecars.len(),
        1,
        "the GitHub MCP server is a sidecar"
    );
    assert_eq!(workload.sidecars[0].name, "github-mcp");
    assert_eq!(&workload.sidecars[0].image, image, "from the same image");

    let env: Vec<&str> = workload
        .container
        .env
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for needed in [
        "DATABASE_URL",
        "A2A_BEARER_TOKENS",
        "MODEL_BASE_URL",
        "MODEL_API_KEY",
        "GITHUB_TOKEN",
        "GITHUB_MCP_URL",
        "PUBLIC_URL",
    ] {
        assert!(env.contains(&needed), "{needed} is not in {env:?}");
    }
    assert!(
        !env.iter().any(|n| n.starts_with("GITHUB_APP")),
        "the token variant, not the App: {env:?}"
    );
}

#[test]
fn the_fixture_names_the_secrets_the_script_makes_and_the_postgres_it_runs() {
    let script = read("coder-e2e.sh");
    let (service, config) = coder();
    let yaml = serde_yaml::to_string(&(&service, &config)).unwrap();
    for name in ["coder-secrets", "coder-db-uri"] {
        assert!(
            script.contains(&format!("secret {name}")),
            "the script makes no Secret {name}"
        );
        assert!(yaml.contains(name), "the fixture does not use {name}");
    }
    // The URI of the script points at the Service of postgres.yaml.
    let postgres = read("postgres.yaml");
    assert!(postgres.contains("name: coder-postgres"));
    assert!(script.contains("coder-postgres.$ns.svc:5432/coder"));
    assert!(postgres.contains("POSTGRES_DB") && postgres.contains("value: coder"));
    // Every image the fixtures run is pinned by digest.
    for text in [&postgres, &read("coder.yaml")] {
        let named = text.lines().filter(|l| {
            let l = l.trim_start();
            (l.starts_with("image:") || l.starts_with("ref:")) && !l.trim_end().ends_with(':')
        });
        for line in named {
            assert!(line.contains("@sha256:"), "not pinned by digest: {line}");
        }
    }
    assert!(
        script.contains("@sha256:"),
        "the curl image is pinned by digest"
    );
}

/// The default agent image the chart ships (AD-029) is the image this end-to-end runs the coder in: the pin is proven to start
/// under the operator, and an agent that names none runs exactly what the fixture runs. Moving one means moving, and proving, the
/// other.
#[test]
fn the_chart_ships_the_image_the_end_to_end_runs_as_its_default_agent_image() {
    let values = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/operator/values.yaml");
    let values: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(&values).unwrap()).unwrap();
    let default = values["defaultAgentImage"]
        .as_str()
        .expect("the chart has a defaultAgentImage");
    assert!(
        default.contains(":sha-") && default.contains("@sha256:"),
        "tag and digest: {default}"
    );

    let (service, config) = coder();
    let pinned = config
        .spec
        .environment
        .image
        .as_ref()
        .expect("the fixture pins its image")
        .reference
        .clone();
    assert_eq!(
        pinned, default,
        "the chart's defaultAgentImage and the image of tests/e2e/coder.yaml are one pin: move both, and run the end-to-end"
    );

    // The fixture with no image, on an operator given the chart's default: the same agent, digest included.
    let owner = aap_ports::OwnerHandle::none;
    let by_image = aap_domain::resolve(
        &service,
        &config,
        &aap_domain::Externals::default(),
        owner(),
    )
    .unwrap();
    let mut imageless = config.clone();
    imageless.spec.environment.image = None;
    let by_default = aap_domain::resolve(
        &service,
        &imageless,
        &aap_domain::Externals::default().with_default_image(default),
        owner(),
    )
    .unwrap();
    assert_eq!(by_default, by_image);
}
