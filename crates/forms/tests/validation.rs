//! Validation: the screens' rules and the operator's, each error with its form field.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use aap_domain::Externals;
use aap_forms::{
    AgentForm, AgentKind, AgentKindForm, BaseUrlForm, Deployment, FormError, FormTarget,
    HeaderForm, ImagePolicy, ModelEndpointForm, OfferedKeys, SecretPick, ToolServerForm,
    from_object_errors, validate_agent, validate_model, validate_tool_server,
};
use aap_ports::{FieldError, Kind, OfferedSecret};
use proptest::prelude::*;
use support::*;

fn externals() -> Externals {
    aap_forms::externals(
        &[stored(
            Kind::ModelEndpoint,
            "gateway",
            model_form("gateway").draft().spec,
            true,
        )],
        &[stored(
            Kind::ToolProvider,
            "websearch",
            tool_form("websearch").draft().spec,
            true,
        )],
        None,
    )
}

fn deployment(kind: AgentKind, name: &str) -> Deployment {
    defaults().deployment_for(kind, name)
}

fn check(form: &AgentForm) -> Result<aap_forms::Resources, Vec<FormError>> {
    validate_agent(
        form,
        &deployment(form.kind(), &form.name),
        NS,
        &externals(),
        &offered(),
        &ImagePolicy::any_registry(),
    )
}

fn field_names<T>(result: Result<T, Vec<FormError>>) -> Vec<String> {
    match result {
        Ok(_) => Vec::new(),
        Err(errors) => errors.into_iter().map(|e| e.field).collect(),
    }
}

fn fields(result: Result<aap_forms::Resources, Vec<FormError>>) -> Vec<String> {
    field_names(result)
}

fn coder_settings(form: &mut AgentForm) -> &mut aap_forms::CoderSettings {
    match &mut form.agent {
        AgentKindForm::Coder(c) => c,
        AgentKindForm::Folder(_) => panic!("a folder"),
    }
}

fn folder_files(form: &mut AgentForm) -> &mut std::collections::BTreeMap<String, String> {
    match &mut form.agent {
        AgentKindForm::Folder(f) => &mut f.files,
        AgentKindForm::Coder(_) => panic!("a coder"),
    }
}

// --------------------------------------------------------------- valid forms

#[test]
fn the_example_forms_are_valid_and_the_operator_resolves_them() {
    for form in [coder_form("coder-me"), folder_form("chat")] {
        let resources = check(&form).unwrap_or_else(|e| panic!("{e:?}"));
        // What the dashboard accepts, the operator can run: the same objects resolve into a runtime.
        let resolved = aap_domain::resolve(
            &resources.service,
            &resources.config,
            &externals(),
            aap_ports::OwnerHandle::none(),
        );
        assert!(resolved.is_ok(), "{:?}", resolved.err());
    }
}

proptest! {
    #![proptest_config(config(128))]

    /// The dashboard and the operator agree: an agent the dashboard accepts resolves.
    #[test]
    fn what_the_dashboard_accepts_the_operator_resolves((form, _d) in arb_agent()) {
        // A tool server the generated form names must exist, and a coder's secret key must be offered.
        let tools: Vec<_> = form
            .tool_servers
            .iter()
            .map(|n| stored(Kind::ToolProvider, n, tool_form(n).draft().spec, true))
            .collect();
        let models = [stored(Kind::ModelEndpoint, &form.model.endpoint, model_form(&form.model.endpoint).draft().spec, true)];
        let externals = aap_forms::externals(&models, &tools, None);
        let offer: Vec<OfferedSecret> = form.secret_picks().iter().map(|(_, p)| offer(&p.secret, &p.key)).collect();
        let offered = OfferedKeys::from_offered(aap_ports::OFFER_LABEL, &offer);
        let mut form = form;
        form.allow_insecure_http = true;
        let deployment = defaults().deployment_for(form.kind(), &form.name);
        // The generated names are valid by construction except where a rule has a bound; keep to the ones that pass.
        if let Ok(resources) = validate_agent(&form, &deployment, NS, &externals, &offered, &ImagePolicy::any_registry()) {
            let resolved = aap_domain::resolve(&resources.service, &resources.config, &externals, aap_ports::OwnerHandle::none());
            prop_assert!(resolved.is_ok(), "{:?}", resolved.err());
        }
    }

    /// Every error says where: a field of the form, or a deployment field, and a form with none is not refused.
    #[test]
    fn every_error_has_a_field((form, d) in arb_agent(), noise in 0usize..8) {
        let mut form = form;
        // Make the form wrong in one more way, by position.
        match noise {
            0 => form.name = "Bad Name".to_owned(),
            1 => form.title = String::new(),
            2 => form.model.endpoint = "nothere".to_owned(),
            3 => form.audience = Some(vec!["a b".to_owned()]),
            4 => form.image = Some("has space".to_owned()),
            5 => { form.tool_servers.insert("missing".to_owned()); }
            6 => form.tags = vec!["Bad".to_owned()],
            _ => form.allow_insecure_http = false,
        }
        let result = validate_agent(&form, &d, NS, &Externals::default(), &OfferedKeys::default(), &ImagePolicy::any_registry());
        let errors = result.err().unwrap_or_default();
        prop_assert!(!errors.is_empty());
        for e in &errors {
            prop_assert!(!e.field.is_empty(), "an error without a field: {e}");
            prop_assert!(!e.message.is_empty());
        }
    }
}

// ----------------------------------------------------------------- the fields

#[test]
fn each_rule_of_the_coder_screen_names_its_field() {
    type Change = Box<dyn Fn(&mut AgentForm)>;
    let table: Vec<(&str, &str, Change)> = vec![
        (
            "a name with a capital",
            "name",
            Box::new(|f| f.name = "Coder".into()),
        ),
        (
            "a name of one character",
            "name",
            Box::new(|f| f.name = "a".into()),
        ),
        (
            "a name of 41 characters",
            "name",
            Box::new(|f| f.name = "a".repeat(41)),
        ),
        (
            "a name that ends with a dash",
            "name",
            Box::new(|f| f.name = "coder-".into()),
        ),
        ("no title", "title", Box::new(|f| f.title = "  ".into())),
        (
            "a title of 81 characters",
            "title",
            Box::new(|f| f.title = "t".repeat(81)),
        ),
        (
            "a description of 501",
            "description",
            Box::new(|f| f.description = Some("d".repeat(501))),
        ),
        (
            "17 tags",
            "tags",
            Box::new(|f| f.tags = (0..17).map(|i| format!("t{i}")).collect()),
        ),
        (
            "a tag with a capital",
            "tags[1]",
            Box::new(|f| f.tags = vec!["ok".into(), "Bad".into()]),
        ),
        (
            "a tag twice",
            "tags[1]",
            Box::new(|f| f.tags = vec!["ok".into(), "ok".into()]),
        ),
        (
            "no endpoint",
            "model.endpoint",
            Box::new(|f| f.model.endpoint = String::new()),
        ),
        (
            "an endpoint that is not a name",
            "model.endpoint",
            Box::new(|f| f.model.endpoint = "Bad Name".into()),
        ),
        (
            "an endpoint that is not there",
            "model.endpoint",
            Box::new(|f| f.model.endpoint = "nothere".into()),
        ),
        (
            "no alias",
            "model.model",
            Box::new(|f| f.model.model = String::new()),
        ),
        (
            "a tool server that is not there",
            "toolServers[zzz]",
            Box::new(|f| {
                f.tool_servers.insert("zzz".into());
            }),
        ),
        (
            "a tool server that is not a name",
            "toolServers[Bad Name]",
            Box::new(|f| {
                f.tool_servers = ["Bad Name".to_owned()].into();
            }),
        ),
        (
            "a plain http server without the box",
            "allowInsecureHttp",
            Box::new(|f| f.allow_insecure_http = false),
        ),
        (
            "33 audience values",
            "audience",
            Box::new(|f| f.audience = Some((0..33).map(|i| format!("a{i}")).collect())),
        ),
        (
            "an audience value with a space",
            "audience[1]",
            Box::new(|f| f.audience = Some(vec!["ok".into(), "a b".into()])),
        ),
        (
            "an audience value of 65 characters",
            "audience[0]",
            Box::new(|f| f.audience = Some(vec!["a".repeat(65)])),
        ),
        (
            "a star beside a value",
            "audience",
            Box::new(|f| f.audience = Some(vec!["*".into(), "x".into()])),
        ),
        (
            "an image with a space",
            "image",
            Box::new(|f| f.image = Some("a b".into())),
        ),
        (
            "an empty image",
            "image",
            Box::new(|f| f.image = Some(String::new())),
        ),
        (
            "no App id",
            "agent.github.appId",
            Box::new(|f| coder_settings(f).github.app_id = " ".into()),
        ),
        (
            "no owners",
            "agent.github.owners",
            Box::new(|f| coder_settings(f).github.owners.clear()),
        ),
        (
            "a star as owner",
            "agent.github.owners[0]",
            Box::new(|f| coder_settings(f).github.owners = vec!["*".into()]),
        ),
        (
            "an owner that is not a login",
            "agent.github.owners[1]",
            Box::new(|f| coder_settings(f).github.owners = vec!["ok".into(), "bad owner".into()]),
        ),
        (
            "an owner twice",
            "agent.github.owners[1]",
            Box::new(|f| coder_settings(f).github.owners = vec!["ok".into(), "ok".into()]),
        ),
        (
            "a repository owner that is not an owner",
            "agent.createRepoOwners[0]",
            Box::new(|f| coder_settings(f).create_repo_owners = vec!["stranger".into()]),
        ),
        (
            "0 runs at once",
            "agent.workers",
            Box::new(|f| coder_settings(f).workers = Some(0)),
        ),
        (
            "17 runs at once",
            "agent.workers",
            Box::new(|f| coder_settings(f).workers = Some(17)),
        ),
        (
            "an empty OpenCode model",
            "agent.opencodeModel",
            Box::new(|f| coder_settings(f).opencode_model = Some(" ".into())),
        ),
        (
            "a key with no Secret",
            "agent.github.privateKey",
            Box::new(|f| coder_settings(f).github.private_key = SecretPick::new("", "k")),
        ),
        (
            "a key that is not offered",
            "agent.github.privateKey",
            Box::new(|f| {
                coder_settings(f).github.private_key = SecretPick::new("other-secret", "k")
            }),
        ),
    ];
    for (what, field, change) in table {
        let mut form = coder_form("coder-me");
        change(&mut form);
        let got = fields(check(&form));
        assert!(
            got.contains(&field.to_owned()),
            "{what}: expected {field} among {got:?}"
        );
    }
}

#[test]
fn each_rule_of_the_folder_screen_names_its_field() {
    type Change = Box<dyn Fn(&mut AgentForm)>;
    let table: Vec<(&str, &str, Change)> = vec![
        (
            "no instructions",
            "agent.files",
            Box::new(|f| {
                folder_files(f).remove("instructions.md");
            }),
        ),
        (
            "an mcp.json",
            "agent.files[mcp.json]",
            Box::new(|f| {
                folder_files(f).insert("mcp.json".into(), "{}".into());
            }),
        ),
        (
            "a nested mcp.json",
            "agent.files[agents/x/mcp.json]",
            Box::new(|f| {
                folder_files(f).insert("agents/x/mcp.json".into(), "{}".into());
            }),
        ),
        (
            "an absolute path",
            "agent.files[/etc/passwd]",
            Box::new(|f| {
                folder_files(f).insert("/etc/passwd".into(), "x".into());
            }),
        ),
        (
            "a path with ..",
            "agent.files[../x.md]",
            Box::new(|f| {
                folder_files(f).insert("../x.md".into(), "x".into());
            }),
        ),
        (
            "a folder over 1 MiB",
            "agent.files",
            Box::new(|f| {
                folder_files(f).insert("big.md".into(), "x".repeat(1 << 20));
            }),
        ),
    ];
    for (what, field, change) in table {
        let mut form = folder_form("chat");
        change(&mut form);
        let got = fields(check(&form));
        assert!(
            got.contains(&field.to_owned()),
            "{what}: expected {field} among {got:?}"
        );
    }
}

#[test]
fn every_error_is_reported_at_once_and_a_field_is_reported_once() {
    let mut form = coder_form("Coder");
    form.title.clear();
    form.model.endpoint = "nothere".into();
    coder_settings(&mut form).workers = Some(99);
    let errors = check(&form).unwrap_err();
    let names: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
    for expected in ["name", "title", "model.endpoint", "agent.workers"] {
        assert!(names.contains(&expected), "{expected} in {names:?}");
    }
    let mut seen = std::collections::BTreeSet::new();
    for n in &names {
        assert!(seen.insert(*n), "{n} twice in {names:?}");
    }
    // The name rule of the screen, not the operator's weaker one (a DNS label of 52): one error, ours.
    let name = errors.iter().find(|e| e.field == "name").unwrap();
    assert!(name.message.contains("2 to 40"), "{name}");
}

#[test]
fn an_error_of_a_referent_is_on_the_field_that_names_it_and_says_which_object() {
    // The ModelEndpoint is there and its address is not valid: the person edits it on the Models screen,
    // and the agent form says which object to go to.
    let mut bad = model_form("gateway");
    bad.base_url = BaseUrlForm::Value("ftp://x.invalid/v1".into());
    let externals = aap_forms::externals(
        &[stored(
            Kind::ModelEndpoint,
            "gateway",
            bad.draft().spec,
            true,
        )],
        &[stored(
            Kind::ToolProvider,
            "websearch",
            tool_form("websearch").draft().spec,
            true,
        )],
        None,
    );
    let form = coder_form("coder-me");
    let errors = validate_agent(
        &form,
        &deployment(AgentKind::Coder, "coder-me"),
        NS,
        &externals,
        &offered(),
        &ImagePolicy::any_registry(),
    )
    .unwrap_err();
    let e = errors.iter().find(|e| e.field == "model.endpoint").unwrap();
    assert!(e.message.contains("ModelEndpoint gateway"), "{e}");
}

#[test]
fn an_error_in_the_deployments_part_is_on_a_deployment_field() {
    let form = coder_form("coder-me");
    let mut d = deployment(AgentKind::Coder, "coder-me");
    d.store.postgres.cnpg.as_mut().unwrap().instances = 0;
    d.a2a_bearer = None;
    d.coder.git_author = Some(aap_api::GitAuthor {
        name: String::new(),
        email: String::new(),
    });
    d.github_mcp = Some(aap_api::GithubMcp {
        sidecar: true,
        port: 0,
        host: None,
    });
    let got = fields(validate_agent(
        &form,
        &d,
        NS,
        &externals(),
        &offered(),
        &ImagePolicy::any_registry(),
    ));
    for expected in [
        "deployment.store",
        "deployment.a2aBearer",
        "deployment.coder.gitAuthor",
        "deployment.githubMcp",
    ] {
        assert!(got.contains(&expected.to_owned()), "{expected} in {got:?}");
    }
}

#[test]
fn the_offered_key_is_the_guard() {
    // AD-030: a key that no labelled ExternalSecret offers is refused, on the field that names it, and the message says what to add.
    let form = coder_form("coder-me");
    let nothing = OfferedKeys::from_offered("agents.vymalo.com/offer", &[]);
    let errors = validate_agent(
        &form,
        &deployment(AgentKind::Coder, "coder-me"),
        NS,
        &externals(),
        &nothing,
        &ImagePolicy::any_registry(),
    )
    .unwrap_err();
    let e = errors
        .iter()
        .find(|e| e.field == "agent.github.privateKey")
        .unwrap();
    assert!(e.message.contains("agents.vymalo.com/offer"), "{e}");
    assert!(e.message.contains("github-app"), "{e}");
    // The right Secret and another key: not offered. Another Secret and the right key: not offered either.
    for pick in [
        SecretPick::new("github-app", "other.pem"),
        SecretPick::new("other", "private-key.pem"),
    ] {
        let mut f = coder_form("coder-me");
        coder_settings(&mut f).github.private_key = pick;
        assert!(fields(check(&f)).contains(&"agent.github.privateKey".to_owned()));
    }
    // A folder agent names no secret: nothing is offered, nothing is refused.
    assert!(
        validate_agent(
            &folder_form("chat"),
            &deployment(AgentKind::Folder, "chat"),
            NS,
            &externals(),
            &nothing,
            &ImagePolicy::any_registry()
        )
        .is_ok()
    );
}

#[test]
fn a_form_that_names_a_secret_says_so_for_the_permission() {
    // `platform:secrets.pick` is needed by any write whose body names a secret key (§60a).
    let coder = coder_form("coder-me");
    assert!(coder.names_a_secret());
    assert_eq!(coder.secret_picks()[0].0, "agent.github.privateKey");
    assert!(!folder_form("chat").names_a_secret());
    assert!(
        model_form("m")
            .secret_picks()
            .iter()
            .map(|(f, _)| f.as_str())
            .eq(["apiKey", "baseUrl.secret"])
    );
    let mut by_value = model_form("m");
    by_value.base_url = BaseUrlForm::Value("https://x.invalid/v1".into());
    assert_eq!(by_value.secret_picks().len(), 1);
    assert_eq!(
        tool_form("t").secret_picks()[0].0,
        "headers[Authorization].secret"
    );
    let mut none = tool_form("t");
    none.headers.clear();
    assert!(none.secret_picks().is_empty());
}

#[test]
fn an_edit_cannot_change_the_name_or_the_kind() {
    let coder = coder_form("coder-me");
    assert!(aap_forms::check_replace(&coder, &coder).is_empty());
    let mut renamed = coder.clone();
    renamed.name = "other".into();
    assert_eq!(aap_forms::check_replace(&coder, &renamed)[0].field, "name");
    let mut folder = folder_form("coder-me");
    folder.name = "coder-me".into();
    let errors = aap_forms::check_replace(&coder, &folder);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].field, "agent.kind");
    // Everything else of an edit is free.
    let mut edited = coder.clone();
    edited.title = "Another title".into();
    edited.suspend = true;
    assert!(aap_forms::check_replace(&coder, &edited).is_empty());
}

// --------------------------------------------------------- the mapping of objects

#[test]
fn what_an_api_server_refuses_is_named_by_the_field_of_the_form() {
    let form = coder_form("coder-me");
    let at = |field: &str, message: &str| {
        from_object_errors(
            FormTarget::Agent(&form),
            &[FieldError {
                field: field.into(),
                message: message.into(),
            }],
        )
        .remove(0)
    };
    let cases = [
        ("AgentService spec.access.audience", "audience"),
        ("AgentService spec.access.audience[2]", "audience[2]"),
        ("AgentService spec.registry.title", "title"),
        ("AgentService spec.suspend", "suspend"),
        ("AgentConfig spec.model", "model"),
        (
            "AgentConfig spec.tools.mcpServers[websearch]",
            "toolServers[websearch]",
        ),
        (
            "AgentConfig spec.tools.mcpServers[websearch].providerRef",
            "toolServers[websearch]",
        ),
        (
            "AgentConfig spec.harness.adam.coder.github.app",
            "agent.github",
        ),
        (
            "AgentConfig spec.harness.adam.coder.github.app.owners",
            "agent.github.owners",
        ),
        ("AgentConfig spec.environment.image.ref", "image"),
        ("AgentService spec.store.postgres", "deployment.store"),
        ("AgentConfig spec.security", "deployment.security"),
    ];
    for (object, field) in cases {
        assert_eq!(at(object, "refused").field, field, "{object}");
    }
    // A place the form has no field for keeps its place in the message: no error is without one.
    let e = at("AgentService spec.scaling.workers", "refused");
    assert_eq!(e.field, "");
    assert!(
        e.message.contains("AgentService spec.scaling.workers"),
        "{e}"
    );
    // The shared kinds.
    let m = from_object_errors(
        FormTarget::Model,
        &[FieldError {
            field: "ModelEndpoint spec.baseUrl".into(),
            message: "x".into(),
        }],
    );
    assert_eq!(m[0].field, "baseUrl");
    let t = from_object_errors(
        FormTarget::ToolServer,
        &[FieldError {
            field: "ToolProvider spec.mcp.headers[Authorization].secretRef".into(),
            message: "x".into(),
        }],
    );
    assert_eq!(t[0].field, "headers[Authorization].secret");
}

// ----------------------------------------------------------------------- models

#[test]
fn each_rule_of_the_models_screen_names_its_field() {
    let ok = validate_model(&model_form("gateway"), NS, &offered());
    assert!(ok.is_ok(), "{:?}", ok.err());
    type Change = Box<dyn Fn(&mut ModelEndpointForm)>;
    let table: Vec<(&str, &str, Change)> = vec![
        ("a capital", "name", Box::new(|f| f.name = "Gateway".into())),
        (
            "64 characters",
            "name",
            Box::new(|f| f.name = "a".repeat(64)),
        ),
        ("no title", "title", Box::new(|f| f.title = String::new())),
        (
            "a title of 129",
            "title",
            Box::new(|f| f.title = "t".repeat(129)),
        ),
        (
            "a scheme",
            "baseUrl.value",
            Box::new(|f| f.base_url = BaseUrlForm::Value("ftp://x.invalid/v1".into())),
        ),
        (
            "not a URL",
            "baseUrl.value",
            Box::new(|f| f.base_url = BaseUrlForm::Value("nonsense".into())),
        ),
        (
            "a user",
            "baseUrl.value",
            Box::new(|f| f.base_url = BaseUrlForm::Value("https://u:p@x.invalid/v1".into())),
        ),
        (
            "a query",
            "baseUrl.value",
            Box::new(|f| f.base_url = BaseUrlForm::Value("https://x.invalid/v1?key=1".into())),
        ),
        (
            "a fragment",
            "baseUrl.value",
            Box::new(|f| f.base_url = BaseUrlForm::Value("https://x.invalid/v1#x".into())),
        ),
        (
            "an address key not offered",
            "baseUrl.secret",
            Box::new(|f| f.base_url = BaseUrlForm::Secret(SecretPick::new("s", "k"))),
        ),
        (
            "a blank address key",
            "baseUrl.secret",
            Box::new(|f| f.base_url = BaseUrlForm::Secret(SecretPick::new("", ""))),
        ),
        (
            "an API key not offered",
            "apiKey",
            Box::new(|f| f.api_key = SecretPick::new("s", "k")),
        ),
        (
            "33 aliases",
            "models",
            Box::new(|f| f.models = (0..33).map(|i| format!("m{i}")).collect()),
        ),
        (
            "a blank alias",
            "models[1]",
            Box::new(|f| f.models = vec!["ok".into(), " ".into()]),
        ),
    ];
    for (what, field, change) in table {
        let mut form = model_form("gateway");
        change(&mut form);
        let got = field_names(validate_model(&form, NS, &offered()));
        assert!(
            got.contains(&field.to_owned()),
            "{what}: expected {field} among {got:?}"
        );
    }
}

#[test]
fn an_error_never_quotes_the_address_it_refuses() {
    for url in [
        format!("https://user:{SENTINEL}@x.invalid/v1"),
        format!("https://x.invalid/v1?key={SENTINEL}"),
        format!("ftp://x.invalid/{SENTINEL}"),
    ] {
        let mut form = model_form("gateway");
        form.base_url = BaseUrlForm::Value(url);
        for e in validate_model(&form, NS, &offered()).unwrap_err() {
            assert!(!format!("{e} {e:?}").contains(SENTINEL), "{e}");
        }
        let mut tool = tool_form("websearch");
        tool.url = match &form.base_url {
            BaseUrlForm::Value(v) => v.clone(),
            BaseUrlForm::Secret(_) => unreachable!(),
        };
        for e in validate_tool_server(&tool, NS, &offered()).unwrap_err() {
            assert!(!format!("{e} {e:?}").contains(SENTINEL), "{e}");
        }
    }
}

// ----------------------------------------------------------------- the image

fn with_image(
    policy: &ImagePolicy,
    image: Option<&str>,
) -> Result<aap_forms::Resources, Vec<FormError>> {
    let mut form = coder_form("coder-me");
    form.image = image.map(str::to_owned);
    validate_agent(
        &form,
        &deployment(AgentKind::Coder, "coder-me"),
        NS,
        &externals(),
        &offered(),
        policy,
    )
}

const PINNED: &str = "ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn an_agents_own_image_is_pinned_has_no_credentials_and_comes_from_an_allowed_registry() {
    let any = ImagePolicy::any_registry();
    let only = ImagePolicy::only(["ghcr.io"]);
    // A pinned image from an allowed registry, with a tag, a digest or both.
    for image in [
        PINNED,
        "ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e",
        "ghcr.io/vymalo/coder@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ] {
        assert!(with_image(&only, Some(image)).is_ok(), "{image}");
        assert!(with_image(&any, Some(image)).is_ok(), "{image}");
    }
    // The three references a review found accepted.
    for (image, policy, says) in [
        (
            "user:pw@registry.invalid/a:1",
            &any,
            "no user name or password",
        ),
        ("registry.invalid/a", &any, "tag or a digest"),
        (
            "evil.example/x:1",
            &only,
            "evil.example is not an allowed registry",
        ),
    ] {
        let errors = with_image(policy, Some(image)).unwrap_err();
        assert_eq!(errors.len(), 1, "{image}: {errors:?}");
        assert_eq!(errors[0].field, "image", "{image}");
        assert!(
            errors[0].message.contains(says),
            "{image}: {}",
            errors[0].message
        );
    }
    // No image is the operator's default, whatever the allow-list says.
    for policy in [&any, &only, &ImagePolicy::only(Vec::<String>::new())] {
        assert!(with_image(policy, None).is_ok());
    }
}

#[test]
fn an_empty_allow_list_lets_no_agent_pin_an_image_and_an_absent_one_lets_any_registry() {
    let none = ImagePolicy::only(Vec::<String>::new());
    let errors = with_image(&none, Some(PINNED)).unwrap_err();
    assert_eq!(fields_of(&errors), ["image"]);
    assert!(
        errors[0].message.contains("no agent pin its own image"),
        "{errors:?}"
    );
    assert!(with_image(&ImagePolicy::any_registry(), Some("evil.example/x:1")).is_ok());

    let mut d = defaults();
    assert_eq!(
        d.image_policy(),
        ImagePolicy::any_registry(),
        "absent: any registry"
    );
    d.allowed_image_registries = Some(Vec::new());
    assert_eq!(d.image_policy(), none, "present and empty: none");
    d.allowed_image_registries = Some(vec!["ghcr.io".to_owned()]);
    assert_eq!(d.image_policy(), ImagePolicy::only(["ghcr.io"]));
}

fn fields_of(errors: &[FormError]) -> Vec<&str> {
    errors.iter().map(|e| e.field.as_str()).collect()
}

#[test]
fn an_error_about_an_image_never_quotes_it() {
    for image in [
        format!("user:{SENTINEL}@registry.invalid/a:1"),
        format!("{SENTINEL}@registry.invalid/a:1"),
        format!("registry.invalid/{SENTINEL}"),
        format!("registry.invalid/a:1@{SENTINEL}"),
    ] {
        for policy in [ImagePolicy::any_registry(), ImagePolicy::only(["ghcr.io"])] {
            for e in with_image(&policy, Some(&image)).unwrap_err() {
                assert!(!format!("{e} {e:?}").contains(SENTINEL), "{e}");
            }
        }
    }
}

#[test]
fn the_allowed_registries_are_read_from_the_apis_configuration_and_checked_when_it_starts() {
    let mut d = defaults();
    assert!(d.allowed_image_registries.is_none() && d.check().is_empty());
    d.allowed_image_registries = Some(vec!["ghcr.io".into(), "registry.example:5000".into()]);
    assert!(d.check().is_empty(), "{:?}", d.check());
    d.allowed_image_registries = Some(Vec::new());
    assert!(
        d.check().is_empty(),
        "an empty list is allowed: it is how to forbid images"
    );
    for bad in [
        "https://ghcr.io",
        "ghcr.io/vymalo",
        "",
        "ghcr.io:",
        "a b",
        "ghcr.io@x",
    ] {
        d.allowed_image_registries = Some(vec![bad.to_owned()]);
        let problems = d.check();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("allowedImageRegistries")),
            "{bad:?}: {problems:?}"
        );
    }

    let yaml = "namespace: ns\nstore: { cnpg: { instances: 1, storage: { size: 5Gi } } }\na2aBearer: { name: a, key: K }\n";
    let absent: aap_forms::Defaults = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(absent.allowed_image_registries, None);
    let empty: aap_forms::Defaults =
        serde_yaml::from_str(&format!("{yaml}allowedImageRegistries: []\n")).unwrap();
    assert_eq!(empty.allowed_image_registries, Some(Vec::new()));
    let some: aap_forms::Defaults =
        serde_yaml::from_str(&format!("{yaml}allowedImageRegistries: [ghcr.io]\n")).unwrap();
    assert_eq!(some.image_policy(), ImagePolicy::only(["ghcr.io"]));
}

// ------------------------------------------------------------------ tool servers

#[test]
fn each_rule_of_the_tool_servers_screen_names_its_field() {
    let ok = validate_tool_server(&tool_form("websearch"), NS, &offered());
    assert!(
        ok.is_ok(),
        "a plain http server is the agent's rule, not the server's: {:?}",
        ok.err()
    );
    type Change = Box<dyn Fn(&mut ToolServerForm)>;
    let table: Vec<(&str, &str, Change)> = vec![
        (
            "a capital",
            "name",
            Box::new(|f| f.name = "WebSearch".into()),
        ),
        (
            "32 characters",
            "name",
            Box::new(|f| f.name = "a".repeat(32)),
        ),
        ("no title", "title", Box::new(|f| f.title.clear())),
        ("not a URL", "url", Box::new(|f| f.url = "nonsense".into())),
        (
            "a scheme",
            "url",
            Box::new(|f| f.url = "file:///etc/passwd".into()),
        ),
        (
            "a user",
            "url",
            Box::new(|f| f.url = "https://u:p@x.invalid/mcp".into()),
        ),
        (
            "a query",
            "url",
            Box::new(|f| f.url = "https://x.invalid/mcp?key=1".into()),
        ),
        (
            "a variable in the URL",
            "url",
            Box::new(|f| f.url = "https://x.invalid/${TOKEN}".into()),
        ),
        (
            "a header name that is not a token",
            "headers[bad name]",
            Box::new(|f| {
                let h = f.headers.remove("Authorization").unwrap();
                f.headers.insert("bad name".into(), h);
            }),
        ),
        (
            "a prefix with a variable",
            "headers[Authorization].prefix",
            Box::new(|f| f.headers.get_mut("Authorization").unwrap().prefix = Some("${X}".into())),
        ),
        (
            "a key not offered",
            "headers[Authorization].secret",
            Box::new(|f| {
                f.headers.get_mut("Authorization").unwrap().secret = SecretPick::new("s", "K")
            }),
        ),
        (
            "a tool that cannot be named",
            "tools[1]",
            Box::new(|f| f.tools = vec!["ok".into(), "bad tool!".into()]),
        ),
        (
            "a blank tool",
            "tools[0]",
            Box::new(|f| f.tools = vec![" ".into()]),
        ),
        (
            "33 tools",
            "tools",
            Box::new(|f| f.tools = (0..33).map(|i| format!("t{i}")).collect()),
        ),
        (
            "two headers that want one variable from different Secrets",
            "headers",
            Box::new(|f| {
                f.headers.insert(
                    "X-Other".into(),
                    HeaderForm {
                        prefix: None,
                        secret: SecretPick::new("another-secret", "SEARCH_MCP_TOKEN"),
                    },
                );
            }),
        ),
    ];
    let wide = OfferedKeys::from_offered(
        aap_ports::OFFER_LABEL,
        &[
            offer("agent-tools", "SEARCH_MCP_TOKEN"),
            offer("another-secret", "SEARCH_MCP_TOKEN"),
        ],
    );
    for (what, field, change) in table {
        let mut form = tool_form("websearch");
        change(&mut form);
        let got = field_names(validate_tool_server(&form, NS, &wide));
        assert!(
            got.contains(&field.to_owned()),
            "{what}: expected {field} among {got:?}"
        );
    }
}

// ------------------------------------------------------------------ the defaults

#[test]
fn the_defaults_are_checked_by_the_operators_rules_when_the_api_starts() {
    assert_eq!(defaults().check(), Vec::<String>::new());
    let mut d = defaults();
    d.namespace = "Not A Namespace".into();
    d.store = aap_forms::StoreDefault::SecretRef(aap_forms::SecretPattern {
        name: "shared-db".into(),
        key: "uri".into(),
    });
    d.a2a_bearer.key.clear();
    d.offer_label = "has space".into();
    if let aap_forms::StoreDefault::Cnpg(c) = &mut defaults().store {
        c.instances = 0;
    }
    let problems = d.check();
    let all = problems.join("\n");
    for expected in ["namespace", "{name}", "offerLabel", "a2aBearer"] {
        assert!(all.contains(expected), "{expected} in\n{all}");
    }
    let mut zero = defaults();
    zero.store = aap_forms::StoreDefault::Cnpg(aap_api::Cnpg {
        instances: 0,
        storage: aap_api::CnpgStorage {
            size: k8s_openapi::apimachinery::pkg::api::resource::Quantity("5Gi".into()),
            storage_class: None,
        },
    });
    assert!(
        zero.check().iter().any(|p| p.contains("deployment.store")),
        "{:?}",
        zero.check()
    );
}

#[test]
fn the_defaults_are_read_from_the_apis_configuration_and_a_typo_is_refused() {
    let yaml = r"
namespace: another-agentic-system
store:
  cnpg:
    instances: 1
    storage: { size: 5Gi }
a2aBearer: { name: agent-a2a, key: AGENT_REGISTRY_AGENT_TOKEN }
allowFrom:
  - namespaceSelector:
      matchLabels: { kubernetes.io/metadata.name: another-agentic-system }
coder:
  gitAuthor: { name: bot, email: bot@x.invalid }
  allowedRepoHosts: [github.com]
  githubApiUrl: https://api.github.com
  githubMcp: { sidecar: true, port: 8082 }
  workVolume:
    name: work
    scope: agent
    mountPath: /work
    source: { persistent: { size: 20Gi, perReplica: true } }
folder:
  resources: { requests: { cpu: 100m, memory: 256Mi } }
";
    let d: aap_forms::Defaults = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(
        d.offer_label,
        aap_ports::OFFER_LABEL,
        "the offer label has a default"
    );
    assert!(d.check().is_empty(), "{:?}", d.check());
    for typo in [
        format!("{yaml}\nallowfrom: []\n"),
        yaml.replace("a2aBearer", "a2aBearer2"),
        yaml.replace("githubApiUrl", "githubApiURL"),
        yaml.replace("workVolume", "workVolumes"),
    ] {
        assert!(
            serde_yaml::from_str::<aap_forms::Defaults>(&typo).is_err(),
            "{typo}"
        );
    }
    let by_pattern: aap_forms::Defaults = serde_yaml::from_str(&yaml.replace(
        "store:\n  cnpg:\n    instances: 1\n    storage: { size: 5Gi }",
        "store:\n  secretRef: { name: '{name}-db-app', key: uri }",
    ))
    .unwrap();
    let dep = by_pattern.deployment_for(AgentKind::Folder, "researcher");
    assert_eq!(
        dep.store.postgres.secret_ref.as_ref().unwrap().name,
        "researcher-db-app"
    );
    assert!(
        dep.volumes.is_empty() && dep.github_mcp.is_none() && dep.coder.git_author.is_none(),
        "a folder agent gets no coder part"
    );
}
