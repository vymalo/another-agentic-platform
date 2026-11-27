//! `aapctl` (§60b). Phase 1: `validate` and `render`, offline. Phase 2 (K9b-1): `get`, `apply`, `delete`, `export`, `whoami`,
//! `login` (a browser sign-in, or `--token-stdin`) and `logout`, over the Platform API ([`remote`], [`signin`]).
//!
//! Both read `AgentService`, `AgentConfig`, `ModelEndpoint` and `ToolProvider` documents and judge them as the operator does,
//! with `aap_domain::resolve` and the deployment's settings given as flags (the operator's own names and defaults). `render`
//! then prints the objects `aap_runtime_kubernetes::render` makes of each resolved service, the function the provider applies
//! and its golden files are made with. Nothing here reaches a cluster or a network, and no Secret is read: an object names a
//! Secret by reference, and so does what is printed.
//!
//! [`run`] is the whole program with its streams given, so tests can drive it; `main` only maps its answer to an exit code
//! (0 everything is fine, 1 an issue was found, 2 a usage or read error; the network commands add 3 stale or conflict, 4 not
//! allowed, 5 unreachable: [`failure`]).
//!
//! **`validate` and `render` read no environment and open no connection.** Only the network commands read `AAPCTL_TOKEN`,
//! `XDG_CONFIG_HOME` and `HOME` ([`config::Env`]), and only for the server and the token.

mod check;
mod config;
mod failure;
mod forms;
mod load;
mod peer;
mod remote;
mod signin;
mod skew;

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;

use aap_api::AccessTierCap;
use aap_domain::RunPodSettings;
use clap::{Args, Parser, Subcommand};

use crate::check::{Outcome, Settings, check};

/// The uid the owner reference of a rendered object names when `--owner-uid` is not given: the objects are not applied, so
/// there is no service to own them, and this one is as plain as a uid can be.
pub const PLACEHOLDER_OWNER_UID: &str = "00000000-0000-0000-0000-000000000000";

/// The platform's command line.
#[derive(Debug, Parser)]
#[command(
    name = "aapctl",
    version,
    about,
    color = clap::ColorChoice::Never,
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check AgentService, AgentConfig, ModelEndpoint and ToolProvider documents as the operator would, offline. Prints one
    /// line per issue on stderr (file, object, field); exit 0 when there are none, 1 when there are, 2 for a usage or read
    /// error.
    Validate(ValidateArgs),
    /// Print the objects the operator would apply for each AgentService, as a YAML stream, each service after a comment that
    /// holds its digest. Prints nothing when anything is invalid.
    Render(RenderArgs),
    /// Print the version.
    Version,
    /// List or read agents, models or tool servers through the Platform API. `-o json|yaml` prints the API's body, `-o etag` the
    /// tag of one object. Exit 0, 4 not allowed, 5 unreachable.
    Get(remote::GetArgs),
    /// Create or replace what the files hold through the Platform API: models, then tool servers, then agents. Reads each object
    /// first, writes nothing when it is unchanged, refuses an object no form says, a deployment part that differs, a label or an
    /// annotation the API would not write and a namespace that is not the API's, and retries only when the operator's status write
    /// moved the tag. The first refusal stops it. Exit 0, 1 refused, 2 the files, 3 stale or conflict, 4 not allowed, 5 the server.
    Apply(remote::ApplyArgs),
    /// Delete an agent, a model or a tool server through the Platform API (`--wait` until an agent is gone).
    Delete(remote::DeleteArgs),
    /// Print an agent's two objects as YAML, as the API serves them: what `apply` reads back unchanged.
    Export(remote::ExportArgs),
    /// Say which server and token are in use, what the token may do, and when it expires.
    Whoami(remote::Conn),
    /// Sign in with a browser (OAuth 2.0 authorization code with PKCE on a loopback address), or keep a token piped on standard
    /// input (`--token-stdin`). The access token is renewed with the refresh token when it is about to expire. Exit 4 when the
    /// sign-in did not happen.
    Login(remote::LoginArgs),
    /// Forget the kept tokens (and revoke the refresh token at the issuer, once, if it says where), and keep the server.
    Logout(remote::LogoutArgs),
}

/// The settings the operator takes from its configuration file (`defaults.*` and `runPods`), as aapctl's own flags, with the
/// operator's defaults. aapctl does not read the operator's file, and the environment is not read: the same files give the same answer on every machine.
#[derive(Debug, Args)]
struct Deployment {
    /// The coder image of an agent that names none (aapctl's own flag, as the operator's file key `defaults.agentImage`). None: such an agent is invalid.
    #[arg(long, value_name = "IMAGE")]
    default_agent_image: Option<String>,
    /// The settings of run pods (aapctl's own flag: a YAML file holding a `runPods` block, as the operator's file has it). None: an agent that asks for run pods is
    /// invalid.
    #[arg(long, value_name = "FILE")]
    run_pods_file: Option<PathBuf>,
    /// The highest access tier an agent may ask for (as the operator's `defaults.maxAccessTier`): plan, edits, auto or bypass.
    #[arg(long, value_name = "TIER", default_value = "auto", value_parser = parse_tier)]
    max_access_tier: AccessTierCap,
    /// The claims an agent may use as its package cache (as the operator's `defaults.cacheAllowedClaims`), comma separated. None: no
    /// agent may have a cache.
    #[arg(long, value_name = "CLAIMS", value_delimiter = ',')]
    cache_allowed_claims: Vec<String>,
}

/// Where the documents come from.
#[derive(Debug, Args)]
pub(crate) struct Input {
    /// A file, a directory (its `*.yaml` and `*.yml` files) or `-` for standard input. Repeatable, and takes several values.
    #[arg(
        short = 'f',
        long = "filename",
        required = true,
        num_args = 1..,
        action = clap::ArgAction::Append,
        value_name = "FILE|DIR|-"
    )]
    pub(crate) files: Vec<String>,
}

#[derive(Debug, Args)]
struct ValidateArgs {
    #[command(flatten)]
    input: Input,
    #[command(flatten)]
    deployment: Deployment,
    /// A referent that is not among the documents (a config, a model endpoint, a tool provider) is a warning on stderr and not
    /// an issue.
    #[arg(long)]
    allow_missing_refs: bool,
}

#[derive(Debug, Args)]
struct RenderArgs {
    #[command(flatten)]
    input: Input,
    #[command(flatten)]
    deployment: Deployment,
    /// The uid the owner reference of each object names. The objects are not applied, so there is no service to own them.
    #[arg(long, value_name = "UID", default_value = PLACEHOLDER_OWNER_UID)]
    owner_uid: String,
}

fn parse_tier(s: &str) -> Result<AccessTierCap, String> {
    match s.trim() {
        "plan" => Ok(AccessTierCap::Plan),
        "edits" => Ok(AccessTierCap::Edits),
        "" | "auto" => Ok(AccessTierCap::Auto),
        "bypass" => Ok(AccessTierCap::Bypass),
        other => Err(format!(
            "{other:?} is not an access tier: plan, edits, auto or bypass"
        )),
    }
}

/// The deployment's settings, as the operator reads them at start (and refuses, as it does, what it would not start with).
fn settings(d: &Deployment, owner_uid: &str) -> Result<Settings, String> {
    let default_image = d
        .default_agent_image
        .as_deref()
        .map(str::trim)
        .filter(|i| !i.is_empty());
    if let Some(image) = default_image
        && image.contains(char::is_whitespace)
    {
        return Err(format!(
            "--default-agent-image {image:?} has white space in it: an image reference such as repository:tag@sha256:… is expected"
        ));
    }
    let owner_uid = owner_uid.trim();
    if owner_uid.is_empty() {
        return Err(
            "--owner-uid is empty: an object that names no owner is one the operator never applies"
                .to_owned(),
        );
    }
    // A blank path is none, as the operator reads an empty path (was `RUN_PODS_FILE`).
    let run_pods_file = d
        .run_pods_file
        .as_deref()
        .map(|p| PathBuf::from(p.to_string_lossy().trim()))
        .filter(|p| !p.as_os_str().is_empty());
    let run_pods = match &run_pods_file {
        None => None,
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("--run-pods-file {}: cannot be read: {e}", path.display()))?;
            let parsed: RunPodSettings = serde_yaml::from_str(&text).map_err(|e| {
                format!(
                    "--run-pods-file {}: not the settings of run pods: {e}",
                    path.display()
                )
            })?;
            if let Err(problems) = parsed.validate() {
                return Err(format!(
                    "--run-pods-file {}:\n  {}",
                    path.display(),
                    problems.join("\n  ")
                ));
            }
            Some(parsed)
        }
    };
    Ok(Settings {
        default_image: default_image.map(str::to_owned),
        run_pods,
        cache_claims: d
            .cache_allowed_claims
            .iter()
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty())
            .collect(),
        max_access_tier: d.max_access_tier,
        owner_uid: owner_uid.to_owned(),
    })
}

/// Run the program: `args` (with the program name first), the streams, and the exit code to give.
pub fn run<I, T>(args: I, stdin: &mut dyn Read, out: &mut dyn Write, err: &mut dyn Write) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let text = e.to_string();
            if e.use_stderr() {
                let _ = err.write_all(text.as_bytes());
                return 2;
            }
            // `--help` and `--version`
            let _ = out.write_all(text.as_bytes());
            return 0;
        }
    };
    let fail = |err: &mut dyn Write, message: String| {
        let _ = writeln!(err, "aapctl: {message}");
        2
    };
    match cli.command {
        Command::Version => {
            let _ = writeln!(out, "aapctl {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Get(a) => remote::get(&a, out, err),
        Command::Apply(a) => remote::apply(&a, stdin, out, err),
        Command::Delete(a) => remote::delete(&a, out, err),
        Command::Export(a) => remote::export(&a, out, err),
        Command::Whoami(c) => remote::whoami(&c, out, err),
        Command::Login(a) => remote::login(&a, stdin, out, err),
        Command::Logout(a) => remote::logout(&a, out, err),
        Command::Validate(a) => {
            let (loaded, settings) =
                match read(&a.input, &a.deployment, PLACEHOLDER_OWNER_UID, stdin) {
                    Ok(read) => read,
                    Err(message) => return fail(err, message),
                };
            notes(&loaded, err);
            let outcome = check(&loaded.objects, &settings, a.allow_missing_refs);
            report(&outcome, err)
        }
        Command::Render(a) => {
            let (loaded, settings) = match read(&a.input, &a.deployment, &a.owner_uid, stdin) {
                Ok(read) => read,
                Err(message) => return fail(err, message),
            };
            notes(&loaded, err);
            let mut outcome = check(&loaded.objects, &settings, false);
            let rendered = render_all(&mut outcome);
            let code = report(&outcome, err);
            if code == 0 {
                let _ = out.write_all(rendered.as_bytes());
            }
            code
        }
    }
}

fn read(
    input: &Input,
    deployment: &Deployment,
    owner_uid: &str,
    stdin: &mut dyn Read,
) -> Result<(load::Loaded, Settings), String> {
    let settings = settings(deployment, owner_uid)?;
    let loaded = load::load(&input.files, stdin)?;
    Ok((loaded, settings))
}

/// What was skipped, one line each.
fn notes(loaded: &load::Loaded, err: &mut dyn Write) {
    for note in &loaded.notes {
        let _ = writeln!(err, "{note}");
    }
}

/// The issues and warnings on `err`; 1 when there is an issue.
fn report(outcome: &Outcome, err: &mut dyn Write) -> u8 {
    for warning in &outcome.warnings {
        let _ = writeln!(err, "warning: {warning}");
    }
    for issue in &outcome.issues {
        let _ = writeln!(err, "{issue}");
    }
    u8::from(!outcome.issues.is_empty())
}

/// What the provider would apply for every resolved service, as one YAML stream. A service the provider refuses to render is an
/// issue of the outcome.
fn render_all(outcome: &mut Outcome) -> String {
    let mut text = String::new();
    let mut refused = Vec::new();
    for (i, (object, agent)) in outcome.resolved.iter().enumerate() {
        let rendered = match aap_runtime_kubernetes::render(&agent.id, &agent.runtime) {
            Ok(r) => r,
            Err(e) => {
                refused.push(check::Line {
                    file: object.file.clone(),
                    object: object.label.clone(),
                    text: format!("cannot be rendered: {e}"),
                });
                continue;
            }
        };
        if i > 0 {
            text.push_str("---\n");
        }
        let at = match &object.namespace {
            Some(ns) => format!("{ns}/{}", object.name),
            None => object.name.clone(),
        };
        text.push_str(&format!("# AgentService {at}: {}\n", agent.digest));
        let docs: Vec<String> = rendered
            .documents()
            .iter()
            .map(|d| serde_yaml::to_string(d).unwrap_or_default())
            .collect();
        text.push_str(&docs.join("---\n"));
    }
    outcome.issues.extend(refused);
    text
}
