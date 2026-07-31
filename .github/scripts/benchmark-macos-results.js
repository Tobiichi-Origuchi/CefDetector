ObjC.import("Foundation");

function readJson(path) {
  const text = $.NSString.stringWithContentsOfFileEncodingError(
    path,
    $.NSUTF8StringEncoding,
    null
  );
  if (!text) {
    throw new Error(`Unable to read ${path}`);
  }
  return JSON.parse(ObjC.unwrap(text));
}

function writeText(path, text) {
  const value = $(text);
  if (
    !value.writeToFileAtomicallyEncodingError(
      path,
      true,
      $.NSUTF8StringEncoding,
      null
    )
  ) {
    throw new Error(`Unable to write ${path}`);
  }
}

function run(argv) {
  const mode = argv[0];
  if (mode === "summarize") {
    const apps = readJson(argv[1]);
    const total = apps.reduce((sum, app) => sum + app.size, 0);
    return `${apps.length} ${total}`;
  }
  if (mode === "compare") {
    const ignore = new Set(readJson(argv[1]).map(app => app.file));
    const spotlight = new Set(readJson(argv[2]).map(app => app.file));
    const output = argv[3];
    const reports = {
      "ignore-only.txt": [...ignore].filter(path => !spotlight.has(path)),
      "spotlight-only.txt": [...spotlight].filter(path => !ignore.has(path)),
      "intersection.txt": [...ignore].filter(path => spotlight.has(path)),
    };
    for (const [name, paths] of Object.entries(reports)) {
      paths.sort();
      const text = paths.map(JSON.stringify).join("\n");
      writeText(`${output}/${name}`, text + (paths.length ? "\n" : ""));
    }
    return "";
  }
  throw new Error(`Unknown mode: ${mode}`);
}
