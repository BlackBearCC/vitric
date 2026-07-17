//! TypeScript script end-to-end: .ts is transpiled into QuickJS via esbuild, type errors are reported explicitly.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

/// Test uses esbuild: the repo has a copy in mcp/node_modules; CI installs it onto PATH via the workflow.
fn esbuild_env() -> Option<String> {
    if std::env::var("ESBUILD_BIN").is_ok() {
        return Some(std::env::var("ESBUILD_BIN").unwrap());
    }
    let local = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../mcp/node_modules/.bin/esbuild");
    if local.exists() {
        let p = local.canonicalize().unwrap().to_string_lossy().into_owned();
        std::env::set_var("ESBUILD_BIN", &p);
        return Some(p);
    }
    if which_esbuild() {
        return Some("esbuild".into());
    }
    None
}

fn which_esbuild() -> bool {
    std::process::Command::new("esbuild")
        .arg("--version")
        .output()
        .is_ok()
}

fn make_project(tag: &str, script: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-ts-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "scripts"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        r#"{"name":"ts-demo","schema":"schema.json","entry":"scenes/main.json",
            "scenes":["scenes/main.json"],"scripts":["scripts/logic.ts"],"seed":1}"#,
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        r#"{"components":{"Counter":{"fields":{"value":{"type":"int","default":0}}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        r#"{"entities":[{"name":"c","components":{"Counter":{}}}]}"#,
    )
    .unwrap();
    fs::write(dir.join("scripts/logic.ts"), script).unwrap();
    dir
}

#[test]
fn typescript_system_runs_after_transpile() {
    let Some(_) = esbuild_env() else {
        panic!("测试需要 esbuild：仓库里跑 `cd mcp && npm install`，或设 ESBUILD_BIN");
    };
    // Real TS: interfaces, type annotations, generics — QuickJS cannot consume these natively
    let dir = make_project(
        "ok",
        r#"
        interface CounterRow { id: string; Counter: { value: number } }
        const step: number = 2;
        function bump<T extends CounterRow>(rows: T[]): void {
            for (const r of rows) r.Counter.value += step;
        }
        vitric.system("bump", { query: ["Counter"], writes: ["Counter"] }, (entities: CounterRow[]) => {
            bump(entities);
        });
        "#,
    );
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    for _ in 0..3 {
        sim.step(&mut rt).unwrap();
    }
    let c = sim.world.entity("c").unwrap();
    assert_eq!(sim.world.get_field(c, "Counter.value").unwrap(), &json!(6));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn typescript_syntax_error_names_the_file() {
    let Some(_) = esbuild_env() else {
        panic!("测试需要 esbuild：仓库里跑 `cd mcp && npm install`，或设 ESBUILD_BIN");
    };
    let dir = make_project("bad", "const x: = 不是合法TS;");
    let err = match Runtime::boot(&dir) {
        Err(e) => e,
        Ok(_) => panic!("坏 TS 不该 boot 成功"),
    };
    assert!(err.contains("logic.ts") && err.contains("转译失败"), "{err}");
    fs::remove_dir_all(&dir).unwrap();
}
