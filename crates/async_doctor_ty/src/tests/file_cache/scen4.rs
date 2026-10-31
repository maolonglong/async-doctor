//! Multi-root / editable / shadow scenarios (scen4.sh).

use std::fs;
use std::path::Path;

use super::super::helpers::{
    clone_fixture, fresh_cache_dir, report_json_with_cache, temp_project, with_cleared_virtual_env,
};
use super::helpers::incremental_vs_cold;

fn mk_bases(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
}

fn mk_factory(dir: &Path, pkg: &str, ret: &str) {
    fs::write(
        dir.join("factory.py"),
        format!("from {pkg}.bases import Safe, Blocking\ndef make(x=None) -> {ret}:\n    return x\n"),
    )
    .unwrap();
}

#[test]
fn scen_shadow() {
    // Root-level pkg/m.py must take precedence over src/pkg/m.py.
    let dir = temp_project(&[
        ("pyproject.toml", "[project]\nname=\"x\"\n"),
        ("src/pkg/__init__.py", ""),
        ("pkg/__init__.py", ""),
        ("src/pkg/m.py", "def f():\n    pass\n"),
        (
            "app.py",
            "from pkg.m import f\nasync def run():\n    f()\n",
        ),
    ]);
    let cache = fresh_cache_dir("scen-shadow");
    let before = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/m.py"),
        "import time\ndef f():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "shadow");
    let after = report_json_with_cache(&dir, &fresh_cache_dir("scen-shadow-cold"));
    // Cold after edit must report the new sleep (finding count rises).
    let nb: usize = serde_json::from_str::<serde_json::Value>(&before)
        .unwrap()["findings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let na: usize = serde_json::from_str::<serde_json::Value>(&after)
        .unwrap()["findings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        na > nb,
        "shadowing root pkg/m.py must add findings (before={nb} after={na})"
    );
}

#[test]
fn scen_src_layout_ret() {
    let dir = temp_project(&[
        ("pyproject.toml", "[project]\nname=\"x\"\n"),
        ("src/pkg/__init__.py", ""),
        (
            "src/pkg/svc.py",
            "from pkg.factory import make\nasync def run():\n    make().go()\n",
        ),
    ]);
    mk_bases(&dir.join("src/pkg"));
    mk_factory(&dir.join("src/pkg"), "pkg", "Safe");
    let cache = fresh_cache_dir("scen-srcret4");
    let _ = report_json_with_cache(&dir, &cache);
    mk_factory(&dir.join("src/pkg"), "pkg", "Blocking");
    incremental_vs_cold(&dir, &cache, "srcret4");
}

#[test]
fn scen_editable_annotation() {
    // Setuptools editable finder maps libcore → libcore/libcore.
    let nested = clone_fixture("m3c_nested_pkg_editable");
    let dir = temp_project(&[("main.py", "from libcore.factory import make\nasync def run():\n    make().go()\n")]);
    // Copy venv with editable finder into the temp project.
    let src_venv = nested.join(".venv");
    let dst_venv = dir.join(".venv");
    copy_dir(&src_venv, &dst_venv);
    fs::create_dir_all(dir.join("libcore/libcore")).unwrap();
    fs::write(dir.join("libcore/libcore/__init__.py"), "").unwrap();
    mk_bases(&dir.join("libcore/libcore"));
    mk_factory(&dir.join("libcore/libcore"), "libcore", "Safe");

    let _ = &dst_venv;
    with_cleared_virtual_env(|| {
        let cache = fresh_cache_dir("scen-editable");
        let _ = report_json_with_cache(&dir, &cache);
        mk_factory(&dir.join("libcore/libcore"), "libcore", "Blocking");
        incremental_vs_cold(&dir, &cache, "editable");
    });
}

#[test]
fn scen_editadd() {
    let nested = clone_fixture("m3c_nested_pkg_editable");
    let dir = temp_project(&[(
        "main.py",
        "from libcore.newm import f\nasync def run():\n    f()\n",
    )]);
    let src_venv = nested.join(".venv");
    let dst_venv = dir.join(".venv");
    copy_dir(&src_venv, &dst_venv);
    fs::create_dir_all(dir.join("libcore/libcore")).unwrap();
    fs::write(dir.join("libcore/libcore/__init__.py"), "").unwrap();

    let _ = &dst_venv;
    with_cleared_virtual_env(|| {
        let cache = fresh_cache_dir("scen-editadd");
        let _ = report_json_with_cache(&dir, &cache);
        fs::write(
            dir.join("libcore/libcore/newm.py"),
            "import time\ndef f():\n    time.sleep(1)\n",
        )
        .unwrap();
        incremental_vs_cold(&dir, &cache, "editadd");
    });
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn setup_srctests() -> std::path::PathBuf {
    let dir = temp_project(&[
        ("pyproject.toml", "[project]\nname=\"x\"\n"),
        ("src/pkg/__init__.py", ""),
        (
            "tests/conftest.py",
            "import pytest\nfrom pkg.factory import make\n@pytest.fixture\ndef thing():\n    return make()\n",
        ),
        (
            "tests/test_a.py",
            "from pkg.factory import make\nasync def test_x():\n    make().go()\n",
        ),
    ]);
    mk_bases(&dir.join("src/pkg"));
    mk_factory(&dir.join("src/pkg"), "pkg", "Safe");
    dir
}

#[test]
fn scen_srctests() {
    let dir = setup_srctests();
    let cache = fresh_cache_dir("scen-srctests");
    let _ = report_json_with_cache(&dir, &cache);
    mk_factory(&dir.join("src/pkg"), "pkg", "Blocking");
    incremental_vs_cold(&dir, &cache, "srctests");
}

#[test]
fn scen_srctestsadd() {
    let dir = setup_srctests();
    fs::write(
        dir.join("tests/test_b.py"),
        "from pkg.later import f\nasync def test_y():\n    f()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-srctestsadd");
    let before = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("src/pkg/later.py"),
        "import time\ndef f():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "srctestsadd");
    let after = report_json_with_cache(&dir, &fresh_cache_dir("scen-srctestsadd-cold"));
    let nb: usize = serde_json::from_str::<serde_json::Value>(&before).unwrap()["findings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let na: usize = serde_json::from_str::<serde_json::Value>(&after).unwrap()["findings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        na >= nb,
        "srctestsadd: findings must not drop (before={nb} after={na})"
    );
}

#[test]
fn scen_testhelper() {
    let dir = setup_srctests();
    fs::write(
        dir.join("tests/test_c.py"),
        "from helpers import mk\nasync def test_z():\n    mk().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("tests/helpers.py"),
        "from pkg.bases import Safe, Blocking\ndef mk(x=None) -> Safe:\n    return x\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-testhelper");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("tests/helpers.py"),
        "from pkg.bases import Safe, Blocking\ndef mk(x=None) -> Blocking:\n    return x\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "testhelper");
}

#[test]
fn scen_ruffroot() {
    let dir = temp_project(&[
        ("pyproject.toml", "[tool.ruff]\nsrc = [\"tools/qa\"]\n"),
        ("tools/qa/bot/__init__.py", ""),
        (
            "tools/qa/bot/main.py",
            "from bot.factory import make\nasync def run():\n    make().go()\n",
        ),
    ]);
    mk_bases(&dir.join("tools/qa/bot"));
    mk_factory(&dir.join("tools/qa/bot"), "bot", "Safe");
    let cache = fresh_cache_dir("scen-ruffroot");
    let _ = report_json_with_cache(&dir, &cache);
    mk_factory(&dir.join("tools/qa/bot"), "bot", "Blocking");
    incremental_vs_cold(&dir, &cache, "ruffroot");
}

#[test]
fn scen_ruffadd() {
    let dir = temp_project(&[
        ("pyproject.toml", "[tool.ruff]\nsrc = [\"tools/qa\"]\n"),
        ("tools/qa/bot/__init__.py", ""),
        (
            "tools/qa/bot/main.py",
            "from bot.factory import make\nasync def run():\n    make().go()\n",
        ),
        (
            "tools/qa/bot/main2.py",
            "from bot.newm import f\nasync def run2():\n    f()\n",
        ),
    ]);
    mk_bases(&dir.join("tools/qa/bot"));
    mk_factory(&dir.join("tools/qa/bot"), "bot", "Safe");
    let cache = fresh_cache_dir("scen-ruffadd");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("tools/qa/bot/newm.py"),
        "import time\ndef f():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "ruffadd");
}

#[test]
fn scen_tyextra() {
    let dir = temp_project(&[
        (
            "pyproject.toml",
            "[tool.ty.environment]\nextra-paths = [\"libs\"]\n",
        ),
        ("libs/mylib/__init__.py", ""),
        (
            "app.py",
            "from mylib.factory import make\nasync def run():\n    make().go()\n",
        ),
    ]);
    mk_bases(&dir.join("libs/mylib"));
    mk_factory(&dir.join("libs/mylib"), "mylib", "Safe");
    let cache = fresh_cache_dir("scen-tyextra");
    let _ = report_json_with_cache(&dir, &cache);
    mk_factory(&dir.join("libs/mylib"), "mylib", "Blocking");
    incremental_vs_cold(&dir, &cache, "tyextra");
}

#[test]
fn scen_pyright() {
    let dir = temp_project(&[
        ("pyrightconfig.json", "{\"extraPaths\": [\"libs\"]}\n"),
        ("libs/mylib/__init__.py", ""),
        (
            "app.py",
            "from mylib.newm import f\nasync def run():\n    f()\n",
        ),
    ]);
    let cache = fresh_cache_dir("scen-pyright");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("libs/mylib/newm.py"),
        "import time\ndef f():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "pyright");
}

