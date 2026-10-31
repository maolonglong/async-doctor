//! Port of `/tmp/inc/scen2.sh` setups: incremental JSON == cold JSON.

use std::fs;

use super::super::helpers::{fresh_cache_dir, report_json_with_cache};
use super::helpers::{incremental_vs_cold, scen_base as make_base};

#[test]
fn scen_retann() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.factory import make\nasync def run():\n    make().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/factory.py"),
        "from pkg.bases import Safe, Blocking\ndef make(x=None) -> Safe:\n    return x\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-retann");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/factory.py"),
        "from pkg.bases import Safe, Blocking\ndef make(x=None) -> Blocking:\n    return x\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "retann");
}

#[test]
fn scen_modvar() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.cfg import client\nasync def run():\n    client.go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/cfg.py"),
        "from pkg.bases import Safe, Blocking\nclient = Safe()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-modvar");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/cfg.py"),
        "from pkg.bases import Safe, Blocking\nclient = Blocking()\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "modvar");
}

#[test]
fn scen_alias() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.al import go\nasync def run():\n    go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/fns.py"),
        "import time\ndef safe():\n    pass\ndef blk():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/al.py"),
        "from pkg.fns import safe, blk\ngo = safe\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-alias");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/al.py"),
        "from pkg.fns import safe, blk\ngo = blk\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "alias");
}

#[test]
fn scen_ret3() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.factory import make\nasync def run():\n    make().conn().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/factory.py"),
        "from pkg.impl import Impl\ndef make() -> Impl:\n    return Impl()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/impl.py"),
        "from pkg.bases import Safe, Blocking\nclass Impl:\n    def conn(self, x=None) -> Safe:\n        return x\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-ret3");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/impl.py"),
        "from pkg.bases import Safe, Blocking\nclass Impl:\n    def conn(self, x=None) -> Blocking:\n        return x\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "ret3");
}

#[test]
fn scen_star() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.st import *\nasync def run():\n    go()\n",
    )
    .unwrap();
    fs::write(dir.join("pkg/st.py"), "def other():\n    pass\n").unwrap();
    let cache = fresh_cache_dir("scen-star");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/st.py"),
        "import time\ndef other():\n    pass\ndef go():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "star");
}

#[test]
fn scen_star2() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.mid3 import *\nasync def run():\n    go()\n",
    )
    .unwrap();
    fs::write(dir.join("pkg/mid3.py"), "from pkg.st import *\n").unwrap();
    fs::write(dir.join("pkg/st.py"), "def go():\n    pass\n").unwrap();
    let cache = fresh_cache_dir("scen-star2");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/st.py"),
        "import time\ndef go():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "star2");
}

#[test]
fn scen_impas() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "import pkg.mid4 as m\nasync def run():\n    m.u.work()\n",
    )
    .unwrap();
    fs::write(dir.join("pkg/mid4.py"), "from pkg import util as u\n").unwrap();
    fs::write(dir.join("pkg/util.py"), "def work():\n    pass\n").unwrap();
    fs::write(dir.join("pkg/util2.py"), "def work():\n    pass\n").unwrap();
    let cache = fresh_cache_dir("scen-impas");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "impas");
}

#[test]
fn scen_mv() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-mv");
    let _ = report_json_with_cache(&dir, &cache);
    fs::rename(dir.join("pkg/util.py"), dir.join("pkg/util_new.py")).unwrap();
    let app = fs::read_to_string(dir.join("pkg/app.py"))
        .unwrap()
        .replace("from pkg import util", "from pkg import util_new as util");
    fs::write(dir.join("pkg/app.py"), app).unwrap();
    incremental_vs_cold(&dir, &cache, "mv");
}

#[test]
fn scen_pyi() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.cfg import client\nasync def run():\n    client.go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    fs::write(dir.join("pkg/cfg.py"), "client = None\n").unwrap();
    fs::write(
        dir.join("pkg/cfg.pyi"),
        "from pkg.bases import Safe\nclient: Safe\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-pyi");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/cfg.pyi"),
        "from pkg.bases import Blocking\nclient: Blocking\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "pyi");
}

#[test]
fn scen_pyiadd() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.cfg import client\nasync def run():\n    client.go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/cfg.py"),
        "from pkg.bases import Safe\nclient = Safe()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-pyiadd");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/cfg.pyi"),
        "from pkg.bases import Blocking\nclient: Blocking\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "pyiadd");
}

#[test]
fn scen_newpkg() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.sub.deep import f\nasync def run():\n    f()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-newpkg");
    let _ = report_json_with_cache(&dir, &cache);
    fs::create_dir_all(dir.join("pkg/sub")).unwrap();
    fs::write(dir.join("pkg/sub/__init__.py"), "").unwrap();
    fs::write(
        dir.join("pkg/sub/deep.py"),
        "import time\ndef f():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "newpkg");
}

#[test]
fn scen_rel2() {
    let dir = make_base();
    fs::create_dir_all(dir.join("pkg/a/b")).unwrap();
    fs::write(dir.join("pkg/a/__init__.py"), "").unwrap();
    fs::write(dir.join("pkg/a/b/__init__.py"), "").unwrap();
    fs::write(
        dir.join("pkg/a/b/svc.py"),
        "from ...util import work\nasync def run():\n    work()\n",
    )
    .unwrap();
    fs::write(dir.join("pkg/util.py"), "def work():\n    pass\n").unwrap();
    let cache = fresh_cache_dir("scen-rel2");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "rel2");
}

#[test]
fn scen_base2() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.mid2 import Child\nasync def run():\n    Child().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/mid2.py"),
        "from pkg.bases import Safe, Blocking\nclass Child(Safe):\n    def other(self):\n        pass\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-base2");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/mid2.py"),
        "from pkg.bases import Safe, Blocking\nclass Child(Blocking):\n    def other(self):\n        pass\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "base2");
}

#[test]
fn scen_bodyonly() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-bodyonly");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    x = 1\n    time.sleep(x)\n    time.sleep(2)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "bodyonly");
}
