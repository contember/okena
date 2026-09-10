//! Round trips against the real `openspec` CLI.
//!
//! Opt-in: set `OPENSPEC_CLI` to an `openspec` binary (e.g. from
//! `npm i @fission-ai/openspec`) to run. Without it every test returns early,
//! so CI without Node stays green. Each test sandboxes the CLI's data and
//! config directories through `XDG_DATA_HOME` / `XDG_CONFIG_HOME`, so neither
//! side ever touches the developer's real registry.

use okena_openspec::discover::{self, ProjectSource, Sources};
use okena_openspec::{OpenSpecDirs, registry, setup};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Cli {
    bin: PathBuf,
    sandbox: tempfile::TempDir,
}

impl Cli {
    fn new() -> Option<Self> {
        let bin = PathBuf::from(std::env::var_os("OPENSPEC_CLI")?);
        Some(Self {
            bin,
            sandbox: tempfile::tempdir().expect("tempdir"),
        })
    }

    fn dirs(&self) -> OpenSpecDirs {
        OpenSpecDirs {
            data_dir: self.sandbox.path().join("data/openspec"),
            config_dir: self.sandbox.path().join("config/openspec"),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.sandbox.path().join(rel)
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> (bool, String) {
        let out = Command::new(&self.bin)
            .args(args)
            .current_dir(cwd)
            .env("XDG_DATA_HOME", self.sandbox.path().join("data"))
            .env("XDG_CONFIG_HOME", self.sandbox.path().join("config"))
            .env("OPENSPEC_TELEMETRY", "0")
            .env("DO_NOT_TRACK", "1")
            .output()
            .expect("run openspec");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }

    fn json(&self, cwd: &Path, args: &[&str]) -> serde_json::Value {
        let (_, text) = self.run(cwd, args);
        let start = text
            .find('{')
            .unwrap_or_else(|| panic!("no JSON from {args:?}: {text}"));
        serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<serde_json::Value>()
            .next()
            .expect("a JSON value")
            .unwrap_or_else(|e| panic!("bad JSON from {args:?}: {e}\n{text}"))
    }
}

fn canonical(p: &Path) -> String {
    p.canonicalize().unwrap().to_string_lossy().into_owned()
}

#[test]
fn a_store_okena_sets_up_is_listed_healthy_and_usable_by_the_cli() {
    let Some(cli) = Cli::new() else { return };
    let dirs = cli.dirs();
    let root = cli.path("stores/team-plans");
    setup::setup_store(
        &dirs,
        &setup::SetupRequest {
            id: "team-plans".into(),
            path: root.to_string_lossy().into_owned(),
            remote: Some("git@github.com:acme/team-plans.git".into()),
            init_git: false,
        },
    )
    .expect("okena setup");
    setup::set_default_store(&dirs, Some("team-plans")).expect("default store");

    let list = cli.json(cli.sandbox.path(), &["store", "list", "--json"]);
    let stores = list["stores"].as_array().expect("stores");
    assert_eq!(stores.len(), 1, "{list}");
    assert_eq!(stores[0]["id"], "team-plans");

    let doctor = cli.json(
        cli.sandbox.path(),
        &["store", "doctor", "team-plans", "--json"],
    );
    assert!(
        doctor.to_string().contains("\"healthy\":true")
            || doctor["stores"][0]["diagnostics"]
                .as_array()
                .is_some_and(|d| d.is_empty()),
        "CLI doctor is unhappy with okena's store: {doctor}"
    );

    // The CLI can work in it through the default okena set: run from a folder
    // with no planning of its own, `new change` must land in the store.
    let elsewhere = cli.path("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let (ok, text) = cli.run(&elsewhere, &["new", "change", "from-cli"]);
    assert!(ok, "{text}");
    assert!(
        root.join("openspec/changes/from-cli/.openspec.yaml")
            .is_file(),
        "{text}"
    );
}

#[test]
fn a_store_the_cli_sets_up_is_discovered_by_okena_and_can_be_unregistered() {
    let Some(cli) = Cli::new() else { return };
    let root = cli.path("stores/design-system");
    let (ok, text) = cli.run(
        cli.sandbox.path(),
        &[
            "store",
            "setup",
            "design-system",
            "--path",
            &root.to_string_lossy(),
            "--no-init-git",
            "--json",
        ],
    );
    assert!(ok, "{text}");

    let dirs = cli.dirs();
    let web = cli.path("repos/web");
    std::fs::create_dir_all(web.join("openspec")).unwrap();
    std::fs::write(
        web.join("openspec/config.yaml"),
        "schema: spec-driven\nstore: design-system\n",
    )
    .unwrap();

    let found = discover::discover(
        &dirs,
        &Sources {
            registry: true,
            projects: vec![ProjectSource {
                name: "web".into(),
                path: web.to_string_lossy().into_owned(),
            }],
            folders: Vec::new(),
        },
    );
    assert!(found.status.is_empty(), "{:?}", found.status);
    let store = found.root("store:design-system").expect("store discovered");
    assert!(store.healthy, "{:?}", store.status);
    assert_eq!(store.path, canonical(&root));
    assert_eq!(store.used_by, ["web"]);
    assert_eq!(
        found.pointers[0].root_key.as_deref(),
        Some("store:design-system")
    );

    // Unregistering through okena is visible to the CLI.
    registry::unregister(&dirs, "design-system").expect("unregister");
    let list = cli.json(cli.sandbox.path(), &["store", "list", "--json"]);
    assert_eq!(list["stores"].as_array().map(Vec::len), Some(0), "{list}");
    assert!(root.join("openspec").is_dir(), "files stay on disk");
}

#[test]
fn a_checkout_okena_registers_resolves_as_a_declared_store_for_the_cli() {
    let Some(cli) = Cli::new() else { return };
    // A teammate's clone: a healthy root with committed identity.
    let clone = cli.path("clones/team-plans");
    std::fs::create_dir_all(clone.join("openspec/specs")).unwrap();
    std::fs::create_dir_all(clone.join("openspec/changes/archive")).unwrap();
    std::fs::write(clone.join("openspec/config.yaml"), "schema: spec-driven\n").unwrap();
    std::fs::create_dir_all(clone.join(".openspec-store")).unwrap();
    std::fs::write(
        clone.join(".openspec-store/store.yaml"),
        "version: 1\nid: team-plans\n",
    )
    .unwrap();

    registry::register(&cli.dirs(), &clone.to_string_lossy(), None).expect("okena register");

    let web = cli.path("repos/web");
    std::fs::create_dir_all(web.join("openspec")).unwrap();
    std::fs::write(
        web.join("openspec/config.yaml"),
        "schema: spec-driven\nstore: team-plans\n",
    )
    .unwrap();
    let context = cli.json(&web, &["context", "--json"]);
    assert_eq!(context["root"]["source"], "declared", "{context}");
    assert_eq!(context["root"]["store_id"], "team-plans", "{context}");
    assert_eq!(context["root"]["path"], canonical(&clone), "{context}");
}
