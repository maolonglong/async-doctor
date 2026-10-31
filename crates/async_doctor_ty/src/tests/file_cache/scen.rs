//! Port of `/tmp/inc/scen.sh` setups: incremental JSON == cold JSON.

use std::fs;

use super::super::helpers::{fresh_cache_dir, report_json_with_cache};
use super::helpers::{as_package_dir_root, incremental_vs_cold, scen_base as make_base};

#[test]
fn scen_shift() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-shift");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/low.py"),
        "import time\nX = 1\nY = 2\ndef helper():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "shift");
}

#[test]
fn scen_trans() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-trans");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/low.py"),
        "import time\ndef helper():\n    pass\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "trans");
}

#[test]
fn scen_sub() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-sub");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    pass\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "sub");
}

#[test]
fn scen_ren() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-ren");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work2():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "ren");
}

#[test]
fn scen_add() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/app2.py"),
        "from pkg.newmod import f\nasync def h3():\n    f()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-add");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/newmod.py"),
        "import time\ndef f():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "add");
}

#[test]
fn scen_del() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-del");
    let _ = report_json_with_cache(&dir, &cache);
    fs::remove_file(dir.join("pkg/util.py")).unwrap();
    incremental_vs_cold(&dir, &cache, "del");
}

#[test]
fn scen_del2() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-del2");
    let _ = report_json_with_cache(&dir, &cache);
    fs::remove_file(dir.join("pkg/low.py")).unwrap();
    incremental_vs_cold(&dir, &cache, "del2");
}

#[test]
fn scen_direct() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    pass\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-direct");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "direct");
}

#[test]
fn scen_inh() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.mid2 import Child\nasync def run():\n    Child().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/mid2.py"),
        "from pkg.basec import Base\nclass Child(Base):\n    pass\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/basec.py"),
        "class Base:\n    def go(self):\n        pass\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-inh");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/basec.py"),
        "import time\nclass Base:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "inh");
}

#[test]
fn scen_ret() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.factory import make\nasync def run():\n    make().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/factory.py"),
        "from pkg.impl import Impl\ndef make() -> Impl:\n    return Impl()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/impl.py"),
        "class Impl:\n    def go(self):\n        pass\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-ret");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/impl.py"),
        "import time\nclass Impl:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "ret");
}

#[test]
fn scen_ret2() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.factory import make\nasync def run():\n    make().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/factory.py"),
        "from pkg.impl import Impl\ndef make() -> Impl:\n    return Impl()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/impl.py"),
        "import time\nclass Impl:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-ret2");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/impl.py"),
        "import time\n# pad\n# pad\nclass Impl:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "ret2");
}

#[test]
fn scen_relsub() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/app3.py"),
        "from . import util\nasync def h4():\n    util.work()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    pass\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-relsub");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "relsub");
}

#[test]
fn scen_absroot() {
    let (parent, pkg) = as_package_dir_root(
        &[
            ("__init__.py", ""),
            (
                "app.py",
                "from pkg.mid import helper\nfrom pkg import util\nasync def handler():\n    helper()\nasync def h2():\n    util.work()\n",
            ),
            ("mid.py", "from pkg.low import helper\n"),
            ("low.py", "import time\ndef helper():\n    time.sleep(1)\n"),
            ("util.py", "import time\ndef work():\n    time.sleep(1)\n"),
            (
                "app4.py",
                "from pkg.util import work\nasync def h5():\n    work()\n",
            ),
        ],
        "scen-absroot",
    );
    let cache = fresh_cache_dir("scen-absroot");
    let _ = report_json_with_cache(&pkg, &cache);
    fs::write(
        pkg.join("util.py"),
        "import time\ndef work2():\n    time.sleep(1)\ndef work():\n    pass\n",
    )
    .unwrap();
    incremental_vs_cold(&pkg, &cache, "absroot");
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn scen_ss() {
    let dir = make_base();
    let cache = fresh_cache_dir("scen-ss");
    let _ = report_json_with_cache(&dir, &cache);
    let util = dir.join("pkg/util.py");
    let text = fs::read_to_string(&util)
        .unwrap()
        .replace("time.sleep(1)", "time.sleeq(1)");
    fs::write(&util, text).unwrap();
    incremental_vs_cold(&dir, &cache, "ss");
}

#[test]
fn scen_base() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/svc.py"),
        "from pkg.mid2 import Child\nasync def run():\n    Child().go()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/mid2.py"),
        "from pkg.bases import Safe, Blocking\nclass Child(Safe):\n    pass\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/bases.py"),
        "import time\nclass Safe:\n    def go(self):\n        pass\nclass Blocking:\n    def go(self):\n        time.sleep(1)\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-base");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/mid2.py"),
        "from pkg.bases import Safe, Blocking\nclass Child(Blocking):\n    pass\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "base");
}

#[test]
fn scen_reexp() {
    let dir = make_base();
    fs::write(dir.join("pkg/__init__.py"), "from pkg.safe import go\n").unwrap();
    fs::write(dir.join("pkg/safe.py"), "def go():\n    pass\n").unwrap();
    fs::write(
        dir.join("pkg/blk.py"),
        "import time\ndef go():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/app5.py"),
        "from pkg import go\nasync def h6():\n    go()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-reexp");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(dir.join("pkg/__init__.py"), "from pkg.blk import go\n").unwrap();
    incremental_vs_cold(&dir, &cache, "reexp");
}

#[test]
fn scen_move() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/app6.py"),
        "from pkg.util import work\nasync def h7():\n    work()\n",
    )
    .unwrap();
    fs::write(dir.join("pkg/util.py"), "def work():\n    pass\n").unwrap();
    let cache = fresh_cache_dir("scen-move");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util2.py"),
        "import time\ndef work():\n    time.sleep(1)\n",
    )
    .unwrap();
    fs::write(dir.join("pkg/util.py"), "from pkg.util2 import work\n").unwrap();
    incremental_vs_cold(&dir, &cache, "move");
}

#[test]
fn scen_circ() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/ca.py"),
        "from pkg import cb\ndef fa():\n    return cb.fb()\nasync def hc():\n    fa()\n",
    )
    .unwrap();
    fs::write(
        dir.join("pkg/cb.py"),
        "from pkg import ca\ndef fb():\n    pass\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-circ");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/cb.py"),
        "import time\nfrom pkg import ca\ndef pad():\n    pass\ndef fb():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "circ");
}

#[test]
fn scen_mlren() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/app7.py"),
        "from pkg import (\n    util,\n)\nasync def h8():\n    util.work()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-mlren");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work2():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "mlren");
}

#[test]
fn scen_bsren() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/app8.py"),
        "from pkg import \\\n    util\nasync def h9():\n    util.work()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-bsren");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work2():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "bsren");
}

#[test]
fn scen_semi() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/app9.py"),
        "import os; from pkg import util\nasync def h10():\n    util.work()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-semi");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work2():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "semi");
}

#[test]
fn scen_addtop() {
    let dir = make_base();
    fs::write(
        dir.join("app10.py"),
        "from newtop import f\nasync def h11():\n    f()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-addtop");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("newtop.py"),
        "import time\ndef f():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "addtop");
}

#[test]
fn scen_fnimp() {
    let dir = make_base();
    fs::write(
        dir.join("pkg/app11.py"),
        "async def h12():\n    from pkg import util\n    util.work()\n",
    )
    .unwrap();
    let cache = fresh_cache_dir("scen-fnimp");
    let _ = report_json_with_cache(&dir, &cache);
    fs::write(
        dir.join("pkg/util.py"),
        "import time\ndef work2():\n    time.sleep(1)\n",
    )
    .unwrap();
    incremental_vs_cold(&dir, &cache, "fnimp");
}
