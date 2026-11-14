//! Stdlib filesystem, path, package-resource, archive, and console IO sinks.

use crate::{BuiltinSink, RULE_BLOCKING_FILE};

pub(super) const FILESYSTEM_SINKS: &[BuiltinSink] = &[
    // --- file IO (blocking findings; never typing.{IO,BinaryIO,TextIO} or BytesIO/StringIO) ---
    BuiltinSink::finding(
        "builtins.open",
        RULE_BLOCKING_FILE,
        "Use `aiofiles` / `anyio` file APIs, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "io.open",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.read",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.write",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.readline",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.readlines",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.BufferedReader.read",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.BufferedReader.readline",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.BufferedReader.readlines",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.BufferedReader.readinto",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.BufferedWriter.write",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.FileIO.read",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "io.FileIO.readall",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "io.FileIO.readinto",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "io.FileIO.write",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload with `asyncio.to_thread`.",
    ),
    // --- pathlib ---
    BuiltinSink::finding(
        "pathlib.Path.read_text",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.read_text)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.write_text",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.write_text)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.read_bytes",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.write_bytes",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.open",
        RULE_BLOCKING_FILE,
        "Offload file IO, or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.mkdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.mkdir, ...)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.rmdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.rmdir)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.unlink",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.unlink, ...)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.rename",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.rename, ...)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.replace",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.replace, ...)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.walk",
        RULE_BLOCKING_FILE,
        "Offload directory walks with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.touch",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload with `asyncio.to_thread(path.touch, ...)` on a slow FS.",
        "cheap-path-op",
    ),
    // --- importlib.resources / importlib_resources (Traversable + functional APIs) ---
    BuiltinSink::finding(
        "importlib.resources.abc.Traversable.read_text",
        RULE_BLOCKING_FILE,
        "Offload package resource reads with `asyncio.to_thread` or cache at startup.",
    ),
    BuiltinSink::finding(
        "importlib.resources.abc.Traversable.read_bytes",
        RULE_BLOCKING_FILE,
        "Offload package resource reads with `asyncio.to_thread` or cache at startup.",
    ),
    BuiltinSink::finding(
        "importlib.resources.abc.Traversable.open",
        RULE_BLOCKING_FILE,
        "Offload package resource IO with `asyncio.to_thread` or cache at startup.",
    ),
    BuiltinSink::finding(
        "importlib.resources.abc.Traversable.iterdir",
        RULE_BLOCKING_FILE,
        "Offload package resource traversal with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "importlib.resources.read_text",
        RULE_BLOCKING_FILE,
        "Offload package resource reads with `asyncio.to_thread` or cache at startup.",
    ),
    BuiltinSink::finding(
        "importlib.resources.read_binary",
        RULE_BLOCKING_FILE,
        "Offload package resource reads with `asyncio.to_thread` or cache at startup.",
    ),
    BuiltinSink::finding(
        "importlib.resources.open_text",
        RULE_BLOCKING_FILE,
        "Offload package resource IO with `asyncio.to_thread` or cache at startup.",
    ),
    BuiltinSink::finding(
        "importlib.resources.open_binary",
        RULE_BLOCKING_FILE,
        "Offload package resource IO with `asyncio.to_thread` or cache at startup.",
    ),
    BuiltinSink::finding(
        "importlib.resources.as_file",
        RULE_BLOCKING_FILE,
        "Offload `importlib.resources.as_file` extraction/IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "importlib.resources.contents",
        RULE_BLOCKING_FILE,
        "Offload package resource listing with `asyncio.to_thread`.",
    ),
    // --- os raw fd + directory mutations ---
    BuiltinSink::finding(
        "os.read",
        RULE_BLOCKING_FILE,
        "Offload raw fd reads with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.write",
        RULE_BLOCKING_FILE,
        "Offload raw fd writes with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.pread",
        RULE_BLOCKING_FILE,
        "Offload raw fd reads with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.pwrite",
        RULE_BLOCKING_FILE,
        "Offload raw fd writes with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.fsync",
        RULE_BLOCKING_FILE,
        "Offload disk flush (`os.fsync`) with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.fdatasync",
        RULE_BLOCKING_FILE,
        "Offload disk flush (`os.fdatasync`) with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.listdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(os.listdir, ...)`.",
    ),
    BuiltinSink::finding(
        "os.walk",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "os.scandir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.remove",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.unlink",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.rename",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.mkdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.makedirs",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.rmdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    // --- shutil ---
    BuiltinSink::finding(
        "shutil.copy",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.copy2",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.copyfile",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.copyfileobj",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.copytree",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.move",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.rmtree",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.make_archive",
        RULE_BLOCKING_FILE,
        "Offload archive creation with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.unpack_archive",
        RULE_BLOCKING_FILE,
        "Offload archive extraction with `asyncio.to_thread`.",
    ),
    // --- small metadata → candidates ---
    BuiltinSink::candidate(
        "os.stat",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if contended or on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.lstat",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if contended or on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.exists",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.isfile",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.isdir",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.getsize",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.exists",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.stat",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.is_file",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.is_dir",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "importlib.resources.abc.Traversable.is_file",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS or zip-backed resource.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "importlib.resources.abc.Traversable.is_dir",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS or zip-backed resource.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "importlib.resources.is_resource",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS or zip-backed resource.",
        "metadata",
    ),
    // --- archives ---
    BuiltinSink::finding(
        "zipfile.ZipFile",
        RULE_BLOCKING_FILE,
        "Offload zip open/extract/read/write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.open",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.read",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.write",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.writestr",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.extract",
        RULE_BLOCKING_FILE,
        "Offload zip extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.extractall",
        RULE_BLOCKING_FILE,
        "Offload zip extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.open",
        RULE_BLOCKING_FILE,
        "Offload tar open/extract/read/write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.open",
        RULE_BLOCKING_FILE,
        "Offload tar open/extract/read/write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.extract",
        RULE_BLOCKING_FILE,
        "Offload tar extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.extractall",
        RULE_BLOCKING_FILE,
        "Offload tar extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.add",
        RULE_BLOCKING_FILE,
        "Offload tar write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.addfile",
        RULE_BLOCKING_FILE,
        "Offload tar write with `asyncio.to_thread`.",
    ),
    // --- glob / pathlib directory walks (tier with os.listdir) ---
    BuiltinSink::finding(
        "glob.glob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(glob.glob, ...)`.",
    ),
    BuiltinSink::finding(
        "glob.iglob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.iterdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.glob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.rglob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    // --- stdin ---
    BuiltinSink::finding(
        "builtins.input",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(input, ...)`, or use async stdin APIs.",
    ),
];
