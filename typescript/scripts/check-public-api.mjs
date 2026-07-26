import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

// This gate guards the public API surface of the two published entry points on
// two fronts:
//
//   1. Specifier check — no published `.d.ts` may name a private transport
//      (koffi, litesvm, the internal kernel) in an import/export.
//   2. Surface snapshot — the public method/property names of the exported
//      `Test` and `Outcome` classes, resolved from the built `.d.ts` (inherited
//      and static members included, `protected`/`private`/`#private` excluded),
//      must equal a committed expected list. A new public method, or an
//      accidentally-public install-plumbing method, fails the gate until the
//      list below is updated deliberately.
//
// Run `node scripts/check-public-api.mjs --print` to dump the current surface
// (useful when intentionally changing the API — paste the output into EXPECTED).

const require = createRequire(import.meta.url);
const ts = require("typescript");

const pkg = JSON.parse(
  await readFile(new URL("../package.json", import.meta.url), "utf8"),
);
const entryDeclarations = Object.values(pkg.exports).map(
  entry => new URL(`../${entry.types.replace(/^\.\//, "")}`, import.meta.url),
);

// --- 1. Forbidden private-transport specifiers ------------------------------

// Match an actual module specifier for a private transport (a quoted `koffi`,
// `litesvm`, or a `./internal/kernel` path in an import/export), not the bare
// word, which legitimately appears in prose and doc comments.
const forbidden = [
  { label: "the koffi FFI runtime", pattern: /["']koffi(?:\/[^"']*)?["']/ },
  { label: "the LiteSVM backend", pattern: /["']litesvm(?:\/[^"']*)?["']/ },
  { label: "the internal kernel", pattern: /["'][^"']*internal\/kernel(?:\.js)?["']/ },
];

for (const declaration of entryDeclarations) {
  const source = await readFile(declaration, "utf8");
  for (const { label, pattern } of forbidden) {
    if (pattern.test(source)) {
      throw new Error(
        `${declaration.pathname} exposes ${label} in the public API`,
      );
    }
  }
}

// --- 2. Public surface snapshot ---------------------------------------------

// The expected public surface, shared by both entry points (Kit and Web3.js
// expose the identical member names; only the address/account types differ).
// Keep alphabetically sorted. Update deliberately when the public API changes.
const EXPECTED = {
  Test: {
    instance: [
      "[Symbol.dispose]",
      "account",
      "accountAs",
      "add",
      "deriveAta",
      "derivePda",
      "derivePdaWithBump",
      "free",
      "invariant",
      "lamports",
      "preloadProgram",
      "programId",
      "read",
      "send",
      "sendAll",
      "sendAllWith",
      "sendWith",
      "setAccount",
      "setComputeUnitLimit",
      "simulate",
      "simulateAll",
      "simulateAllWith",
      "simulateWith",
      "supply",
      "tokens",
      "warpToTimestamp",
      "write",
    ],
    static: ["open"],
  },
  Outcome: {
    instance: [
      "account",
      "accountAs",
      "accountChanges",
      "accounts",
      "check",
      "checks",
      "computeUnits",
      "error",
      "events",
      "fails",
      "failsWith",
      "isErr",
      "isOk",
      "logs",
      "returnData",
      "returnValue",
      "succeeds",
    ],
    static: [],
  },
};

const entryPaths = entryDeclarations.map(url => fileURLToPath(url));
const program = ts.createProgram(entryPaths, {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.Node16,
  moduleResolution: ts.ModuleResolutionKind.Node16,
  lib: ["lib.es2022.d.ts", "lib.esnext.disposable.d.ts"],
  skipLibCheck: true,
});
const checker = program.getTypeChecker();

/** Whether a member is hidden from the public surface (protected/private/#). */
function isHidden(symbol) {
  const escaped = symbol.escapedName?.toString() ?? "";
  if (escaped.startsWith("__#")) return true; // hard #private field
  const declarations = symbol.getDeclarations() ?? [];
  return declarations.some(declaration => {
    const flags = ts.getCombinedModifierFlags(declaration);
    return (
      (flags & ts.ModifierFlags.Private) !== 0 ||
      (flags & ts.ModifierFlags.Protected) !== 0
    );
  });
}

/** A member's public-facing name, normalizing well-known symbol members. */
function memberName(symbol) {
  const escaped = symbol.escapedName?.toString() ?? symbol.getName();
  const wellKnown = /^__@(.+)@\d+$/.exec(escaped);
  return wellKnown ? `[Symbol.${wellKnown[1]}]` : symbol.getName();
}

/** Public member names of a type, inherited members included, sorted. */
function publicMembers(type) {
  return [
    ...new Set(
      type
        .getProperties()
        .filter(symbol => !isHidden(symbol))
        .map(memberName),
    ),
  ]
    .filter(name => name !== "prototype")
    .sort();
}

/** The instance and static public surface of an exported class/type. */
function surfaceOf(moduleSymbol, name) {
  const exported = checker
    .getExportsOfModule(moduleSymbol)
    .find(symbol => symbol.getName() === name);
  if (exported === undefined) {
    throw new Error(`export \`${name}\` not found`);
  }
  const instanceType = checker.getDeclaredTypeOfSymbol(exported);
  const instance = publicMembers(instanceType);
  let staticMembers = [];
  if (exported.valueDeclaration) {
    const staticType = checker.getTypeOfSymbolAtLocation(
      exported,
      exported.valueDeclaration,
    );
    staticMembers = publicMembers(staticType);
  }
  return { instance, static: staticMembers };
}

const printOnly = process.argv.includes("--print");
const failures = [];

for (const entryPath of entryPaths) {
  const source = program.getSourceFile(entryPath);
  const moduleSymbol = checker.getSymbolAtLocation(source);
  const label = entryPath.split("/").pop();
  for (const name of Object.keys(EXPECTED)) {
    const actual = surfaceOf(moduleSymbol, name);
    if (printOnly) {
      console.log(`${label} ${name}:`, JSON.stringify(actual, null, 2));
      continue;
    }
    for (const kind of ["instance", "static"]) {
      const expected = EXPECTED[name][kind];
      const actualNames = actual[kind];
      const added = actualNames.filter(n => !expected.includes(n));
      const removed = expected.filter(n => !actualNames.includes(n));
      if (added.length > 0 || removed.length > 0) {
        failures.push(
          `${label} ${name} (${kind}) surface drift:` +
            (added.length ? `\n  + unexpected public: ${added.join(", ")}` : "") +
            (removed.length ? `\n  - missing expected: ${removed.join(", ")}` : ""),
        );
      }
    }
  }
}

if (printOnly) process.exit(0);

if (failures.length > 0) {
  throw new Error(
    `public API surface changed (update EXPECTED in scripts/check-public-api.mjs ` +
      `only when the change is intended):\n${failures.join("\n")}`,
  );
}
